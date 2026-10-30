# [`pkg/executor/simple.rs`](./simple.rs)

## 文件定位

管理路径处理刷新/删除统计信息、TLS reload、计划缓存、BDR role、资源组、range placement、会话状态恢复和关闭实例。

## 核心职责

- `SimpleExec::Next` 保证每个执行器实例最多执行一次，并按 `Statement` 变体路由到专用方法；`Binlog` 是明确的空操作。
- `SimpleBackend` 是副作用边界：`Operation` 表示写系统表、事务控制、会话状态变更、通知权限缓存、统计信息操作、KILL、TLS、关机及集群广播；查询、权限和密码策略则通过 trait 的只读/校验方法完成。
- 用户与角色路径负责存在性和特权校验、认证插件选择、口令哈希、双密码限制、密码历史/时间窗口、系统表更新及权限缓存通知。
- 事务路径处理 `BEGIN`、`COMMIT`、`ROLLBACK` 和 savepoint，并对会隐式提交的账户/角色/`FLUSH` 语句先结束旧事务。
- 管理路径处理刷新/删除统计信息、TLS reload、计划缓存、BDR role、资源组、range placement、会话状态恢复和关闭实例。

这不是通用行执行器：接口没有输出 chunk，成功只返回 `()`；它也不解析 SQL，而是消费已归一化的 `Statement`。generic 操作的 IO 和共享状态语义由后端决定；`scheduleProcessShutdown` 是 canonical runtime 直接调用的真实进程信号消费者。

## 主要符号

- `SimpleError`：统一表示后端失败、拒绝访问、非法选项、用户不存在、角色未授予、密码复用和未支持功能。`Display` 当前输出 `Debug` 形式。
- `UserIdentity`、`UserSpec`、`UserRecord`：分别表示 `user@host`、CREATE/ALTER USER 输入和系统表侧用户快照。`UserRecord.password_history` 的元素是“哈希/认证串 + Unix 时间戳”。
- `ResourceOption*`、`PasswordOption*`、`resourceOptionsInfo`、`passwordOrLockOptionsInfo`：把 AST 选项归并到有界数值及“是否显式修改”标志；`NOT_SPECIFIED = -1` 表示回退到全局策略。
- `RoleMode`、`AdminKind`、`StatementScope`：表达角色选择、ADMIN 子命令和 session/instance/global 作用域。
- `Statement`：本文件可接收的语句联合；`Operation`：交给后端执行的副作用联合。二者构成“语句意图 -> 后端动作”的边界。
- `SimpleBackend`：生产接线必须实现的 trait。它要求 `Send + Sync + 'static`，提供执行、查用户、权限/角色边、密码插件、全局策略、时钟、当前用户/角色、事务状态、部署开关、元数据存在性、广播和 warning 汇报。
- `SimpleExec<B>`：保存 `Arc<B>`、当前 `Statement`、`ResolveCtx`、远程来源标志、`done` 与 stale transaction start TS；目前 `ResolveCtx` 未被本文件逻辑读取。
- 用户/角色入口：`executeCreateUser`、`executeAlterUser`、`executeDropUser`、`executeRenameUser`、`executeSetPwd`、`executeGrantRole`、`executeRevokeRole`、`executeSetRole`、`executeSetDefaultRole`。
- 密码策略入口：`getUserPasswordLimit`、`passwordVerification`、`checkPasswordReusePolicy`；后者依次校验、裁剪历史并追加新记录。
- 辅助边界：`userExists*` 处理用户名变体；`renameUserHostInSystemTable` 更新八张 mysql 系统表；`killBySQLStmt`、`killRemoteConn` 和 `broadcast` 只转发到后端。

## 执行流程

