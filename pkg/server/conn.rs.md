# `pkg/server/conn.rs`

## 文件定位

`pkg/server/conn.rs` 是 `astersql-server` crate 的 MySQL 单连接协议状态机。crate 根模块在 `pkg/server/lib.rs` 中以 `pub mod conn` 暴露它；真实监听入口位于 `pkg/server/server.rs::Server::serve_mysql_connection`，该入口接受 TCP/Unix socket 后调用 `newClientConn`，并在专用连接线程中依次执行 `ClientConn::handshake` 和 `ClientConn::Run`。

这个文件处在“网络包 IO”与“SQL 会话执行”之间：向上由 `ConnectionServer`、`SessionDriver` 和 `ConnectionDomain` 接入服务端注册表、会话驱动与连接 ID；向下通过 `PacketIo` 操作 MySQL 包，通过 `TiDBContext` 完成鉴权、切库、普通查询和预处理语句执行。它不实现 SQL 规划器或存储引擎，也不实现真实 TCP；生产适配分别在会话实现和 `pkg/server/runtime.rs`/`pkg/server/server.rs` 中。

`pkg/server/Cargo.toml` 将本目录声明为 `astersql-server`（`[lib] path = "lib.rs"`），并直接依赖解析器、AST、会话管理、变量、信息模式、指标、日志以及 `pkg/server` 的内部解析/错误子 crate。该文件当前没有 feature 条件；只有若干故障注入/观测辅助函数受 `#[cfg(test)]` 控制。

## 核心职责

- 维护一个客户端连接从分配 ID、握手、鉴权、注册、命令循环到注销和资源释放的完整生命周期，核心对象是 `ClientConn`。
- 实现 MySQL 4.1 握手响应解析、TLS 升级、认证插件切换、压缩协商和 `init_connect`；主要入口是 `handshake`、`readOptionalSSLRequestAndHandshakeResponse`、`handleAuthPlugin`、`openSessionAndDoAuth`。
- 将 MySQL `COM_*` 操作码映射为 `Command`，由 `dispatch` 路由到普通查询、切库、字段列表、刷新、统计、健康探测、换用户、连接重置和 `COM_STMT_*` 路径。
- 把会话返回的 `QueryResult` 编码为 MySQL 文本或二进制协议，包括列定义、NULL 位图、长度编码整数、日期/时间值、OK/EOF/ERR 包和流式结果集。
- 协调取消、连接存活探针、优雅停机、游标、长参数内存、响应写耗时以及预处理语句资源回收。
- 在连接边界把内部错误转换为 MySQL 错误码/SQLSTATE（`mysql_error_code_and_state`），避免把会话内部类型泄漏给协议层。

## 主要符号

- `ConnError` / `ConnResult<T>`：连接层统一错误模型。显式区分 IO、坏包、不支持协议/命令、安全传输要求、鉴权拒绝、服务器关闭、包过大、结果不确定、客户端退出、会话错误和锁中毒。
- `CompressionAlgorithm`、`TlsState`、`ServerConfig`、`AuthIdentity`、`AuthRequest`、`HandshakeResponse`：握手和鉴权所需的协议/配置快照。
- `SessionState`、`Value`、`ColumnInfo`、`NativeType`、`QueryResult`、`PreparedMetadata`：会话结果到 MySQL 线协议之间的拥有型数据桥。`QueryResult` 可持有内存行，也可持有 `ProtocolResultSet` 流式源。
- `ResponseLifecycle` / `WriteSQLResponseTimer`：累计真正的协议写耗时，并用原子标志保证完成回调最多执行一次；遗漏显式 `finish` 时由 `Drop` 兜底。
- `Command`：MySQL 命令字节枚举；`TryFrom<u8>` 对未知操作码返回 `UnsupportedCommand`。
- `CancellationToken`：单调的原子取消位；`cancel` 后不会复位。
- `PacketIo`：包读写、刷新、序号复位、超时、压缩、TLS、地址、存活探测和关闭的传输抽象。`PacketCloseHandle` 允许不获取包锁直接关闭底层传输。
- `TiDBContext`：协议层所需的会话契约，覆盖鉴权、SQL/预处理执行、字段列表、切库、取消、状态与进程信息；许多默认方法使适配器可逐步实现可选能力。
- `SessionDriver`、`ConnectionDomain`、`ConnectionServer`：分别注入会话创建、连接 ID 生命周期和服务端注册/健康/配置能力。
- `ClientConn`（别名 `clientConn`）：主状态机。重要字段包括原子 `capability`/`connection_id`/`status`/`closed`/`registered`，锁保护的身份、TLS、会话、当前取消令牌、预处理语句和最近包，以及无需包锁的 `packet_close`。
- `newClientConn`：取得连接 ID、读取服务配置并构造未握手连接。
- `parse_handshake_response` 与 `read_*`/`take_bytes`/`put_lenenc_*`：有界解析和生成 HandshakeResponse41、NUL 字符串及长度编码整数。

