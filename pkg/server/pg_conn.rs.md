# `pkg/server/pg_conn.rs`

## 文件定位

`pg_conn.rs` 是 `astersql-server` crate 内独立 PostgreSQL TCP 端口的连接级协议边界。模块由 `pkg/server/lib.rs` 公开为 `pub mod pg_conn`，生产入口是 `Server::start_postgres_accept`：它在 `Server::run` 完成监听器初始化、Domain 激活及 MySQL/status 服务启动后，将 PostgreSQL `TcpListener`、共享 `SessionDriver`、`ConnectionDomain` 和全局 `RequireSecureTransportEnabled()` 结果交给 `PgService::start`（`pkg/server/server.rs`）。停服时 `Server::close_listeners` 与 `Drop for Server` 都调用 `PgService::close`。

这个文件不实现另一套 SQL 引擎。它负责 PostgreSQL 3.0/3.2 的连接协商、认证接线、消息循环、取消请求和连接资源回收，然后把查询落到 `crate::conn::{SessionDriver, TiDBContext}` 所代表的 canonical 会话。启动包的结构解析在 `pkg/server/pg_protocol.rs`，扩展查询、名称/语句适配、catalog 和结果编码分别下沉到 `pg_extended.rs`、`pg_name.rs`、`pg_session.rs`、`pg_catalog.rs`、`pg_result.rs`。

## 核心职责

- `PgService::start` 创建非阻塞 accept 线程，并为每条已接受连接创建一个具名 worker；连接 ID 的申请和释放由 `ConnectionDomain` 统一管理。
- `PgService::negotiate` 处理 SSL/GSS 探测、CancelRequest、3.0/3.2 StartupMessage、启动参数校验、canonical 会话创建与认证，以及 AuthenticationOk/ParameterStatus/BackendKeyData/ReadyForQuery 响应。
- 认证后消息循环同时承接简单查询 `Q` 和 `pg_extended::Extended::handle` 已实现的扩展查询消息；SQL 最终经 `TiDBContext` 执行，结果经 `pg_result::write_result` 编码。
- `active` 注册表、`cancel` 和 `with_query` 共同限定取消的作用域：只有密钥匹配且目标正执行命令时才取消，并在每条命令结束时清除取消状态。
- `read_initial`、`read_message`、`write_message`、`write_error` 实现有界网络帧读写；`sqlstate` 将可信的 canonical 错误类别或精确错误形态映射到 PostgreSQL SQLSTATE。

## 主要符号

- 常量 `SSL_REQUEST`、`GSS_REQUEST`、`CANCEL_REQUEST` 是 PostgreSQL 特殊启动请求码；`MAX_MESSAGE` 将认证后单消息长度限制为 1 MiB。启动包上限来自 `pg_protocol::MAX_STARTUP_LENGTH`（10,000 字节）。
- `cancel_key_length(protocol_version)`：3.0 使用 4 字节 secret，3.2 使用 32 字节 secret；连同 4 字节 PID 构成对应的 BackendKeyData body。`pg_conn_test.rs::{startup_protocol_30_roundtrip,startup_auth_roundtrip}` 验证 body 分别为 8/36 字节。
- 私有 `Active`：记录协议版本、随机取消密钥、共享 `TiDBContext`、当前是否执行命令以及 catalog 专用 `CancellationToken`。它是取消安全性的连接级状态。
- `PgService`：持有服务启动时间、幂等停止标志、accept/worker 线程句柄、可强制 shutdown 的 socket 副本，以及按 PostgreSQL PID 索引的 `Active`。`resource_counts` 仅在测试构建中暴露资源数量。
- `PgService::{start,close}`：服务生命周期 API。`start` 返回 `Arc<PgService>`；`close` 原子地保证只执行一次，停止 accept、取消活动查询、关闭 socket 并 join worker。
- `PgService::negotiate`：单连接状态机的主体。它是私有方法，只由 `start` 创建的连接 worker 调用。
- `PgService::{cancel,catalog_cancel,with_query}`：取消注册表操作。`with_query` 的 RAII `Completion` 在正常返回或展开离开闭包时恢复 `executing=false` 并调用 `finish_query_cancellation`。
- 帧辅助函数 `read_initial`、`read_message`、`write_message`、`write_error`：其中后三者为 `pub(crate)`，供 `pg_extended.rs` 和 `pg_result.rs` 复用编码边界。
- `sqlstate(&ConnError)`：供本文件、`pg_extended.rs` 和 `pg_catalog.rs` 共享的错误分类器；未知错误保持 `XX000`，不从任意错误文本提取伪造 SQLSTATE。