1. 调用方构造 `SimpleExec`，注入共享 `Arc<B>` 和一个已归一化的 `Statement`。
2. `Next` 若发现 `done` 已置位则直接成功返回。否则，`autoNewTxn` 对 CREATE/ALTER/DROP/RENAME USER、GRANT/REVOKE ROLE 和 FLUSH 返回 true，先发送 `Operation::Commit`，模拟 MySQL 隐式提交边界。
3. `Next` clone 当前语句并匹配变体，调用对应 `execute*` 方法。无论专用方法成功或失败，分派后都会设置 `done = true`，所以失败也不会在同一实例上重试；但“预提交”失败发生在设置 `done` 之前，后端错误返回后可再次调用。
4. 账户批量变更通常先 `require_privilege`，再发送文本形式的 `BEGIN PESSIMISTIC`。闭包内逐用户查询并锁定、校验和写表；`finish_transaction` 成功时 commit，失败时 best-effort rollback 后保留原错误。事务完成后才发送 `NotifyPrivileges`。
5. CREATE USER 选择默认插件、按需校验明文并哈希；角色的认证串为空。ALTER USER 校验自助修改边界、沙箱模式、双密码与插件兼容性，再检查密码复用策略。SET PASSWORD 对他人账户要求 `UPDATE mysql.user` 权限。
6. 密码复用流程从用户属性或全局值解析条数和天数，按最近 N 条和时间窗口查重；通过后删除过期/超量历史，再追加当前哈希。非 native 插件通过 `check_hashing_password` 比较明文与历史哈希。
7. 角色路径校验用户/角色及授权边，更新 `mysql.role_edges` / `mysql.default_roles` 或当前 active roles，随后通知权限缓存。
8. 事务、统计、KILL、TLS、计划缓存、BDR、资源组、range 和关机路径主要把已校验参数转换成相应 `Operation` 或调用 `broadcast`。

## 数据与状态

- 执行器局部状态：`done` 是一次性消费标志；`staleTxnStartTS` 在 `executeBegin` 中保存传入的 stale TS，缺省为 0；`IsFromRemote` 被 KILL 转换为 `Operation::Kill.remote`。
- 共享状态：`backend: Arc<B>` 允许执行器与外部共享后端。文件本身不持锁；所有系统表、会话、事务和集群状态的一致性由后端实现负责。
- 账户状态：用户名和主机在生成系统表写操作时通常将 host 转小写。`userIdentityToUserList` 只做字符串拼接，并不负责 SQL escaping。
- 选项状态：资源限制钳制到 `0..=i16::MAX`；密码历史/复用值钳制到 `0..=u16::MAX`；显式 password lifetime 必须在 `1..=u16::MAX`，否则报错。锁定天数支持 `-1` 表示 unlimited。
- 密码历史：`getValidTime` 用 `now - days * 86400` 的饱和运算得到下界；`passwordVerification` 为即将插入的新记录预留一行，因此历史已满时会计算至少一条待删除记录。
- `Operation::Sql(String)` 是抽象协议而不是参数化 SQL API；生成的字符串有些是示意性伪 SQL（例如 `INSERT mysql.user USER=...`），其解释完全依赖未来的生产后端。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开 `simple` 模块，并在 `#[cfg(test)]` 下挂载 `simple_test.rs` 与 `simple_internal_test.rs`。RustCodeGraph 能定位 `SimpleExec` 和完整文件，但对常见名称 `Next` 的 callers/callees 发生跨 Go/Rust 同名歧义；结合直接引用搜索，当前未发现 Rust 生产调用者、后端实现或构造点。因此不能把索引显示的“used by 116 files”直接解释为真实运行时调用。

下游方面，本文件直接依赖：

- `std::collections::{BTreeMap, BTreeSet}`：账户属性和 ALL EXCEPT 去重；
- `Arc`、`Duration`：共享后端及关机延迟描述；
- `astersql_parser_ast::AlterUserStmt`：`alterUserHasPrivilegedOptions` 的安全 allowlist；
- `SimpleBackend`：真正的权限、系统表、事务、密码、统计、广播、进程与会话实现。

