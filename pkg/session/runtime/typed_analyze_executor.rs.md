# `pkg/session/runtime/typed_analyze_executor.rs`

## 文件定位

本文件属于 `astersql-session` crate 的 session runtime，实现 `astersql_executor::adapter::ExecExecutor` 与会话统计分析管线之间的专用桥接器。模块由 [`pkg/session/runtime.rs`](../runtime.rs) 私有声明，类型 `SessionTypedAnalyzeExecutor` 也仅以 `pub(super)` 暴露给 runtime 父模块，因此它不是对外 API。

上游在 [`SessionBoundAdapterOwner::BuildExecutor`](scan_adapter_runtime.rs) 识别 `PlanKind::Analyze` 后，从 owner 中取得预先绑定的规范 SQL，并构造该执行器；下游由 `ConcreteSession::execute_analyze`（[`statistics.rs`](statistics.rs)）执行实际统计收集与持久化。本文件本身不实现采样、直方图或 TopN 算法，而是把适配层的 `Open/Next/Close` 生命周期转换为一次 session 级 ANALYZE 调用。

## 核心职责

1. 保存 ANALYZE 所需的 thread-local `ConcreteSession` 与规范 SQL，并以无结果集执行器的形式接入统一 statement adapter。
2. 在首次 `Next` 时按当前 SQL mode 重新解析 SQL，强制要求恰好一条 `ast::AnalyzeTableStmt`，再调用 `ConcreteSession::execute_analyze`。
3. 对受限 SQL（内部/自动分析）临时应用专用并发与 `READ-COMMITTED` 隔离级别，并保证成功或错误返回时恢复原值。
4. 向 adapter 声明这是立即执行、写语义、零列 schema、无外键级联且不可 detach 的执行器。

这些职责分别由 `SessionTypedAnalyzeExecutor::execute`、`RestrictedAnalyzeGuard::drop` 和 `impl ExecExecutor for SessionTypedAnalyzeExecutor` 承担。

## 主要符号

- `SessionTypedAnalyzeExecutor`：一次绑定 ANALYZE 的状态机。`session: Rc<ConcreteSession>` 保持会话所有权；`sql` 保存规范 SQL；`opened` 约束调用顺序；`done` 保证每次打开后最多执行一次；`schema` 永远为空。
- `SessionTypedAnalyzeExecutor::new(session, sql)`：父模块可见的构造函数，初始 `opened = false`、`done = false`、空 schema。
- `SessionTypedAnalyzeExecutor::execute(&self)`：核心私有入口；解析并校验 AST，准备受限 ANALYZE 环境，然后调用 `ConcreteSession::execute_analyze`。
- `RestrictedAnalyzeGuard<'a>`：借用 `ConcreteSession` 并保存 `analyze_concurrency`、`restricted_analyze_scan_concurrency`、`transaction_isolation` 三个旧值。
- `RestrictedAnalyzeGuard::drop`：RAII 恢复点。通过 `ConcreteSession.state: RefCell<_>` 写回三个旧值；`std::mem::take` 转移 guard 保存的隔离级别字符串。
- `ExecExecutor::{Open, Next, Close}`：`Open` 开启并重置 `done`，`Next` 清空输出并触发一次执行，`Close` 仅关闭状态。
- `ChunkConfig`、`NewChunk`、`Schema`：共同描述容量为 1 但没有字段/行负载的空输出契约。
- `CalculateNoDelay = true`、`IsWriteExecutor = true`：使 adapter 当场跑完该无 schema 语句，并按写执行器处理记账语义。
- 外键与分离接口：`CheckForeignKeys` 成功空操作，cascade 查询均为空/false，`Detach` 返回 `None`。

## 执行流程

