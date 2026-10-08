# `pkg/server/runtime.rs`

## 文件定位

`runtime.rs` 属于 `astersql-server` crate（`pkg/server/Cargo.toml` 的库入口为 `pkg/server/lib.rs`），是 Rust 版 MySQL server 与 canonical `astersql-domain` / `astersql-session` 实现之间的生产适配层。它不负责监听端口或解析完整命令循环；上游 `pkg/server/server.rs`、`pkg/server/conn.rs` 定义 server/connection 抽象，本文件为其中的 `PacketIo`、`ServerDomain`、`ConnectionDomain`、`ServerDriver`、`SessionDriver` 和 `TiDBContext` trait 提供真实实现。`cmd/tidb-server/main.rs::assembleCanonicalServer` 将 `CanonicalServerDomain`、`CanonicalConnectionDomain`、`CanonicalServerDriver` 与 `ConcreteSessionDriver` 装配进应用入口。

Go 版本没有同名 `runtime.go`：对应职责分散在 `pkg/server/internal/packetio.go`、`pkg/server/conn.go`、`pkg/server/driver_tidb.go` 和 `pkg/server/server.go`。因此本文件是迁移边界，而非某一个 Go 文件的逐行翻译。

## 核心职责

- `TcpPacketIo` / Unix 平台下的 `UnixPacketIo` 把 socket 接到既有 MySQL 包编解码器，处理序号、压缩、超时、关闭、地址、TCP TLS 与 PROXY protocol 延迟探测。
- `CanonicalServerDomain` 与 `CanonicalConnectionDomain` 把 canonical `Domain` 暴露为 server 所需的只读运行时能力，包括 DDL 标识、schema、系统变量、DXF 历史、TiFlash 进度、进程列表和连接 ID 生命周期。
- `ConcreteSessionDriver::open_ctx` 为每条连接创建独立的 session worker 线程，并返回线程安全的 `ConcreteTiDBContext` 门面。
- `run_session_worker` 串行持有非共享的 `ConcreteSession`，通过 `SessionRequest` 完成 SQL、prepared statement、field list、LOCAL INFILE、流式结果、重置和协议响应收尾。
- `protocol_column`、`result_metadata`、`protocol_value`、`prepared_argument` 等函数在 canonical session 数据与 MySQL wire 数据模型之间转换。

## 主要符号

- `TcpPacketIo`：公开 TCP transport；`new_with_options` 可注入 TLS 配置、已知 peer 地址及 `(networks, fallbackable, timeout)` PROXY 配置。首次 `read_packet` 才调用 `server::proxy_source_addr`，确保服务端握手包可先写出。
- `UnixPacketIo`（`cfg(unix)`）：Unix domain socket transport；复用包编解码和压缩，但地址按 localhost/Unix 路径表达，并明确不支持 TLS 升级。
- `BootstrapAuthMode::{InsecureRootOnly, SecureUnsupported}`：当前 bootstrap 鉴权能力边界；不是完整权限系统。
- `CanonicalServerDomain`：实现 `server::Domain`；持有 `Arc<Domain>`、构造时刻及 `CanonicalExtractRuntime`。
- `CanonicalConnectionDomain`：只代理 `next_connection_id` / `release_connection_id`。
- `CanonicalServerDriver`：`ServerDriver::name()` 固定返回 `"tidb"` 的标识类型。
- `ConcreteSessionDriver`：持有 canonical Domain、鉴权模式和弱引用 session manager；`open_ctx` 是连接到 session 的关键入口。
- `ConcreteTiDBContext`：实现 `conn::TiDBContext` 的同步门面；共享状态使用 `Mutex`/原子量，session 本体只在线程内访问。
- `SessionRequest`：线程间 RPC 枚举；每个需要结果的请求携带容量为 1 的同步响应通道，`Shutdown` 终止 worker。
- `run_session_worker`：创建、配置并独占 `ConcreteSession` 的事件循环。
- `execute_on_session` / `execute_prepared_on_session`：普通 SQL 和 prepared SQL 的执行与缓冲/流式结果分流。
- `result_metadata` / `protocol_value`：生成 wire 列元数据，并把 canonical NULL 哨兵、旧 `<nil>` 及内部二进制十六进制前缀归一化为 `Value`。