## 执行流程

1. `Server::serve_mysql_connection` 创建真实 `PacketIo`，调用 `newClientConn`，先把连接放入 `pending_clients`，再启动名为 `astersql-mysql-<id>` 的线程。
2. `ClientConn::handshake` 写初始握手包；读取首个客户端包，若是 32 字节 SSLRequest 则升级 TLS 后再读完整响应；`parse_handshake_response` 校验 `CLIENT_PROTOCOL_41` 并解析用户、认证数据、数据库、插件、属性和 zstd 级别。
3. 握手根据用户实际插件决定是否执行 `authSwitchRequest`；SHA2/SM3 路径可请求完整密码；随后 `openSessionAndDoAuth` 创建 `TiDBContext`、提交 `AuthRequest`，并在请求指定数据库时切库。管理员跳过 `init_connect`。
4. 成功后发送 OK，启用协商出的 zlib/zstd 压缩，复位包序号，再注册到服务端连接表。注册失败会撤销 `registered` 标志并返回错误。
5. `Run` 进入 `run_loop`。每轮先检查服务端停机状态：非事务连接直接退出，事务连接可继续；然后用 CAS 将状态从 `connStatusDispatching` 切到 `connStatusReading`，设置会话 `wait_timeout` 并读包，读完再 CAS 回分发态。
6. `dispatch` 解析第一个字节为 `Command`，保存最近包，建立本轮取消令牌和进程信息，然后路由。`Query` 进入 `handleQuery`；`StmtPrepare/Execute/SendLongData/Close/Reset/Fetch` 进入 `handleStmt`；`Quit` 产生内部 `ClientQuit` 终止信号。
7. `handleQuery` 对需要 SQLKiller 存活检查的语句临时安装无阻塞探针；LOCAL INFILE 先发 `0xfb + path` 并收集数据，否则调用 `execute_query_streaming`。多结果集为前序结果加 `SERVER_MORE_RESULTS_EXISTS`，逐个编码、刷新并结束响应生命周期。
8. `handleStmt` 缓存预处理元数据与参数状态。执行时解析绑定参数；游标模式先写元数据并保存 `QueryResult`，`StmtFetch` 每次最多取 1024 行，耗尽时写 `LAST_ROW_SENT`、关闭源并清除游标。长参数通过 `LongDataState` 计费，在执行或关闭时释放。
9. `write_result_chunks` 保证流式源的 `next_chunk` 在写计时区间之外调用；首次写元数据，随后编码行，最终调用 `finish`、写 EOF 并调用 `close`。无列结果直接写 OK。
10. 普通分发错误由 `run_loop` 写 ERR 包后继续；`ClientQuit` 与 `ResultUndetermined` 直接终止。`Run` 最终总会调用 `Close`，把客户端正常退出转换为成功返回。

## 数据与状态

连接状态只允许围绕 `connStatusDispatching`、`connStatusReading`、`connStatusShutdown`、`connStatusWaitShutdown` 演进。读包前后使用 `CompareAndSwapStatus`，因此并发 kill/优雅停机抢先改变状态时，连接线程不会继续执行新命令。`setStatus` 同时把状态同步到 `TiDBContext`。

握手后固定保存协商能力位、排序规则、用户名、默认库、连接属性、认证插件和 TLS 摘要。能力位决定压缩算法、多语句、会话跟踪、EOF 废弃、连接属性、认证数据编码等协议分支。连接属性总长硬限制为 1 MiB；边界由 `MAX_CONNECTION_ATTRIBUTES_SIZE` 和 `parse_handshake_response` 强制执行。

预处理语句由两个 `Mutex<HashMap<...>>` 维护：`prepared_statements` 保存参数、长数据与游标，`prepared_columns` 保存原始列元数据。`StmtClose` 和连接关闭负责释放长数据；换用户成功后清空两张表，避免跨身份复用语句。

`ResponseLifecycle` 用 `Mutex<Duration>` 累计写耗时，以 `AtomicBool` 防止完成回调重复触发。`ProtocolResultSet` 的迭代器/块留在拥有线程，协议层只消费其拥有型行，避免跨线程借用会话内部游标。

## 依赖与调用关系

