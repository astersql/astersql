# `pkg/server/server.rs`

## 文件定位

`server.rs` 属于 `astersql-server` crate（`pkg/server/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/server/lib.rs::server` 暴露。它是 SQL 服务端的进程级生命周期与连接管理核心：在上游接收 `cmd/tidb-server/main.rs::assembleCanonicalServer` 装配的配置、session driver 和 domain，在下游连接 `conn.rs::ClientConn`、`runtime.rs::{TcpPacketIo, UnixPacketIo}`、`http_status.rs::Server::start_status_http`、PostgreSQL `PgService`、standby 控制器及 `astersql-session-sessmgr`。

该文件不是协议命令状态机本身。MySQL 握手和命令循环由 `ClientConn::{handshake, Run}` 承担，真实 session 由 `SessionDriver` 打开，status 路由由 `http_status.rs` 构造；本文件负责监听、接入、登记、观察、终止和回收这些对象。Go 的直接对照文件是 `pkg/server/server.go`。

## 核心职责

- 校验 `ServerConfig`，构造 SQL TLS/AutoTLS 配置，并管理 MySQL TCP、可选 Unix socket、可选 PostgreSQL 和 status HTTP 监听器。
- 运行 MySQL accept 循环，为每条 socket 创建 `ClientConn` 和独立 worker，区分握手前 `pending_clients` 与握手后 `clients`。
- 实现连接上限、capability、进程/事务/状态变量视图、performance-schema 账户连接汇总、KILL 与正常关闭消息缓存。
- 作为 `SessionManager`、`InfoSchemaCoordinator`、`NormalCloseKiller` 和 `ConnectionServer` 的适配器，把 server 状态提供给 session/DDL/连接层。
- 协调 standby 激活、健康状态、优雅排空、listener/worker/AutoID 服务的幂等关闭，以及 advertised-status 检查器生命周期。
- 解析并校验 PROXY protocol v1/v2 来源地址；把 TLS 和 PROXY 配置交给 `TcpPacketIo`，而不是在 accept 循环中直接实现全部 packet I/O。

## 主要符号

- `DEFAULT_CAPABILITY`：新连接的全局 MySQL capability 基线；`Server::{xor_capability, add_capability}` 原子更新它。TLS 是否启用会在 `ConnectionServer::config` 中动态设置或清除 `CLIENT_SSL` 位。
- `ResourceGroupConnectionCount`：以 `connection_id -> resource_group` 映射实现打开、关闭、迁移与计数；关闭未知 ID 不会产生负计数。当前 `Server` 本体不持有该类型，它是公开的独立计数工具。
- `install_fips_crypto_provider`：仅在 rustls AWS-LC provider 自报 FIPS 时安装，否则失败关闭；已存在另一默认 provider 也返回错误。
- `build_sql_tls_config` / `build_sql_tls_metadata`：前者生成供新 SQL 连接使用的 rustls 配置，后者保留路径和证书有效期元数据；AutoTLS 在临时目录生成 `cert.pem`/`key.pem`。
- `proxy_source_addr` 及 `parse_network`、`read_proxy_v1`、`read_proxy_v2`：验证可信代理网段、限时窥探 header，并按 fallback 策略返回来源地址。
- `StatusConfig` / `ServerConfig` / `TlsConfig`：分别描述 status 服务、SQL listener 与证书元数据；`TlsConfig::verify_peer_common_name` 复用 `astersql_util::security` 的 CA/CN 校验。
- `ProcessInfo` / `TransactionInfo`：server 对外暴露的连接与事务快照。`ManagedConnection for ClientConn` 当前能生成进程信息，但 `transaction_info` 返回 `None`。
- `ManagedConnection`：将 `kill_query` 与 `close` 分开，这是 `KILL QUERY` 不应断开客户端的核心契约；可选提供 `TransactionMDL`。
- `Domain`：status/session-manager 所需的 domain 门面；除 `server_id`、`start_timestamp` 外，多数能力有空值或显式错误默认实现，生产完整度取决于调用方注入的具体实现。
- `NormalCloseCache`：按 `(keyspace, connection_id)` 保存最多 1000 条消息；重复键刷新到队尾，超限淘汰最旧键。
- `Server`：所有共享状态的中心对象；关键入口为 `new/with_standby`、`set_connection_runtime`、`run`、`register_connection`、`kill`、`drain_clients` 和 `close`。

## 执行流程