1. [`SessionBoundAdapterOwner::BindAnalyzeStatement`](typed_adapter_bridge.rs) 先按会话 SQL mode 校验“单条 `ANALYZE TABLE`”，并把 SQL 写入 `analyze_sql`。
2. [`SessionBoundAdapterOwner::BuildExecutor`](scan_adapter_runtime.rs) 遇到 `PlanKind::Analyze` 时读取该 SQL；若未绑定，立即返回 `canonical ANALYZE was not bound to session runtime`，否则调用 `SessionTypedAnalyzeExecutor::new`。
3. statement adapter 调用 `Open`。执行器置 `opened = true` 并清除上一次的 `done`，允许这一轮生命周期执行一次。
4. adapter 因空 schema 或 `CalculateNoDelay()` 为真进入无延迟路径；`Next` 首先拒绝未打开状态，随后 `output.Reset()`。若尚未执行，它先把 `done` 置为 true，再调用 `execute`。
5. `execute` 将 session 的 `sql_mode` 转为 parser mode，调用 `parse_with_sql_mode`，要求结果长度为 1，并将唯一 AST 向下转型为 `ast::AnalyzeTableStmt`。这层重复验证保护“计划种类与绑定 SQL/AST 不一致”的边界，即便正常绑定入口已经校验过一次。
6. 若 `state.in_restricted_sql` 或 `SessionVars.InRestrictedSQL` 任一为真，则读取 `tidb_auto_build_stats_concurrency` 与 `tidb_sysproc_scan_concurrency`。合法的前者覆盖统计构建并发，正数的后者写入受限扫描并发，同时隔离级别临时改为 `READ-COMMITTED`。
7. 调用 `ConcreteSession::execute_analyze(statement)`。该方法在 [`statistics.rs`](statistics.rs) 中组装 `SessionAnalyzeRuntime`，把 `state.analyze_concurrency` 传入分析运行时，并最终进入 `astersql_executor::analyze::AnalyzeExec::RunCanonicalWithContext`；受限扫描路径在 [`relational_scan.rs`](relational_scan.rs) 读取 `restricted_analyze_scan_concurrency`。
8. `execute` 返回时局部 `_guard` 离开作用域；无论下游成功还是通过 `?` 提前返回错误，`Drop` 都恢复三个会话字段。随后 adapter 完成无延迟语句并调用关闭流程。

## 数据与状态

执行器内部只有单线程、单生命周期状态，没有结果行缓存：

- `opened` 是硬前置条件；未 `Open` 的 `Next` 返回错误。
- `done` 在调用 `execute` **之前**置为 true，因此同一轮 `Open` 后最多尝试一次，即便执行失败也不会由再次 `Next` 隐式重试。重新 `Open` 会再次置为 false。
- `schema` 从构造起保持空，`NewChunk` 也使用空 `FieldType` 列表；`Next` 每次都重置调用方 chunk。
- 受限执行期间临时修改的是 `ConcreteSession.state` 中的 `analyze_concurrency`、`restricted_analyze_scan_concurrency` 与 `transaction_isolation`。guard 保存的是精确旧值，而非默认值。
- 自动构建并发只接受 `1..=256`；系统进程扫描并发只接受正 `i32`。变量读取失败、解析失败或越界时不会中止 ANALYZE：对应覆盖值保持缺省/`None`，但隔离级别仍切为 `READ-COMMITTED`。

## 依赖与调用关系

上游主链为：`BindAnalyzeStatement` → `SessionBoundAdapterOwner.analyze_sql` → `BuildExecutor(PlanKind::Analyze)` → `SessionTypedAnalyzeExecutor::{Open, Next, Close}`。统一 adapter 契约定义在 [`pkg/executor/adapter.rs`](../../executor/adapter.rs)：其无延迟判断会对空 schema 或 `CalculateNoDelay` 执行 `handleNoDelayExecutor`。

下游主链为：`execute` → `parse_with_sql_mode` / `ast::AnalyzeTableStmt` → `ConcreteSession::execute_analyze` → `AnalyzeExec::RunCanonicalWithContext`。扫描并发覆盖继续被 `scan_registered_table_with_limit` 消费。