上游主链为：`pkg/server/server.rs::Server::serve_mysql_connection` → `newClientConn` → `ClientConn::handshake` → `ClientConn::Run` → `run_loop` → `dispatch`。同一 `server.rs` 还通过 `ManagedConnection for ClientConn` 读取进程信息/连接属性/压缩状态，并把 kill 与 close 分别委托给 `cancelDispatch` 和 `Close`；`ConnectionServer for Server` 则实现注册、注销、健康状态、配置、驱动和 Domain 访问。

下游主链为：`dispatch` → `TiDBContext::{use_db, execute_query_streaming, prepare_statement, execute_prepared_streaming, execute_command, change_user, reset_connection}`。预处理参数解析和长数据状态来自 `pkg/server/conn_stmt.rs`；包层由 `PacketIo` 隔离，生产实现由 `pkg/server/runtime.rs` 提供。

语法级依赖包括 `astersql_parser::Parser` 与 `astersql_parser_ast`：`should_install_connection_alive` 按真实 AST 区分普通语句、EXPLAIN/TRACE、BRIE、DDL、提交回滚等，决定是否把传输存活探针安装到会话。协议错误文本引用 `astersql_server_err`；日志、指标和会话变量分别来自 `astersql_util_logutil`、`astersql_metrics`、`astersql_sessionctx_vardef`。

RustCodeGraph 索引显示该文件直接被 `pkg/server/runtime.rs`、`pkg/server/conn_test.rs`、`pkg/executor/import_into_test.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs` 和 `pkg/session/runtime/system_session.rs` 使用。精确 `callers/callees` 查询在本次环境中超时；上述生产调用边另由已索引的 `pkg/server/server.rs` 源码核实。

## 错误处理与边界

所有包读取都通过切片边界检查返回 `MalformedPacket`，不依赖 panic；长度相加使用 `checked_add`。握手拒绝不支持 Protocol 4.1 的客户端，按配置强制 TLS/Unix socket 安全传输，并拒绝超过 1 MiB 的连接属性。盐少于 20 字节、认证插件未知、auth_socket 用于非 Unix 连接也会立即失败。

`mysql_error_code_and_state` 将常见执行错误文本映射到 MySQL 错误码和 SQLSTATE，例如重复键 1062/23000、外键 1451/1452、语法错误 1064、取消 1317、包过大 1153/08S01、IMPORT INTO 预检查 8173；未知会话错误回落到 1105/HY000。这里依赖规范化错误文本，是扩展错误类型时容易遗漏的兼容边界。

`run_loop` 区分可回复错误和必须断连错误：一般命令错误写 ERR 后继续；客户端退出和结果不确定不再回复。锁中毒统一转为 `ConnError::Poisoned`。`handleChangeUser` 在新身份认证完成前保留旧会话，任何解析、插件交换或认证失败都会关闭临时新会话并恢复用户、数据库、插件和旧上下文；只有成功后才关闭旧会话、迁移资源组计数并清空预处理缓存。

值得注意的当前边界是：`writeStats` 只返回简化统计；`dispatch` 对 CreateDb、DropDb、ProcessInfo、Connect、ProcessKill、Debug 等枚举值仍返回不支持；默认 `TiDBContext::execute_local_infile` 也明确返回未支持。文档不能把 Go 文件已具备的更完整能力视为 Rust 已支持。

## 并发与资源生命周期

每个连接由 `server.rs` 创建独立 OS 线程。`ClientConn` 通过 `Arc` 在服务端注册表、工作线程和管理接口间共享；可变协议状态用原子或 `Mutex`/`RwLock` 保护。`packet` 锁串行化所有读写，但 `packet_close` 刻意绕开该锁，使关闭线程能打断正阻塞于客户端读取的传输。

关闭顺序由 `Close` 固定且幂等：`closed.swap(true)` 防重入；先把 ID 置零并从活动连接表注销，使性能模式与优雅停机立刻停止观察它；随后调用独立传输关闭句柄、标记 shutdown、取消当前命令、归还 Domain 连接 ID，再关闭 `PacketIo`、释放所有长参数、关闭会话并清空上下文。传输错误与会话错误通过 `packet_result.and(session_result)` 合并，前者失败时仍已执行后续清理。