## 执行流程

1. `Server::init_postgres_listener` 仅在 `ServerConfig::postgres_port` 为 `Some` 时绑定独立端口；`Server::start_postgres_accept` 在 driver/domain 均已安装时调用 `PgService::start`。
2. `start` 把 listener 设为非阻塞。accept 线程轮询连接；`WouldBlock` 时休眠 10 ms。接受连接后先申请 connection ID、克隆控制 socket 并登记到 `sockets`，再启动连接 worker。创建 worker 失败会撤销 socket 登记和 connection ID；成功时顺便 join 已完成的旧 worker。
3. worker 设置阻塞 I/O 与 10 秒读写超时，然后调用 `negotiate`。无论协商成功与否，worker 都尝试移除 `active`、关闭 context、shutdown socket、移除 `sockets` 并释放 connection ID；协议级未捕获错误会先编码为 `FATAL/08P01`。
4. `negotiate` 通过 `read_initial` 读取完整、有上限的首包。每种 SSL/GSS 探测至多一次并回复 `N`；重复探测报错。CancelRequest 校验总长，从包中取 PID 和 secret 调用 `cancel` 后立即结束该短连接。普通启动包交给 `parse_startup`，结构/版本错误返回 `FATAL/0A000`。
5. 若服务器要求安全传输，由于当前不支持 PostgreSQL TLS，返回 `FATAL/08004`。随后要求非空 `user`，并只接受 `user`、`database`、`application_name`、UTF-8 encoding、ISO DateStyle、正值 `extra_float_digits` 及受字符集约束的 TimeZone；其他参数返回 `0A000`。
6. 连接 ID 必须能收窄为 `u32`。`driver.open_ctx(id, 0, 45, "", None)` 先创建未选库会话；查询用户认证插件，只允许 `AUTH_NATIVE_PASSWORD`，再用空认证数据执行 canonical `authenticate`。当前 canonical driver 的实际支持范围是无 TLS、空密码的 root；认证失败返回 `28P01`。认证成功后才 `use_db`，再执行 `SET time_zone = '<validated>'`。
7. 生成密码学安全随机取消密钥并写入 `active`，依次发送 AuthenticationOk、五个 ParameterStatus、BackendKeyData 和 ReadyForQuery。之后读超时改为 `context.wait_timeout()`，并创建连接私有 `pg_extended::Extended`。
8. 消息循环先让 `Extended::handle` 处理扩展查询。简单查询 `Q` 要求单个 NUL 终止 UTF-8 字符串：先尝试 `SessionQuery`，再尝试 bounded catalog 查询，否则依次经过 `pg_name::adapt`、`pg_result::adapt_session_query` 与 `pg_result::command`，最后在 `with_query` 保护下执行。每轮返回结果或 ErrorResponse，再发送反映事务状态的 ReadyForQuery。
9. `X` 空消息正常终止；EOF 也视为正常断开。循环退出后注销 `active` 并关闭 context，随后 worker 完成其统一 socket/ID 清理。

## 数据与状态

- `startup_epoch_micros` 在服务启动时固定，传给扩展查询和结果适配，用于连接间一致的服务启动时间语义；它不是每查询时间戳。
- `stopped: AtomicBool` 用 Acquire/AcqRel 协调 accept 循环与幂等关闭。`accept`、`workers`、`sockets`、`active` 各自有独立 `Mutex`，避免把网络 I/O 放在一个全局锁下。
- `sockets: HashMap<u64, TcpStream>` 保存 worker socket 的克隆，仅用于停服时主动 `shutdown` 阻塞读；worker 所有权中的原 socket 承担正常 I/O。
- `active: HashMap<u32, Active>` 只在完成认证且已生成取消密钥后登记。PID 来自统一的 `u64` connection ID，但 PostgreSQL 边界拒绝超出 `u32` 的 ID。
- `Active::executing` 是取消有效窗口；`catalog_cancel` 每条命令前替换为新 token。普通引擎查询使用新的 `CancellationToken` 参数，同时 `context.cancel()` 提供 canonical 会话级取消；catalog 执行使用登记的 token。
- `Extended` 及其 session/statement/portal 状态局限在单 worker 栈帧；`Q` 到来时调用 `reset_unnamed`，避免未命名扩展查询状态泄漏到简单查询。

## 依赖与调用关系