## 执行流程

1. 应用入口创建 canonical `Domain` 和 `CanonicalSessionFactory`，再构造 `ConcreteSessionDriver`、两个 Domain 适配器及 `CanonicalServerDriver`；`cmd/tidb-server/main.rs::assembleCanonicalServer` 是已索引的生产调用者。
2. server 接受连接后创建 `TcpPacketIo`（Unix listener 则创建 `UnixPacketIo`）。TCP 首次读包时可解析 PROXY header；客户端请求 TLS 时 `upgrade_to_tls` 完成 rustls 握手，并把共享 TLS reader 交给内部 `PacketIO`。
3. `ConcreteSessionDriver::open_ctx` 建立请求通道和初始化回执通道，启动命名为 `mysql-session-{connection_id}` 的线程。worker 创建 `ConcreteSession`，设置 manager、连接 ID、capability、collation 和可选初始数据库，然后把 `SQLKiller` 与 `TransactionMDL` 返回给门面。
4. 连接线程调用 `ConcreteTiDBContext` 方法。门面先检查关闭/取消状态、拆分多语句、记录最后一条语句，再发送 `SessionRequest` 并同步等待回执；worker 因而按连接串行执行 session 操作。
5. 普通执行调用 `ConcreteSession::execute`；prepared 执行先由 `prepared_argument` 解码 MySQL binary 参数，再调用 `execute_protocol_statement`。无 record set 时仅返回状态；有结果时选择一次性拉取或注册到 `protocol_result::WorkerResults` 流式读取。
6. 结果列经 resolver/internal-column 转为 `ColumnInfo`，行值经 `protocol_value` 转换；每个结果附加 `ResponseLifecycle`，wire 层完成写出后回调 `FinishProtocolResponse(write_duration)`。
7. `reset_connection` 清理 session 协议状态、killer、取消标志与最后语句；`close` 幂等置位、发送 kill 和 `Shutdown`，最后 `join` worker。

## 数据与状态

transport 的 `alive` 表示最近 I/O/压缩配置是否仍可用，`closed` 保证 close 幂等；TCP 同时保留原始写 socket、内部 cloned reader 和可选的共享 TLS stream。`peer_addr` 可在 PROXY header 成功后被来源地址覆盖，`proxy_protocol_checked` 保证只探测一次。

`ConcreteTiDBContext` 的 `requests` 与 `worker` 都是可取走的 `Option`：关闭后不能再发送请求，也不会重复 join。`state` 缓存最近一次协议状态；`connection_status`、`cancel_requested`、`in_multi_statements`、`closed` 用原子量跨 connection/worker 协作；`compression`、`process_info`、`last_statement` 用互斥锁提供快照。真正的 session、prepared statement 集合、结果游标与内存追踪都留在 worker 线程中，避免让 `ConcreteSession` 跨线程共享。

`SessionRequest` 的普通通道可以排队，单次响应使用 `sync_channel(1)`。流式结果由 `WorkerResults` 注册并以 `ResultOperation` 回到同一 worker，确保 record set 的 next/close 操作仍发生在持有 session 的线程。chunk 大小来自 `tidb_init_chunk_size` / `tidb_max_chunk_size`，缺省分别为 32 / 1024。

## 依赖与调用关系

上游主要是 `cmd/tidb-server/main.rs::assembleCanonicalServer` 与 `pkg/server/server.rs`、`conn.rs` 的连接生命周期。RustCodeGraph 对 `session_driver` 显示生产调用者 `assembleCanonicalServer` / `createServer`，测试调用者包括 `canonical_listener_starts_on_port_zero_and_preserves_cleanup_order`；`pkg/server/runtime_test.rs` 直接覆盖 transport、driver 和 Domain 适配器。