1. `cmd/tidb-server/main.rs::assembleCanonicalServer` 创建 `Server`，随后在启动前调用 `set_connection_runtime` 注入 canonical `SessionDriver` 与 `ConnectionDomain`；该方法同时把 `Server` 的弱引用注册为 session manager，且运行后禁止替换。
2. `Server::with_standby` 先验证 PROXY 网段与 host，再构造 SQL TLS/AutoTLS，初始化锁、原子状态和缓存，最后触发 `StandbyController::on_server_created`。
3. `Server::run` 用 CAS 保证只启动一次，依次绑定 MySQL/Unix 与 PostgreSQL listener、发布 MPP server 地址、设置 domain、等待并准备 standby 激活，再置健康并启动 status、MySQL、PostgreSQL 服务。任一步失败都会清健康/运行标志、关闭已建立监听并在需要时 join worker。
4. `start_mysql_accept` 仅在 driver/domain 都已安装时启动。`serve_mysql_loop`/Unix 版本以非阻塞 listener 轮询；每个连接经 `TcpPacketIo::new_with_options` 或 `UnixPacketIo::new` 包装后进入 `serve_mysql_connection`。
5. 新 `ClientConn` 先进入 `pending_clients`，连接线程执行握手；失败时关闭并移除，成功时记录 `login_success`、移除 pending，然后执行 `ClientConn::Run`。连接层通过 `ConnectionServer` 回调登记/注销正式连接。
6. `register_connection` 先拒绝 shutdown，再原子获取连接令牌并检查重复 ID；成功后更新用户账户累计值、总接受数、standby 活跃通知与条件变量。`unregister_connection` 只在确实移除时归还令牌并唤醒等待者。
7. `kill` 在 query-only 分支仅调用 `kill_query`；连接终止分支可记录正常关闭消息，随后先 `close` 再 `kill_query`，避免阻塞或已脱离客户端的 SQL 继续执行。找不到普通连接时，`SessionManager::Kill` 转交 domain 的系统进程追踪器。
8. `drain_clients` 先进入 shutdown 并关闭监听，等待 `drain_wait`；仍有连接则取消当前语句，再等待 `cancel_wait`，最后关闭残余连接。`close` 是更彻底且幂等的收尾：通知 standby、关闭监听、join listener worker、关闭 pending/registered 连接、join connection worker、关闭 AutoID 并清 `running`。

## 数据与状态

`Server` 由 `Arc` 共享。监听器、worker handle、缓存、内部会话和账户汇总使用 `Mutex`；domain、连接表、pending 表、运行时依赖与 TLS 快照使用 `RwLock`。连接归零等待由 `clients_wait_lock + clients_changed` 实现，所有等待都在循环中重新检查 `connection_count`，可抵抗伪唤醒。

`running` 防止重复 `run`，`shutdown_mode` 阻止新登记并驱动 accept 循环退出，`health` 控制对外健康与 status accept，`close_started` 保证完整关闭只执行一次。`force_shutdown` 与 `need_request_manager_free` 只是可观察标志，设置它们不会自动进入 shutdown；独立测试 `shutdown_flags_do_not_start_server_shutdown` 固化了这一边界。

有连接上限时，`active_tokens` 通过 `fetch_update` 原子获取/释放；`max_connections == 0` 表示无限制。`accepted_connections` 是累计成功登记次数，不是当前连接数，也被用于生成握手 salt 的序列输入。握手前连接不占 `clients`，而由 `pending_clients` 单独保存，关闭时两类都必须处理。

`session_tls_config` 决定新 SQL 连接真正使用的 rustls 配置，`tls_config` 是状态/证书元数据视图。`reload_tls` 成功时同时替换两者；普通失败保留最后可用配置，只有请求 `NO ROLLBACK ON ERROR` 且未强制 secure transport 时才清空 TLS。

## 依赖与调用关系

生产上游调用链为 `cmd/tidb-server/main.rs::assembleCanonicalServer -> Server::new -> set_connection_runtime`，随后 `cmd/tidb-server/stubs.rs::Server::Run -> canonical Server::run`；退出链为其 `DrainClients -> Server::{drain_clients, close}`。测试工具 `pkg/server/tests/servertestkit/testkit.rs` 也以同一方式启动真实 server。