[`pkg/session/Cargo.toml`](../Cargo.toml) 将本文件归入 `astersql-session`（`lib.rs` 入口），并直接声明本文件使用的 `astersql-errors`、`astersql-executor`、`astersql-parser-ast`、`astersql-parser-mysql`、`astersql-parser-types`、`astersql-sessionctx-vardef`、`astersql-sessionctx-variable` 与 `astersql-util-chunk` 等 workspace path 依赖。本文件没有 feature 条件或条件编译项。

## 错误处理与边界

- SQL mode 转换和重新解析错误统一用 `astersql_errors::New(error.to_string())` 转为 adapter 共享错误。
- 解析结果不是恰好一条语句时返回 `bound ANALYZE must contain exactly one statement`；AST 不是 `AnalyzeTableStmt` 时返回 `bound ANALYZE plan/AST kind mismatch`。
- `Next` 在 `opened == false` 时返回 `ANALYZE executor is not open`。
- `ConcreteSession::execute_analyze` 的 `SessionError` 通过 `into_shared()` 保留为 adapter 错误；本层不吞掉实际分析失败。
- 读取受限 SQL 专用 sysvar 的失败是有意降级，不会阻止分析。这与 Go 版本使用 `terror.Log` 记录后继续的控制流一致；Rust 当前在本层不产生日志。
- RAII guard 在正常返回和 `Result` 错误传播时恢复状态。若 `Drop` 中的 `RefCell::borrow_mut` 因已有活跃借用而 panic，则无法提供普通错误恢复；现有控制流在构造 guard 前释放了局部 `state` 借用，避免正常路径触发该冲突。
- `Detach = None` 表示不能把执行器变成脱离会话的 `Send` 扫描；调用方必须保留原 session owner。

## 并发与资源生命周期

`Rc<ConcreteSession>`、`RefCell` 与 adapter trait 的注释共同表明该执行器绑定当前线程，不承担跨线程并发执行。它不会自行创建线程、任务、通道、锁或事务；实际 ANALYZE worker 并发由下游 `AnalyzeExec` 根据 `SessionAnalyzeRuntime.concurrency` 管理。

