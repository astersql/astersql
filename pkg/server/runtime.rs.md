# `pkg/server/runtime.rs`

## 文件定位

`TcpPacketIo` / Unix 平台下的 `UnixPacketIo` 把 socket 接到既有 MySQL 包编解码器，处理序号、压缩、超时、关闭、地址、TCP TLS 与 PROXY protocol 延迟探测。

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

下游关键依赖是 `astersql_session::runtime::{CanonicalSessionFactory, ConcreteSession, SplitSQLStatements}`、`astersql_domain::Domain`、`astersql_server_internal::PacketIO`、`protocol_result::WorkerResults`、`astersql_util_sqlkiller::SQLKiller` 与 `rustls`。`pkg/server/Cargo.toml` 明确声明这些 workspace crate、`rustls = "0.23"`、`chrono` 和 `serde_json`；后两者用于 Domain 状态接口的时间/JSON输出。

重要调用边为：`ConcreteSessionDriver::open_ctx -> thread::Builder::spawn -> run_session_worker -> ConcreteSession::{new, configure_connection, execute/...}`；`ConcreteTiDBContext::execute_query[_streaming] -> SessionRequest::Execute[Streaming] -> execute_on_session`；`execute_prepared_statement/streaming -> SessionRequest::ExecutePrepared[Streaming] -> execute_prepared_on_session`；`TcpPacketIo::read_packet -> proxy_source_addr -> internal::PacketIO::read_packet`。

## 错误处理与边界

socket/codec/rustls/channel 错误统一映射为 `ConnError`；transport 的读写失败会将 `alive` 置为 false。锁中毒使用 `ConnError::Poisoned`，worker panic 在关闭 join 时转换为 session 错误。TCP 关闭对 macOS half-close 的 `NotConnected` 做容错并退化为关闭写侧；TCP/Unix close 都是幂等的。

鉴权当前仅允许 `InsecureRootOnly` 下、native-password 插件、空 auth data 的 `root`；`SecureUnsupported` 必定返回“尚未实现”的 session 错误。`execute_command` 与 `change_user` 明确返回 unsupported，不应在文档或扩展中描述为已支持。

普通执行可按参数拒绝多语句；prepared 参数对固定宽度、UTF-8 和 TIME/DATE 长度做校验，未知类型返回错误；prepared SQL 返回多个 result set 也会拒绝。LOCAL INFILE 只在解析确认为 client file location 时返回路径。`protocol_value` 遇到非法内部 hex 前缀时保留文本，避免误解码。

`CanonicalServerDomain::publish_tiflash_replica_report` 拒绝零总 region 或完成数超过总数，并验证表/库存在。DXF 历史、schema reload 等下游错误转为字符串，这是 `ServerDomain` trait 的边界约定。

## 并发与资源生命周期

冻结增量：`TcpPacketIo::connection_alive_probe` 克隆 TCP handle，并返回调用 `astersql_server_internal_util::probe_tcp_stream_alive` 的闭包。只有探针返回 0 才视为死亡，-1 未知状态保守视为存活；克隆失败则返回 None。探针使用非消费的系统调用，不再切换共享 socket 的 nonblocking flag，也不调整读 deadline，可与 packet reader 并发。它探测 TCP 连接而非 TLS 应用层状态；UnixPacketIo 未由此获得 TCP 探针。

每个连接一个 worker 线程；`ConcreteTiDBContext` 可由 server 侧共享，但所有 session 可变操作通过 channel 串行化。`SessionManager` 保存为 `Weak`，避免 driver/domain 与 manager 形成强引用环；设置 manager 时还把它作为 info-schema coordinator 的弱引用交给 Domain。

取消由 `CancellationToken`（请求侧）与共享 `SQLKiller`（session 侧）共同完成；`cancel()` 发出 `QueryInterrupted`，`finish_query_cancellation` / 成功执行路径重置 killer，测试 `query_cancellation_completion_preserves_session_transaction` 证明一次取消收尾不会破坏事务会话。

流式/缓冲结果必须关闭 record set；缓冲路径在读完后显式 `close`，流式路径由 `WorkerResults` 和 `ResponseLifecycle` 管理。连接关闭顺序是置 `closed`、kill、取走 sender 并发送 `Shutdown`、取走并 join worker。socket transport 的 close handle 持有 cloned descriptor，可从其他控制路径主动打断阻塞 I/O。

## 与 Go 版本的对应关系

- `ConcreteSessionDriver::open_ctx` 与 `ConcreteTiDBContext` 对应 `pkg/server/driver_tidb.go::{TiDBDriver.OpenCtx, TiDBContext}`；Rust 用单连接 worker 解决 canonical session 的线程约束，Go 依靠 goroutine/session 对象直接调用。
- `prepared_argument`、执行/关闭 prepared statement、long-data 计费和结果转换对应 `TiDBStatement` / `TiDBContext` 的协议职责；具体长数据缓存主要仍由 `conn_stmt` 与 session 实现承担，本文件只代理配额快照/计费和执行。
- Rust 当前 bootstrap 鉴权仅是受限模式，不能等同 Go `conn.go` 的完整用户插件、密码、锁定、过期密码与 change-user 流程。Go 的 `Server.Run` 还负责 listener/status server/goroutine 编排，这些在 Rust 的 `server.rs` 与命令入口，不属于本文件。