主要下游边为 `run -> init_tidb_listener/init_postgres_listener -> standby::{wait_for_activate, prepare_for_activation} -> start_status_http/start_mysql_accept/start_postgres_accept`；`serve_mysql_connection -> newClientConn -> ClientConn::{handshake, Run}`；`ConnectionServer::config/driver/domain` 为 `conn.rs` 提供协议配置与 session 依赖；`SessionManager` 实现把进程、KILL、TLS 和内部会话能力交给 `astersql-session-sessmgr`。

`pkg/server/http_status.rs` 在独立 impl 中实现 `Server::start_status_http`，并回调本文件的 `set_status_listener`/`set_status_worker`。`pkg/server/runtime.rs::TcpPacketIo` 调用本文件的 `proxy_source_addr`。`pkg/server/standby.rs` 通过 `StandbyReadyServer` 和 `StandbyShutdownServer` 使用 listener 初始化、健康、排空等待与 AutoID/关闭标志。

`pkg/server/Cargo.toml` 声明本文件直接需要的 `astersql-executor-mppcoordmanager`、server handler/runtime 子 crate、`astersql-session-sessmgr`、`astersql-infoschema`、`astersql-parser-mysql`、`astersql-util`、`serde_json` 和 `rustls = "0.23"`；crate 本身没有 feature 表，平台差异只由源码中的 `cfg(unix)` 控制 Unix socket。

## 错误处理与边界

构造期拒绝空 host、非法 PROXY CIDR、缺少成对 SQL 证书/私钥、AutoTLS 缺少临时目录，以及证书加载/生成失败。`init_tidb_listener` 幂等；Unix socket 路径只会删除已存在的 socket，若是普通文件则拒绝覆盖，非 Unix 平台配置 socket 会明确报错。

PROXY v1 header 最长 108 字节且只接受 TCP4/TCP6 六字段形式；v2 校验签名、版本、长度及 IPv4/IPv6 stream family。启用代理但 peer 不在白名单时拒绝；没有 header 时仅在 `fallbackable` 为真时回退到真实 peer。读取结束后尽力清除 read timeout。

accept 循环把单连接 transport 构造、握手及线程创建失败隔离到该连接，不令整个 server 崩溃；listener 的非 `WouldBlock` accept 错误会结束相应循环。锁中毒多使用 `expect`，因此这是进程内不变量而非可恢复错误通道。

当前迁移边界必须如实看待：`ManagedConnection for ClientConn::transaction_info` 返回 `None`，`update_cpu_time` 是空实现，`connection_active` 为空；内部 session 经 `InfoSchemaCoordinator::StoreInternalSession` 登记的 `start_ts` 为 0。`Domain` 的 schema/global-variable/TiFlash/DXF 默认方法也可能返回未配置错误或空值，只有具体 domain 适配器覆盖后才可视为支持。

## 并发与资源生命周期

listener worker 和每连接 worker 都是命名 OS 线程。listener 被设为非阻塞，通过 10ms sleep 处理 `WouldBlock`；连接 socket 随后切回阻塞模式，由每连接线程运行握手与命令循环。`connection_workers` 暂存全部 join handle，直到 `close` 统一回收。

关闭顺序有意先置 shutdown/不健康并关闭 listener，使新连接停止进入，再 join accept/status worker；之后关闭 pending 握手、终止已登记连接并等待所有 connection worker，最后关闭 AutoID。`close_started.swap` 防止重复 join/关闭，`Drop` 只提供最后的监听器与 PostgreSQL service 兜底，不替代显式 `close` 的连接/worker 收敛。

涉及连接回调时先在锁内复制 `Arc` 列表，再在锁外调用 `close`/`kill_query`，避免外部实现回调重入 `Server` 时持有 clients 锁。`register_connection` 在更新完映射后主动释放写锁才通知 standby 和条件变量。正常关闭缓存、账户累计和内部 session 各自使用独立锁，避免与高频连接表共享同一临界区。

## 与 Go 版本的对应关系