上游生产调用链为 `Server::run -> Server::start_postgres_accept -> PgService::start -> PgService::negotiate`（`pkg/server/server.rs`）。关闭链为 `Server::{close_listeners,drop} -> PgService::close`。`pkg/server/lib.rs` 装配本模块及其独立测试文件。

主要下游边由 RustCodeGraph 对 `negotiate` 的查询确认，包括本文件的 `cancel`、`catalog_cancel`、`with_query`、帧与错误辅助函数，以及 `pkg/server/pg_protocol.rs::parse_startup`。源码还显示查询路径调用 `pg_extended::Extended::handle`、`pg_session::SessionQuery::parse/execute`、`pg_catalog::CatalogQuery::parse_session/execute`、`pg_name::adapt`、`pg_result::{adapt_session_query,command,write_result}` 和 `TiDBContext::{execute_query,execute_prepared_statement}`。

crate 边界由 `pkg/server/Cargo.toml` 确认：包名是 `astersql-server`、根文件是 `lib.rs`、`autotests=false`；本文件直接使用标准库 TCP/线程/同步原语和 `rustls` 的安全随机源，canonical 会话能力则由 crate 内 `conn` 抽象承接。测试因关闭自动发现而必须由 `lib.rs` 的 `#[cfg(test)] #[path = "pg_conn_test.rs"]` 显式装配。

## 错误处理与边界

- 启动包必须为 8 到 `MAX_STARTUP_LENGTH` 字节；认证后消息的长度字段必须为 4 到 1 MiB。超界在分配 payload 前被拒绝。简单查询必须是无内嵌 NUL 的单个 NUL 终止 UTF-8 字符串。
- 协商阶段区分协议错误与可正常编码的拒绝：无法继续的 I/O/帧错误向 worker 返回 `io::Error`，worker 尝试发送 `FATAL/08P01`；已识别的版本、认证、数据库、参数或安全传输拒绝直接发送对应 FATAL 后正常结束连接。
- 启动参数采用白名单。TimeZone 只能包含 ASCII 字母数字及 `/ _ + - : .`，随后才拼入 `SET time_zone`；不存在的时区由引擎错误映射为 `22023`。这同时是 SQL 注入边界。
- 不支持 TLS/GSS 协商，只回复 `N`；当全局要求安全传输时，不允许降级，明确返回 `08004`。认证插件不是 native password 时返回 `0A000`。
- 单次查询结果必须恰为一个 result；多结果返回 `0A000`。结果编码的 `InvalidData` 被转为可恢复的 `0A000`，其他写错误终止连接。`response_lifecycle` 在编码尝试后记录写时长并 finish。
- `sqlstate` 只匹配 `ConnError` 变体和少量 canonical 精确前缀/消息：访问拒绝、协议、停服、资源上限、OID、重复键、未知列/表/库、取消等有专用码，其余为 `XX000`。
- 当前边界明确不支持完整 PostgreSQL 语义；未被 `Extended::handle` 或简单查询路径识别的命令返回 `0A000`。文档不把 PostgreSQL 客户端可连接等同于完整 PostgreSQL 兼容。

## 并发与资源生命周期

accept 线程和每连接一个 worker 都由 `PgService` 持有 `JoinHandle`。accept 线程只短暂持有资源表锁；连接执行在独立线程中进行。worker 结束时释放 active context、socket 登记和 domain connection ID。accept 循环还会回收已完成 worker，避免句柄无限积累。

`close` 首先用原子交换实现幂等，再等待 accept 线程停止；之后取消所有活动 context/catalog token、shutdown 所有 socket 以唤醒阻塞读，最后取走并 join 全部 worker。顺序保证不会在仍继续 accept 新连接时清空 worker。`pg_conn_test.rs::startup_real_dual_listener_shutdown_closes_pending_and_authenticated_clients` 验证已认证连接和仅完成 SSL 探测的 pending 连接都会被关闭；`catalog_error_recovery_and_cleanup` 验证重复 `close`、断线后的 context 释放以及最终 `(active,sockets,workers) == (0,0,0)`。

取消路径使用 `active` 同一把锁串行化 `with_query` 的“进入执行”状态与 CancelRequest 检查。密钥长度必须符合目标连接协商版本，内容采用累积 XOR 比较而非遇到首个差异即返回；PID、secret、版本或长度任一不匹配都静默无效。RAII `Completion` 保证查询闭包如何返回都恢复状态，因此 idle/stale CancelRequest 不会污染下一条查询。`startup_cancel_is_scoped_and_consumed` 覆盖跨连接密钥、错误长度、跨版本密钥、idle/stale 请求以及取消后继续执行。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 把整个 crate 对应到 Go 包 `pkg/server`，但仓库中不存在 `pkg/server/pg_conn.go`，Go 的 `pkg/server/conn.go` 是既有 MySQL 连接实现，也没有这一独立 PostgreSQL listener 的同路径实现。因此本文件是 AsterSQL Rust 侧新增的 PostgreSQL 兼容边界，不能声称逐函数复刻 Go。