## 扩展指南

- 新增 wire transport 能力应实现/扩展 `PacketIo`，并在 `runtime_test.rs` 使用真实回环 socket 验证包序号、half-close、超时和失败后的 `alive`；不要把 framing 再实现一遍。
- 新增 session 操作时，应同时增加 `SessionRequest` 变体、worker match 分支和 `TiDBContext` 门面方法，保证响应通道在所有分支都完成，并评估关闭/取消竞态。
- 新增结果读取方式应保持 record set 只在 worker 线程访问，并接入 `ResponseLifecycle::FinishProtocolResponse`；否则可能破坏统计、资源释放或产生跨线程非安全访问。
- 扩展 prepared 类型需修改 `prepared_argument` / `decode_binary_temporal`，对照 MySQL binary protocol 的有符号性、长度和编码，并在独立的 `pkg/server/runtime_test.rs` 增加畸形包和边界值回归。
- 实现安全鉴权或 change-user 时，应替换 `BootstrapAuthMode` 的明确限制并对齐 Go `conn.go`，不能仅放宽 root 条件；需要覆盖插件协商、用户主机匹配、权限、TLS 状态和 session reset。
- 扩展 Domain status API 应优先在 `CanonicalServerDomain` 代理真实 Domain 能力，保留输入验证与错误映射；涉及 handler 的测试应放在现有独立测试文件，不能内嵌进 `runtime.rs`。

## 连接方言身份

`ConcreteSessionDriver::open_ctx` 默认 MySQL，并委托显式 open_ctx_with_dialect；后者把身份传入 run_session_worker，再以 ConcreteSession::new_with_dialect 构造连接私有会话。身份在 worker 首条 SQL 前固定，所有查询仍使用共享执行内核。测试专用观测记录真实 worker 会话身份。

## PostgreSQL 转换的拆句门禁（2026-10-24）

真实 listener 在派发 SQL 前的拆句校验使用 SplitSQLStatementsWithDialect；ConcreteTiDBContext 保存建立连接时的不可变身份，普通和流式请求均传入同一身份。原 SplitSQLStatements API 保持 MySQL 默认。此处仍先校验完整 SQL 再按原引号/注释边界拆分，不重写转换或参数。

## PostgreSQL int4 显式转换（2026-10-24）

session_protocol_error 在普通执行、预处理执行与 recordset 收取边界保留 SessionError 中的 PgInt4Error，其他 SessionError 维持原消息桥接。事务失败和恢复继续使用既有 PG 状态机。

## 整数参数元数据接线

Prepare worker 先调用 `ConcreteSession::protocol_integer_parameter_types` 获取连接方言下共享 AST 的直接整数约束，再准备真实 statement；约束放入 `PreparedMetadata.parameter_cast_oids` 返回 PG 消费者。MySQL 连接返回零约束且不改变原有 prepared 执行语义。

### PostgreSQL限定布尔显式转换（任务3）

session_protocol_error优先保留PgBoolError，而不是降级为通用会话文本错误。

## PostgreSQL 有限 numeric 承载

会话 numeric 源错误桥接为 ConnError::PgNumeric，保留22003等分类。


## PostgreSQL float4 文本输入（任务16）

session_protocol_error 保留 PgFloat4Error 到协议连接层；FLOAT4参数继续复用现有f32小端解码。

### 2026-10-26 浮点真实客户端差分（任务20）

session_protocol_error将SessionError中PgFloat8Error源转换为ConnError::PgFloat8。

## 2026-10-26 文本族身份边界（任务21）

Prepare、普通执行与结果元数据三个既有桥接点传递 PostgreSQLTextOid。

### 2026-10-26 varchar 字符长度（任务22）

共享 `pg_varchar` 按 Unicode 标量计数；显式 CAST 裁剪，赋值仅允许额外 U+0020，类型长度域为 1..10485760。沿已有 FieldType 文本身份及错误源传递，保留 MySQL 路径。回归见 `pg_extended_test::pg_scalar_22`、`builtin_cast_test::pg_scalar_22_formal_varchar` 和 `relational_value_test::pg_scalar_22_varchar_relational`。

## UUID显式转换局部接线

将会话PgUuidCastError原样桥接到ConnError::PgUuid，保留22P02/0A000诊断。

## JSON显式转换与类型交接

session_protocol_error把PgJsonCastError映射为ConnError::PgJson，不丢失来源错误身份。


## float4数组与原始binary32位型（任务31）

prepared_argument只对内部标签0xf0/0xf1生成Float4Bits/Float4Array，不改变MySQL原始type4参数。结果复用既有protocol_value字节桥，无float4→f64提升；NativeType继续独立负责描述。

验证与兼容性范围见 `docs/postgresql-pg05-array-float4-evidence.md`；测试位于对应独立测试文件，生产文件不嵌入单元测试。