关键内部调用链包括 `Next -> executeAlterUser -> checkPasswordReusePolicy -> passwordVerification -> {fullRecordCheck, checkPasswordHistoryRule, checkPasswordTimeRule} -> {deleteHistoricalData, addHistoricalData}`，以及 `Next -> executeCreateUser/executeDropUser/executeRenameUser -> finish_transaction -> Operation::{Commit,Rollback}`。`executeRefreshStats` 和 `executeFlushStatsDelta` 通过 `broadcast` 下沉集群行为；`executeKillStmt` 通过 `killBySQLStmt` 下沉连接管理。

## 错误处理与边界

进程关闭的线程创建错误同步返回；运行中信号错误输出上下文并 abort 整个进程，对齐 Go goroutine panic 的进程失败语义（独立 sequence 返回 I/O 错误供断言）。等待秒数及纳秒换算使用 wrapping 算术对齐 Go int/time.Duration 溢出，最终非正的 duration 按 Go Sleep 规则变成零等待。正常退出会由操作系统回收尚在等待的 watchdog；进程未退出则最终 SIGKILL。

- 后端返回的 `SimpleError` 大多用 `?` 原样传播。批量账户事务失败时会尝试 rollback，但 rollback 错误被丢弃，以保留原始业务错误。
- `executeCommit` 是特例：commit 错误只交给 `backend.warn`，`Next` 仍返回成功，这与其他事务动作的传播方式不同。
- `IF NOT EXISTS` / `IF EXISTS` 把重复或缺失用户转换为 warning 并继续处理；没有该修饰时返回 `InvalidOption` 或 `UserNotFound`。
- `skip_grant_table` 会绕过 `require_privilege`，但 `executeUse` 直接调用 `verify_privilege("USE")`，没有走该绕过逻辑。
- `executeSetSessionStates` 仅验证去空白后首尾为花括号，并不真正解析 JSON；解析和恢复由 `Operation::DecodeSessionStates` 后端完成。
- 明确未支持：SET DEFAULT ROLE 的 `Default` / `AllExcept` 模式、非 TLS 的 ALTER INSTANCE、GLOBAL plan-cache flush、ADMIN RELOAD STATISTICS、未知 ALTER RANGE 名称。`Statement::Binlog` 被静默忽略。
- `restoreRefreshStatsSQL` / `restoreFlushStatsDeltaSQL` 只拒绝空字符串，不执行 Go 版本的 AST 恢复和全限定名称检查。
- 生成 SQL 的用户名、主机名、密码/认证串和表/列名大多直接插值；任何生产后端都必须避免把这些字符串直接当作未转义 SQL 执行。

## 并发与资源生命周期

`SimpleBackend: Send + Sync + 'static` 与 `Arc<B>` 允许后端跨线程共享，但 `Next` 需要 `&mut self`，同一执行器实例的 `done` 和 stale TS 不依赖内部锁。generic 执行器不启动任务、不持有通道、不管理响应流或显式锁；其并发与资源关闭由后端契约承担。`scheduleProcessShutdown` 单独持有一个分离的进程 watchdog 线程；进程正常退出时由 OS 回收，否则超时发 SIGKILL，不延长正常退出。

账户修改的生命周期是“旧事务隐式提交 -> 后端开启悲观事务 -> 逐项写入 -> commit/rollback -> 权限通知”。注意 `Next` 的旧事务提交与方法内文本 `BEGIN PESSIMISTIC` 是两个不同阶段，且 `pessimistic_transaction` / `is_starter_deployment` trait 方法当前未被本文件使用。

generic `asyncDelayShutdown` 保留立即发送 `Operation::Shutdown` 的抽象入口。生产 canonical session 直接调用 `scheduleProcessShutdown`，不依赖 generic 后端。该函数创建命名后台线程，以当前进程 PID 运行 `runProcessShutdown`：先等待响应刷新 delay，发送 SIGTERM，随后读取一次全局配置，等待 grace + drain + 10 秒，最后发送 SIGKILL。只有时钟与进程信号在独立测试中替换。类似地，Rust `killRemoteConn` 只设置 `remote: true`，没有像 Go 版本那样创建并消费 DistSQL response，因此本文件本身不存在需要 close 的远程响应资源。