可复用的 Go/Rust 对齐点在 canonical 抽象而不是线协议：连接 ID 仍由 domain 分配/释放，认证、选库、查询、事务状态、取消、响应生命周期与 context 关闭仍通过 `SessionDriver`/`TiDBContext` 接入现有服务端语义。协议专属的 StartupMessage、BackendKeyData、SQLSTATE、catalog/name 适配和 PG 帧编码则没有同路径 Go 对照，应以本文件及独立 Rust 协议测试为事实来源。

## 扩展指南

- 新增启动参数：修改 `PgService::negotiate` 的白名单，同时明确其会话生效位置、注入约束和 ParameterStatus 回报；在 `pkg/server/pg_conn_test.rs` 扩展成功、拒绝和恶意输入用例。
- 新增启动期 TLS/GSS：接入点是 SSL/GSS request 分支和 `require_secure_transport` 判断；必须重做 socket 所有权、TLS 状态传递、认证请求的 `tls_state`、超时及停服 shutdown 测试，不能只把回复从 `N` 改成 `S/G`。
- 新增前端消息或扩展查询行为：优先放入 `pkg/server/pg_extended.rs`，保持 `negotiate` 只做连接级分派；同步 `pg_extended_test.rs`，并验证错误后的“忽略直到 Sync”恢复语义。
- 新增 SQL/catalog/name 兼容：分别在 `pg_session.rs`、`pg_catalog.rs`/`pg_catalog_query.rs`、`pg_name.rs`、`pg_result.rs` 实现，本文件只负责选择路径和生命周期；同步相应独立测试，不把大段语义堆回连接循环。
- 新增错误映射：只在 `sqlstate` 中基于稳定 `ConnError` 变体或 canonical 明确契约匹配，并扩展 `pg_error_test.rs`；避免从任意消息提取 SQLSTATE。
- 修改取消：保持“只取消正在执行的目标连接”“每命令重置 token”“命令退出必清理”三个不变量，并同步 `pg_conn_test.rs::startup_cancel_is_scoped_and_consumed`。密钥格式变化还需覆盖 3.0/3.2 客户端兼容。
- 修改线程/资源模型：必须维持 connection ID、context、socket、worker 的对称释放，并扩展 `catalog_error_recovery_and_cleanup` 与 server 双监听关闭测试；性能风险集中在每连接一线程、10 ms accept 轮询以及全局 `active` 锁的高并发竞争。

## 验证依据

- RustCodeGraph 索引状态：项目共 11,467 个已索引文件，目标 `pkg/server/pg_conn.rs` 被识别为含 53 个符号；读取了该文件全部 685 行。精确查询定位 `PgService`、`negotiate`、`with_query`、帧辅助函数与 `sqlstate`；`callees negotiate` 确认其到 `parse_startup`、取消/查询保护和消息辅助函数的直接边。通用名 `start` 存在跨仓库歧义，因此上游调用以明确的 `pkg/server/server.rs::start_postgres_accept` 源码为准。
- 已读生产证据：`pkg/server/pg_conn.rs`、`pkg/server/pg_protocol.rs`、`pkg/server/server.rs`、`pkg/server/lib.rs`、`pkg/server/Cargo.toml`；并用仓库搜索确认不存在同路径 `pkg/server/pg_conn.go`。
- 已读独立测试：`pkg/server/pg_conn_test.rs` 全部 622 行。它覆盖启动认证、协议版本与密钥尺寸、服务端参数、DataGrip 参数、取消隔离、secure mode 拒绝、双 listener 停服、错误恢复及资源清理。相关更深层测试还由 `lib.rs` 装配在 `pg_query_test.rs`、`pg_extended_test.rs`、`pg_error_test.rs`、`pg_catalog_test.rs`、`pg_types_test.rs` 和 `pg_client_integration_test.rs`。
- 人工事实复核结论：该文件存在的原因、生产入口、连接状态机、查询下沉路径、错误/取消边界、线程与资源关闭顺序以及安全扩展接入点，均可由上述符号和文件反查；未将缺少 Go 同路径实现的部分描述成 Go 已有能力。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证使用任务文件指定命令，结果记录在最终交付信息中。