下游关键依赖是 `astersql_session::runtime::{CanonicalSessionFactory, ConcreteSession, SplitSQLStatements}`、`astersql_domain::Domain`、`astersql_server_internal::PacketIO`、`protocol_result::WorkerResults`、`astersql_util_sqlkiller::SQLKiller` 与 `rustls`。`pkg/server/Cargo.toml` 明确声明这些 workspace crate、`rustls = "0.23"`、`chrono` 和 `serde_json`；后两者用于 Domain 状态接口的时间/JSON输出。

重要调用边为：`ConcreteSessionDriver::open_ctx -> thread::Builder::spawn -> run_session_worker -> ConcreteSession::{new, configure_connection, execute/...}`；`ConcreteTiDBContext::execute_query[_streaming] -> SessionRequest::Execute[Streaming] -> execute_on_session`；`execute_prepared_statement/streaming -> SessionRequest::ExecutePrepared[Streaming] -> execute_prepared_on_session`；`TcpPacketIo::read_packet -> proxy_source_addr -> internal::PacketIO::read_packet`。

## 错误处理与边界

socket/codec/rustls/channel 错误统一映射为 `ConnError`；transport 的读写失败会将 `alive` 置为 false。锁中毒使用 `ConnError::Poisoned`，worker panic 在关闭 join 时转换为 session 错误。TCP 关闭对 macOS half-close 的 `NotConnected` 做容错并退化为关闭写侧；TCP/Unix close 都是幂等的。

鉴权当前仅允许 `InsecureRootOnly` 下、native-password 插件、空 auth data 的 `root`；`SecureUnsupported` 必定返回“尚未实现”的 session 错误。`execute_command` 与 `change_user` 明确返回 unsupported，不应在文档或扩展中描述为已支持。

普通执行可按参数拒绝多语句；prepared 参数对固定宽度、UTF-8 和 TIME/DATE 长度做校验，未知类型返回错误；prepared SQL 返回多个 result set 也会拒绝。LOCAL INFILE 只在解析确认为 client file location 时返回路径。`protocol_value` 遇到非法内部 hex 前缀时保留文本，避免误解码。

`CanonicalServerDomain::publish_tiflash_replica_report` 拒绝零总 region 或完成数超过总数，并验证表/库存在。DXF 历史、schema reload 等下游错误转为字符串，这是 `ServerDomain` trait 的边界约定。

## 并发与资源生命周期

每个连接一个 worker 线程；`ConcreteTiDBContext` 可由 server 侧共享，但所有 session 可变操作通过 channel 串行化。`SessionManager` 保存为 `Weak`，避免 driver/domain 与 manager 形成强引用环；设置 manager 时还把它作为 info-schema coordinator 的弱引用交给 Domain。

取消由 `CancellationToken`（请求侧）与共享 `SQLKiller`（session 侧）共同完成；`cancel()` 发出 `QueryInterrupted`，`finish_query_cancellation` / 成功执行路径重置 killer，测试 `query_cancellation_completion_preserves_session_transaction` 证明一次取消收尾不会破坏事务会话。

流式/缓冲结果必须关闭 record set；缓冲路径在读完后显式 `close`，流式路径由 `WorkerResults` 和 `ResponseLifecycle` 管理。连接关闭顺序是置 `closed`、kill、取走 sender 并发送 `Shutdown`、取走并 join worker。socket transport 的 close handle 持有 cloned descriptor，可从其他控制路径主动打断阻塞 I/O。

## 与 Go 版本的对应关系