查询存活探针只为部分可中断语句临时安装，执行结束后清除；`current_cancel` 每次分发替换，管理线程可经 `cancelDispatch` 同时设置令牌和通知会话。响应生命周期在正常、编码错误和 drop 路径上都能结束，但新增提前返回点仍应确保 `ResponseLifecycle::finish` 或其 `Drop` 兜底可达。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/conn.go`。Rust 保留了 Go 的主要命名和顺序：`newClientConn`/`clientConn`、`handshake`、`Run`、`dispatch`、`handleQuery`、`handleStmt`、`handleChangeUser`、`writeResultSet`、`writeError`、`Close`/`closeConn`，并保留 dispatching ↔ reading 的 CAS、非事务停机退出、认证插件切换、压缩优先 zlib、重入关闭以及换用户失败回滚旧会话等关键语义。

实现形态存在有意的 Rust 化：Go 的具体 `Server`、`TiDBContext`、`PacketIO` 被三个服务 trait、会话 trait 和 `PacketIo` 注入；Go 的 `sync.Once` 对应 `AtomicBool::swap`；Go context cancel 对应 `CancellationToken`；Go 的结果集接口被 `ProtocolResultSet` 和拥有型 `Value` 桥接；锁中毒成为显式错误。

Rust 当前并非 Go `conn.go` 的完全等价实现。Go `Run`/`dispatch` 还包含 panic recovery、审计/扩展插件、trace/pprof/TopSQL、服务端并发 token、更多指标与日志脱敏、分配器复用和更细的错误分类；Go `handleQuery` 内部完成解析、多语句策略、点查预取、事务错误动作与 TiFlash 回退，而 Rust 把大部分执行策略委托给 `TiDBContext::execute_query_streaming`。新增功能必须先判断应落在协议状态机还是会话适配器，不应仅因 Go 逻辑位于 `conn.go` 就复制进本文件。

## 扩展指南

- 新增或支持一个 `COM_*` 命令：同步修改 `Command`、`TryFrom<u8>` 与 `dispatch`，明确进程信息、取消、是否回复/flush、错误后连接是否可复用，并在独立的 `pkg/server/conn_test.rs` 或更聚焦的协议测试文件中覆盖未知/短包和正常包；不要把测试内嵌进 `conn.rs`。
- 扩展握手能力或认证插件：修改能力常量、`HandshakeResponse`/`parse_handshake_response`、`handleAuthPlugin` 和必要的 `AuthRequest` 字段；同步验证 TLS 前后包序号、老客户端分支、连接属性上限及 Go 插件映射。安全传输和 auth_socket 限制不能被绕过。
- 增加结果类型：优先扩展 `ColumnInfo`/`NativeType` 与 `encode_binary_column`，同时验证文本/二进制两种编码、NULL 位图、signed/unsigned 和日期时间小数秒；协议元数据截断不能污染原生类型。
- 修改流式结果或游标：保持 `next_chunk`/`current_row` 不计入协议写耗时，确保 `finish`、`on_fetch_returned`、`close` 和 `ResponseLifecycle::finish` 的顺序与错误路径完整；同步检查 `pkg/server/conn_stmt.rs`。
- 修改关闭或换用户：维护“先从注册表消失，再关闭可能阻塞的资源”和“新认证成功前旧会话仍可用”两个不变量；测试幂等关闭、并发取消、认证失败恢复、预处理/长参数释放和资源组计数。
- 新增错误：更新 `ConnError::Display` 与 `mysql_error_code_and_state`，用独立测试断言 ERR header、数值错误码、`#`、5 字节 SQLSTATE 和消息；避免只做字符串包含匹配而没有协议断言。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server` 确认 `conn.rs` 有 323 个符号；`node --file pkg/server/conn.rs` 分段读取了全部 1–2835 行，并报告其 5 个直接使用文件。
- 生产源码：`pkg/server/conn.rs`；模块与入口：`pkg/server/lib.rs`、`pkg/server/server.rs`（`serve_mysql_connection`、`ManagedConnection for ClientConn`、`ConnectionServer for Server`）；crate 边界：`pkg/server/Cargo.toml`。
- Go 对照：`pkg/server/conn.go` 中的 `newClientConn`、`clientConn`、`handshake`、`Run`、`dispatch`、`handleQuery`、`handleChangeUser` 和 `Close`/`closeConn`。
- Rust 独立测试：`pkg/server/conn_test.rs` 覆盖命令字节、文本值编码、取消单调性、存活探针分类与断连检测、认证插件、握手属性截断/1 MiB 上限、换用户成功和三类失败回滚、长参数包上限及 MySQL 1153/08S01、IMPORT INTO 8173/HY000。
- 人工复核限制：RustCodeGraph 的精确 `callers/callees` 命令本次在 30 秒内未返回；调用关系改由其文件使用关系和已索引生产入口源码交叉确认。本任务是纯文档分析，按计划未运行 Cargo。