## 与 Go 版本的对应关系

Rust 版本不是逐能力等价的完整接线，主要差异如下：

- Go `SimpleExec` 嵌入真实 `BaseExecutor`、AST 节点、InfoSchema 和 resolve context；Rust 使用自定义 `Statement`、`Operation` 与 trait 后端，generic 后端目前没有生产实现；SQL SHUTDOWN 另由 canonical AST 分发直接接线真实信号消费者。
- Go 的隐式提交走 `sessiontxn.NewTxnInStmt` 并维护 session `InTxn`；Rust只发送 `Operation::Commit`。Go 账户写入使用受限系统会话和真实内部 SQL；Rust生成简化字符串协议。
- Go CREATE/ALTER USER 处理 TLS、资源限制、账户属性、认证令牌等更多字段。Rust虽定义部分 option 辅助函数，但 `Statement::{CreateUser,AlterUser}` 未携带这些集合，主执行路径没有消费 `loadResourceOptions` / `loadOptions`。
- Go KILL 解析 global connection ID、区分本地/远端、产生 warning，并确保消费和关闭远程 response；Rust只把 ID、query 与 remote 标志转发给后端。
- Go REFRESH/FLUSH STATS 根据 AST、当前实例与 cluster-wide 标志执行或广播；Rust的 `Flush.stats_delta` 总是广播 stats delta，`RefreshStats.current_instance=false` 也直接广播。
- Go shutdown 的异步信号序列由 Unix `scheduleProcessShutdown` 对齐；非 Unix 返回 Unsupported。generic 后端抽象仍保留，生产消费者通过 canonical session 直接接入。Go session states 用 JSON decoder 和 `UseNumber`；Rust只做外层花括号检查。
- Go plan-cache flush会检查开关、记录时间戳并清缓存；Rust封装成 `Operation::FlushPlanCache`。Go ALTER RANGE 还验证 placement option 类型并生成 bundle；Rust只验证 range 名和 policy 是否存在。

所以扩展时应以 Go 行为和测试为兼容基线，但必须先决定是扩充当前抽象协议，还是完成真实生产后端接线，不能把已有函数名当作能力已落地的证据。

## 扩展指南

- 新增简单语句时，同时增加 `Statement` 变体、`Next` 分支、必要的 `Operation`/`SimpleBackend` 能力，并评估是否属于 `autoNewTxn` 的隐式提交集合；应在独立 `simple_test.rs` 中验证重复 `Next`、成功和失败分支。
- 扩充 ALTER USER AST 字段时，必须同步 `alterUserHasPrivilegedOptions` 的 allowlist 及 `simple_internal_test.rs` 表驱动用例，否则可能错误允许无特权的自助修改。
- 修改账户、角色或密码流程时，要保持“锁定读取、原子事务、失败回滚、成功后通知”的顺序，并补齐 IF EXISTS/NOT EXISTS、插件变更、双密码、沙箱和历史边界用例。
- 接入生产后端前，应把 `Operation::Sql` 替换为结构化操作或可靠的参数化/转义接口，并明确事务句柄归属；否则存在注入、部分提交和不同后端解释不一致风险。
- 扩展 KILL、广播或 shutdown 时，应对齐 Go 的远程 response close、warning 和进程优雅退出生命周期；扩展统计语句时要保留数据库全限定名与本地/集群作用域。
- 性能风险主要在批量用户操作的逐项查询/写入、密码历史全量装载排序，以及广播操作；兼容风险集中在权限 bypass、warning/error 分类、主机名大小写、认证插件和隐式提交语义。
- Rust 测试必须继续放在独立文件：行为测试优先扩充 `pkg/executor/simple_test.rs`，AST 特权 allowlist 扩充 `pkg/executor/simple_internal_test.rs`，不要把测试内嵌回 `simple.rs`。
