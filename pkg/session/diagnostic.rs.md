# `pkg/session/diagnostic.rs`

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准。

## 文件定位

本文件属于 `astersql-session`，由 `pkg/session/lib.rs` 的 `pub mod diagnostic` 装配；函数本身为 crate 内可见。它提供既有存储的诊断启动入口，与正常 bootstrap/upgrade 分离。相邻 `diagnostic_sql.rs` 负责客户端 SQL allowlist，两者不能互相替代。

## 核心职责

在已经 bootstrap 的 Domain 上建立 ConcreteSession，验证持久化版本，只读载入查询所需元数据，再启动诊断 Domain 服务和统计读取。入口不创建或升级系统表，不推进 bootstrap version；新增普通启动步骤不能自动落入此路径。

## 主要符号

唯一函数 `bootstrap_session_impl_diagnostic(domain: Arc<Domain>) -> SessionResult<ConcreteSession>` 接受已有 Domain 的共享所有权，返回可查询的具体会话。依赖 `ConcreteSession`、crate 内重导出的 `ValidateDiagnosticBootstrap`、`SessionError` 和 `SessionResult`，没有本地配置、全局状态或独立 trait。

## 执行流程

1. `ConcreteSession::new(Arc::clone(&domain))` 构造会话。
2. `ValidateDiagnosticBootstrap(&session, &domain)` 检查已有状态；失败立即返回。
3. `session.load_diagnostic_metadata_readers()` 载入查询缓存。
4. `domain.start_diagnostic()` 启动诊断服务。
5. `domain.load_stats_diagnostic()` 载入诊断统计，成功后返回 session。

`CanonicalSessionFactory` 的存储启动路径在 `runtime/session.rs` 检查 `diagnosticmode::Enabled()`；为真时调用本入口并提前返回 factory，避开其后普通启动流程。factory 使用本函数的初始化副作用，不保存该返回会话。

## 数据与状态

本文件不保存版本号；校验函数读取 KV bootstrap version、SQL bootstrapped 标记及 mysql.tidb 中 SQL 版本。KV 必须大于零、SQL 已 bootstrap，且两处版本相等；不要求等于当前 320，因此旧而一致的版本可供诊断，不能借此执行升级。

metadata reader 在相邻实现中读取权限表并合并进程内 privilege handle（skip_grant_table 时跳过），读取未删除 binding 并恢复全局 binding 缓存。缺少可解析表时对应扫描可为空；错误仍会传播。读入内存不等于更新持久化系统表，也不代表 Go 所有缓存刷新循环已经一一移植。

## 依赖与调用关系

`pkg/session/Cargo.toml` 声明包名与 `lib.rs` 入口；本文件直接依赖 `astersql_domain::Domain`。已接线的链为 `runtime/session.rs` 的 factory 存储启动 → 本入口 → `ValidateDiagnosticBootstrap` / metadata readers → Domain 的诊断启动与统计 API。`runtime.rs` 只将校验函数 crate 内重导出，没有将其变成 crate 外公共 API。

客户端 SQL 拦截另由 `runtime/dispatch.rs` 调用 `diagnostic_sql::is_diagnostic_sql_allowed`，不是本函数逐句校验。

## 错误处理与边界

构造之后所有可失败操作按顺序 `?` 返回；Domain 启动错误包装为 `start diagnostic Domain: ...`，统计错误包装为 `load diagnostic statistics: ...`。metadata 与版本错误保留相邻实现的上下文，例如未 bootstrap、SQL 版本缺失或 SQL/KV 不一致。

此函数没有显式 `domain.close()`、错误回滚或部分服务撤销。若统计加载在启动服务后失败，调用方仍必须管理 Domain 资源，不能把返回 Err 当作全部服务已关闭的证据。它没有 context/cancellation 参数，也没有自建取消或重试循环。

## 并发与资源生命周期

Domain 通过 Arc 共享，会话也持有其引用；本文件不创建线程、锁或通道。后台任务的启动及停止职责留在 Domain 的诊断 API 和 close 路径。会话局部值在错误返回时按 Rust 所有权规则释放，但这不能替代显式关闭仍有共享引用的 Domain。

## 与 Go 版本的对应关系

对应 `pkg/session/diagnostic.go::bootstrapSessionImplDiagnostic` 的独立诊断启动意图。Go 接受 context/store，初始化 schema cache、MDL、全局变量，再取得 Domain，创建三条 restricted session，装配 Domain on-close 清理，启动 privilege/sysvar/binding 循环、blacklist 和统计读取。

Rust 接受已创建的 Domain，用具体 metadata reader 与 Domain API 编排，并额外显式校验 SQL/KV 版本一致；本文件没有 Go 三会话数组、defer 出错关闭 Domain 或全部初始化调用。不能仅凭入口名称宣称两边资源清理及后台服务完全等价。

## 扩展指南

诊断启动新增服务前应核实只读职责、持久化写入和关闭机制，保持正常 bootstrap/upgrade 分离。修改版本校验应同步 `dml_runtime_test.rs`，覆盖未 bootstrap、版本缺失/不一致及旧版本保留；修改客户端允许面需另审 `diagnostic_sql.rs` 及独立 `diagnostic_sql_test.rs`。错误后的部分服务清理需要独立回归，不应从 Arc 自动释放推断。

## 验证依据

- 用 `git diff ad0a6f4..2c944fb -- pkg/session/diagnostic.rs` 确认新增文件，并读取冻结全文件、`runtime/session.rs` 的启动分支、校验函数和 metadata reader；读取 `runtime.rs`、`lib.rs`、`Cargo.toml` 确认可见性与接线。`pkg/session/doc.go` 不存在。
- `dml_runtime_test.rs::start_diagnostic_preserves_persisted_bootstrap_versions` 将 SQL/KV 版本都降到 current-1，调用本入口后重读仍为旧值，最后关闭 Domain。该测试不证明所有错误清理分支。
- Go 对照为冻结 `diagnostic.go`；RustCodeGraph status 可用且含 Rust，但索引不代表冻结 blob，行为以 git show 为准。
- 本次只做文档 Ready 等价检查：八篇各 11 个二级章节、局部链接、尾随空白和 diff 空白；不运行 Cargo、Go 测试或构建。Ready skill 路径缺失，按计划采用文档验证；上述测试仅审阅源码。