- `Server`、`NewServer/Run/Close`、listener 与连接表直接对应 `pkg/server/server.go::Server` 及其同名方法；Rust 把 Go 的 goroutine/wait group 改为显式 `JoinHandle`，并把 MySQL、PostgreSQL、status listener 的地址分别保存。
- `ManagedConnection` 与 `SessionManager` 实现对应 Go `clientConn` 和 server 的 process/KILL/TLS 接口。Rust 明确拆分 `kill_query` 与 `close`；非 query KILL 仍会二者都调用，对齐 Go `kill` 最终总会执行 `killQuery` 的行为。
- `drain_clients` 保留“自然退出、取消执行、关闭残余连接”的三阶段意图，但 Rust 用 `Condvar` 等待连接表归零；Go 版还等待每个 session 的 commit wait group，并检查连接状态/事务，这部分在 Rust 本文件中尚无等价逻辑。
- Go `ShowTxnList` 从真实 session 读取事务，`UpdateProcessCPUTime` 合并 SQL CPU，内部 session start TS 会排除 auto-analyze；Rust 的对应路径目前分别为空事务、CPU no-op 和登记值（协调接口写入 0），不能宣称完全迁移。
- Go `KillSysProcesses` 直接操作 domain 的系统进程追踪器；Rust `kill_system_processes` 只关闭 `clients` 中标记为 system 的连接，而找不到 ID 的单次 `SessionManager::Kill` 才调用 `Domain::kill_system_process`。
- Go server 还包含 audit plugin、extension 回调、指标、认证 token、gRPC/status 等更宽职责；Rust 将部分能力拆到 `conn.rs`、`http_status.rs`、`rpc_server.rs`，部分尚未在此文件实现。Rust 额外提供独立 PostgreSQL listener。

## 扩展指南

- 新增 listener 或启动阶段时，应接入 `run` 的失败回滚、`enter_shutdown_mode/close_listeners` 和 `join_listener_workers` 三处，并为地址清理、端口占用及重复关闭在独立 `*_test.rs` 中增加回归。
- 扩展连接状态必须保持 pending 与 registered 的边界：只有握手成功并由连接层登记后才计入上限和 process list；所有失败路径都要关闭 transport、移除 pending 并可被 `close` join。
- 修改 KILL/排空时必须保持 query-only 不断链、KILL CONNECTION 同时取消 SQL，以及外部回调在 clients 锁外执行；若补齐 Go 的事务/commit wait 语义，应在 `pkg/server/server_test.rs` 添加明确的竞态与超时测试。
- TLS/PROXY 修改应分别落在 `build_sql_tls_*`、`reload_tls`、`proxy_source_addr` 与 `runtime.rs::TcpPacketIo` 接线处，并同步 `pkg/server/tests/tls/tls_test.rs`、`runtime_test.rs` 的证书、畸形 header、fallback 和 secure-transport 用例。
- 扩展 `Domain`/`SessionManager` 时不要依赖默认空实现来伪造成功；生产 `CanonicalServerDomain` 必须提供真实能力，并补独立测试验证错误传播。实现事务/CPU/internal start TS 时应对照 Go `server.go` 而不是只让 trait 编译通过。
- 测试逻辑继续放在 `pkg/server/server_test.rs` 或对应独立集成测试，不应内嵌进本生产文件；若改变 HTTP API，还必须遵循 `pkg/server/AGENTS.md` 同步 `docs/tidb_http_api.md`。

## 验证依据

- 生产源码：RustCodeGraph `node --file pkg/server/server.rs` 分段核对全部 2432 行、217 个符号；重点检查配置/TLS/PROXY、`Server` 字段与全部 impl、连接接入、排空和 trait 适配。
- 调用图与入口：RustCodeGraph `status` 显示索引含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/server/server.rs` 确认目标文件；`query` 精确定位 `register_connection`、`drain_clients`、`proxy_source_addr`、`reload_tls`。`callers` 查询未在限定时间返回，因此生产上游又由 `rg` 核对 `cmd/tidb-server/main.rs`、`cmd/tidb-server/stubs.rs`、`pkg/server/http_status.rs` 和 `pkg/server/runtime.rs` 的真实调用点。
- crate 与模块：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`；路径级约束：`pkg/server/AGENTS.md`。
- Go 对照：`pkg/server/server.go`，核对 `Server`、`NewServer`、`Run`、`Close`、连接登记、KILL、process/txn、TLS、排空、内部 session 与 AutoID 行为。
- Rust 独立测试：`pkg/server/server_test.rs`，覆盖真实 MySQL 握手/查询/prepared cursor/processlist、连接上限、账户汇总、KILL、capability、关闭标志、排空、PostgreSQL 生命周期及连接事件；补充入口/运行时/TLS/standby 测试分别位于 `cmd/tidb-server/main_test.rs`、`pkg/server/runtime_test.rs`、`pkg/server/tests/tls/tls_test.rs`、`pkg/server/tests/standby/standby_test.rs`。
- 本任务仅生成文档，按计划未运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工检查本文没有把上述已确认的迁移缺口写成已支持能力。