资源生命周期遵循 `new → Open → 一次有效 Next → Close`。`Open` 可重置一次性标志，`Close` 不销毁 session 或 SQL，只关闭本轮状态。`RestrictedAnalyzeGuard` 的生命周期严格包围 `execute_analyze` 调用，确保临时配置不会泄漏到后续语句；[`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 同时验证成功和失败后都恢复原构建并发、空扫描覆盖以及 `REPEATABLE-READ`。

执行器不持有输出资源、外键级联批或可分离快照。`NewChunk` 的单行容量只是满足统一接口，ANALYZE 的可观察结果是统计元数据副作用而非返回行。

## 与 Go 版本的对应关系

最直接的 Go 对照位于 [`pkg/executor/adapter.go`](../../executor/adapter.go) 的 statement 执行路径：当计划是 `*plannercore.Analyze` 且 `InRestrictedSQL` 为真时，它保存原 `tidb_build_stats_concurrency`、分析扫描并发和事务隔离级别，读取 `tidb_auto_build_stats_concurrency` / `tidb_sysproc_scan_concurrency`，临时设置 `READ-COMMITTED`，并用 `defer` 恢复。Rust 的 `RestrictedAnalyzeGuard::drop` 对应 Go 的 `defer`，目的和恢复字段一致。

Go 的实际分析执行器是 [`pkg/executor/analyze.go`](../../executor/analyze.go) 中 `AnalyzeExec::Next`，负责收集任务、选择构建并发、启动 worker 并保存结果；Rust 本文件只相当于 session/adapter 边界，真正算法下沉到 Rust `ConcreteSession::execute_analyze` 与 `astersql_executor::analyze::AnalyzeExec`。

当前可见差异包括：Rust 以 `state.in_restricted_sql || SessionVars.InRestrictedSQL` 判定受限模式，Go 对照片段只检查 `SessionVars.InRestrictedSQL`；Rust 对读取值做显式范围过滤且静默降级，Go 借助 sysvar 校验/解析并用 `terror.Log` 记录错误；Rust 为防止绑定错配会在执行时重新解析并检查 AST。以上是当前代码事实，不应据此推断二者所有诊断日志或非法 sysvar 行为完全等价。

## 扩展指南

- 新增 ANALYZE 执行前后的 session 临时设置时，应把旧值加入 `RestrictedAnalyzeGuard` 并在 `Drop` 对称恢复；不能只在成功分支手工恢复。同步扩展 `restricted_adapter_analyze_publishes_stats_and_restores_temporary_session_settings` 的成功与失败断言。
- 修改 SQL/AST 接受范围时，必须同时审查 `BindAnalyzeStatement` 与 `execute` 的二次校验，避免绑定阶段接受而执行阶段拒绝，或 `PlanKind::Analyze` 与 AST 类型脱节。
- 修改一次性执行语义时应保留“错误后不在同一生命周期重跑”的明确选择，并在独立的 [`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 添加测试；不要把 Rust 测试内嵌到本生产文件。
- 若要支持返回行或延迟结果集，需要联合修改 `schema`、`ChunkConfig`、`NewChunk`、`Next` 和 `CalculateNoDelay`，并复核 adapter 的 `handleNoDelay` 分支，而不能只向 chunk 写数据。
- 若要支持 detach/跨线程执行，当前 `Rc<ConcreteSession>`、`RefCell` 借用和 thread-bound session contract 都必须重新设计；简单把 `Detach` 改成 `Some` 不安全。
- 性能敏感点是并发 sysvar 到 `SessionAnalyzeRuntime` 和扫描请求的传递。变更范围过滤或默认降级策略时，应同时核对 `tidb_vars.rs` / `sysvar_builtins.rs` 的合法范围、Go adapter 行为与统计测试，防止内部 ANALYZE 过度并发。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标文件已索引；`files --filter pkg/session/runtime/typed_analyze_executor.rs` 确认单一目标；`explore "pkg/session/runtime/typed_analyze_executor.rs TypedAnalyzeExecutor"` 返回文件完整源码；`query SessionTypedAnalyzeExecutor` 与 `node SessionTypedAnalyzeExecutor` 定位结构体。泛化的 `callers/callees` 对 `execute`、`Open`、`Next` 等重名符号产生歧义，因此关键边进一步用精确源码引用核验。
- 目标源码：[`typed_analyze_executor.rs`](typed_analyze_executor.rs)，核对两个结构体、`Drop`、构造/执行方法和完整 `ExecExecutor` 实现；文件无条件编译项。
- Rust 入口与调用边：[`typed_adapter_bridge.rs`](typed_adapter_bridge.rs) 的 `BindAnalyzeStatement`， [`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 的 `BuildExecutor`，以及 [`pkg/executor/adapter.rs`](../../executor/adapter.rs) 的 `ExecExecutor`、`handleNoDelay` 和执行生命周期。
- Rust 下游：[`statistics.rs`](statistics.rs) 的 `ConcreteSession::execute_analyze` / `SessionAnalyzeRuntime`，以及 [`relational_scan.rs`](relational_scan.rs) 对 `restricted_analyze_scan_concurrency` 的消费。
- crate 边界：[`pkg/session/Cargo.toml`](../Cargo.toml)；`pkg/session` 根目录未发现 `doc.go`，故无可读取的 package contract 文件。
- 独立 Rust 回归测试：[`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 的 `restricted_adapter_analyze_publishes_stats_and_restores_temporary_session_settings`，覆盖绑定、无结果立即执行、统计发布，以及成功/失败后的三项状态恢复。未发现与目标文件同名的独立测试文件。
- Go 对照：[`pkg/executor/adapter.go`](../../executor/adapter.go) 的受限 ANALYZE 临时设置与 `defer` 恢复；[`pkg/executor/analyze.go`](../../executor/analyze.go) 的 `AnalyzeExec::Next`；sysvar 定义见 `pkg/sessionctx/vardef/tidb_vars.go` 与 `pkg/sessionctx/variable/sysvar.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档恰有十一个固定二级章节，并人工复核所有行为陈述均指向上述源码或测试证据。