- `TcpPacketIo` / `UnixPacketIo` 对应 `pkg/server/internal/packetio.go` 与 `conn.go` 中 packet、TLS、PROXY、地址和 socket 生命周期，但 Rust 把能力收敛到 `PacketIo` trait。
- `ConcreteSessionDriver::open_ctx` 与 `ConcreteTiDBContext` 对应 `pkg/server/driver_tidb.go::{TiDBDriver.OpenCtx, TiDBContext}`；Rust 用单连接 worker 解决 canonical session 的线程约束，Go 依靠 goroutine/session 对象直接调用。
- `prepared_argument`、执行/关闭 prepared statement、long-data 计费和结果转换对应 `TiDBStatement` / `TiDBContext` 的协议职责；具体长数据缓存主要仍由 `conn_stmt` 与 session 实现承担，本文件只代理配额快照/计费和执行。
- `CanonicalConnectionDomain` 对应 Go `Server`/`Domain` 的连接 ID 分配释放；`CanonicalServerDomain` 汇聚 Go `server.go` 从 Domain 读取的状态能力。
- Rust 当前 bootstrap 鉴权仅是受限模式，不能等同 Go `conn.go` 的完整用户插件、密码、锁定、过期密码与 change-user 流程。Go 的 `Server.Run` 还负责 listener/status server/goroutine 编排，这些在 Rust 的 `server.rs` 与命令入口，不属于本文件。

## 扩展指南

- 新增 wire transport 能力应实现/扩展 `PacketIo`，并在 `runtime_test.rs` 使用真实回环 socket 验证包序号、half-close、超时和失败后的 `alive`；不要把 framing 再实现一遍。
- 新增 session 操作时，应同时增加 `SessionRequest` 变体、worker match 分支和 `TiDBContext` 门面方法，保证响应通道在所有分支都完成，并评估关闭/取消竞态。
- 新增结果读取方式应保持 record set 只在 worker 线程访问，并接入 `ResponseLifecycle::FinishProtocolResponse`；否则可能破坏统计、资源释放或产生跨线程非安全访问。
- 扩展 prepared 类型需修改 `prepared_argument` / `decode_binary_temporal`，对照 MySQL binary protocol 的有符号性、长度和编码，并在独立的 `pkg/server/runtime_test.rs` 增加畸形包和边界值回归。
- 实现安全鉴权或 change-user 时，应替换 `BootstrapAuthMode` 的明确限制并对齐 Go `conn.go`，不能仅放宽 root 条件；需要覆盖插件协商、用户主机匹配、权限、TLS 状态和 session reset。
- 扩展 Domain status API 应优先在 `CanonicalServerDomain` 代理真实 Domain 能力，保留输入验证与错误映射；涉及 handler 的测试应放在现有独立测试文件，不能内嵌进 `runtime.rs`。

## 验证依据

- 生产源码：`pkg/server/runtime.rs`（2098 行），核对 transport、四个公开适配类型、`SessionRequest`、worker、执行/结果转换及 `TiDBContext` 实现。
- crate/模块：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`；入口接线：`cmd/tidb-server/main.rs`。
- Rust 独立测试：`pkg/server/runtime_test.rs`（1187 行），包含 TCP codec/lifecycle、PROXY 延迟探测、真实 SQL/鉴权、连接 ID、取消、DDL/MDL/Domain 生命周期、LOCAL INFILE、连接事件、Unix 地址及 long-data 配额用例；入口补充测试位于 `cmd/tidb-server/main_test.rs`。
- Go 对照：`pkg/server/internal/packetio.go`、`pkg/server/conn.go`、`pkg/server/driver_tidb.go`、`pkg/server/server.go`。
- RustCodeGraph：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server/runtime.rs` 识别该文件 195 个符号；`explore` 核对 `assembleCanonicalServer -> from_canonical/session_driver` 以及 `session_driver` 的入口与测试调用者，`node --file` 分段核对全部 2098 行实现。
- 本任务是纯文档分析，未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核本文只陈述上述源码、调用图、Cargo、Go 与测试能够支持的事实。
