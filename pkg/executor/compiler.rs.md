# [`pkg/executor/compiler.rs`](compiler.rs)

## 文件定位

本文件属于 `astersql-executor` crate；crate 根在 `pkg/executor/lib.rs` 以 `pub mod compiler` 对外公开它，并在测试构建中以 `mod compiler_test` 挂载独立测试 `pkg/executor/compiler_test.rs`。它位于 SQL 前端从 AST 到执行器的边界：输入是 `astersql_parser_ast::NodeRef`，输出是 `crate::adapter::ExecStmt`，中间协调预处理、优化、指标、计划追踪、PointGet 缓存复用、事务预热和语句 RU 所有者安装。

当前 Rust 实现不是直接绑定具体会话/规划器的生产适配器，而是通过 `CompilerDependencies` 抽象所有外部行为。仓库文本搜索只确认模块公开与独立单元测试引用，未发现生产 Rust 代码构造或调用此处 `Compiler`；因此它是已实现的编译流程核心和迁移契约，但其生产接线在本次证据范围内未验证。Go 的生产实现位于 `pkg/executor/compiler.go`。

## 核心职责

- `Compiler::Compile` 提供带 panic 边界和编译 tracing region 生命周期的公开入口。
- `Compiler::compileInner` 固定各阶段顺序，并要求所有会话、事务、规划器、指标和追踪操作由 `CompilerDependencies` 提供，避免缺失阶段静默成功。
- `needLowerPriority`、`needLowerPriorityView` 与 `isPhysicalPlanNeedLowerPriority` 根据计划类型和估算行数递归判定是否降低 coprocessor 请求优先级。
- `CountStmtNode`、`getStmtDbLabel` 与 `getDbFromResultNode` 实现非受限 SQL 的语句计数及数据库标签抽取。
- `preparedCacheForExec` 保证仅当 PointGet 执行器确认可安全复用时，才把 prepared 缓存交给 `ExecStmt`。
- `hexEncode` 为计划 digest 生成小写十六进制文本；编译完成后把 typed plan 写入 `ExecStmt::TypedPlan`，并调用 `statement_ru_result::install_statement_ru_owner`。

## 主要符号

- `CompilerContext(Arc<dyn Any + Send + Sync>)`、`CompilerSession(Arc<dyn Any + Send + Sync>)`：可克隆的不透明句柄；默认值只包装单元值，真正语义由适配器解释。
- `CompileRegion` 与私有 `CompileRegionGuard`：前者只定义 `End`，后者在 `Drop` 时调用它，覆盖正常返回、`?` 提前返回和 unwind。
- `CompilerInfoSchema`：要求 `Send` 的信息模式边界；具体读取能力仍由依赖实现掌握。
- `CompilerPreprocessResult`：携带 `resolve::Context` 与 `lastSnapshotTS`；后者供 `AssertTransactionState` 检查快照/事务一致性。
- `CompilerPreparedStatement`：同时持有 `PlanCacheStmt` 和已经针对原 prepared AST 计算好的 `CompilerStatementView`。
- `CompilerOptimizeResult`：优化后的 `Box<dyn base::Plan>` 与输出列名。
- `CompilerSessionState`：编译阶段使用的受限 SQL、资源组、优先级、连接 ID 和日志脱敏开关快照。
- `CompilerPlanTrace`、`CompilerMetricMode`、`ExecStmtBuildInput`：分别封装追踪数据、指标配置和构建 `ExecStmt` 所需输入。
- `PriorityPlan`/`PhysicalPriorityPlan`：将具体计划降维为优先级判定视图；支持物理树、`Execute` 包装和 Insert/Delete/Update 的可选查询子计划。
- `CompilerStatementView`、`StatementDatabaseView`、`ResultNodeView`：将大量具体 AST 类型归一为“语句类型 + 数据库来源树”，隔离本文件与具体 AST downcast。
- `CompilerDependencies`：本文件最关键的扩展契约，集中定义 24 个外部操作，包括 preprocessing、optimization、prepared cache、事务、metrics、trace、panic 恢复及最终构建。
- `Compiler { Ctx, dependencies }`：公开编译器对象；`Compile` 公开，`compileInner` 私有。

## 执行流程

1. `Compiler::Compile` 先通过 `StatementText` 保存 SQL 文本，再调用 `StartCompileRegion`；返回的 region 立即放入 `CompileRegionGuard`。
2. 它以 `catch_unwind(AssertUnwindSafe(...))` 包裹 `compileInner`。普通 `Result` 原样返回；panic 先交给 `RecoverCompilePanic`。可恢复时记录 `LogCompilePanic` 并返回业务错误，不可恢复时 `resume_unwind`。
3. `compileInner` 先设置只读标志，调用 `Preprocess`，再用 `AssertTransactionState` 检查 preprocessing 产出的解析/快照状态。
4. 从事务取得 info schema，抓取 `CompilerSessionState`，并尝试解析 prepared statement；随后 `Optimize` 生成最终计划和输出名，再检查 statement staleness。
5. 指标语句视图优先取 prepared 缓存中的原语句视图，否则由当前 AST 和 resolve context 生成；`CountStmtNode` 对非受限 SQL 计数。
6. 只有会话优先级等于 `mysql::NoPriority` 时才执行昂贵计划判定；随后无条件调用 `SetPlan`，让 statement context 看到优化结果。
7. `PlanTrace` 返回数据时，digest 经 `hexEncode` 编码；非空 normalized plan 按 `redactLog` 处理，再连同语句类型和 connection ID 发出 trace。
8. 有 prepared 对象时调用 `ReusePointGetPlan`，它可以原地替换 `optimized.plan`。无 prepared 对象则复用结果为 `false`。
9. 对最终计划执行 `WarmUpTransaction`，再将 `Box<dyn Plan>` 转为共享的 `Arc<dyn Plan>`，用 `ExecStmtBuildInput` 构建语句。只有复用成功时，`preparedCacheForExec` 才附加缓存。
10. 对 `EXECUTE` 语句，若 typed plan 本身不是 `RuntimeExecute`，则包装为 `RuntimeExecute::New`；其他情况直接保存原 typed plan。最后安装 statement RU owner 并返回。

## 数据与状态

编译器自身只长期持有 `Ctx` 和可变的依赖对象；一次编译的 info schema、session state、prepared cache、计划和输出名均为局部状态。`CompilerContext`、`CompilerSession` 和最终 typed plan 使用 `Arc`，表达共享所有权；优化阶段的计划先由 `Box` 独占，PointGet 复用有机会原地替换后才转换为 `Arc`。

`CountStmtNode` 的标签集合使用 `HashSet<String>` 去重。没有可识别数据库时插入空串，确保启用按库指标后仍有一个标签样本。`ResultNodeView::Join` 递归合并左右节点；`Binding` 优先取 origin，只在 origin 无数据库时退回 hinted。`getDbFromResultNode` 本身返回 `Vec` 并可能保留重复，但进入 `getStmtDbLabel` 后会去重。

优先级判断把 `estimatedRows: f64` 转成 `i64` 后与 `ExpensiveThreshold` 做严格大于比较；等于阈值不降级。显式设置了语句优先级时完全跳过这一自动判定。

## 依赖与调用关系

RustCodeGraph 对 `Compiler::Compile` 的下游边确认了 `CompileRegionGuard`、`StartCompileRegion`、`StatementText`、`compileInner`、`RecoverCompilePanic` 和 `LogCompilePanic`。对 `compileInner` 的图边确认了所有核心依赖调用，以及 `CountStmtNode`、`needLowerPriority`、`preparedCacheForExec` 和 `hexEncode`。辅助调用链为：

`Compile → compileInner → {Preprocess, Optimize, CountStmtNode, needLowerPriority, PlanTrace, ReusePointGetPlan, WarmUpTransaction, BuildExecStmt} → ExecStmt`

直接 crate 依赖由 `pkg/executor/Cargo.toml` 声明：本文件实际使用 `astersql-errors`、`astersql-parser-ast`、`astersql-planner-core`、`astersql-planner-core-base`、`astersql-planner-core-resolve`、`astersql-types`，并通过 crate 内 `adapter` 与 `statement_ru_result` 接入执行语句及 RU 生命周期。MySQL `NoPriority` 常量来自 workspace 中的 parser MySQL crate。

上游方面，`pkg/executor/lib.rs` 公开模块；RustCodeGraph/文本搜索未确认生产 Rust 调用者。`pkg/executor/compiler_test.rs` 是已确认的直接测试调用者。Go 主链则由会话层构造 `executor.Compiler{Ctx: ...}` 后调用 `Compile`，但这只能作为迁移语义证据，不能证明 Rust 接线。

## 错误处理与边界

`Preprocess`、`PreparedStatement`、`Optimize`、`ReusePointGetPlan` 和 `WarmUpTransaction` 的错误都用 `?` 立即传播，不继续构建半成品 `ExecStmt`。`BuildExecStmt` 本身返回对象而非 `Result`，所以构建失败策略由依赖实现约束。

panic 并非全部转成错误：只有 `RecoverCompilePanic` 明确认可的 panic 才记录并返回 `errors::Error`，其余保持 panic 语义重新抛出。这与 Go 只恢复内存超限、查询中断、最大执行时间等特定错误的意图一致。无论错误还是 panic，`CompileRegionGuard` 都会结束 region；如果 `End` 自身 panic，则 Rust 的析构 panic 规则仍适用，本文件没有二次保护。

本文件依赖适配器正确构造 `StatementView`、`PriorityPlan` 和 `ResultNodeView`。例如真实表名需先通过 `resolve::Context` 解析；本文件不会自行访问 AST 的具体表元数据。`CompilerDependencies` 没有默认成功实现，这防止生产适配器遗漏关键阶段，但也意味着当前文件单独不能运行。

## 并发与资源生命周期

`CompilerContext` 和 `CompilerSession` 的内部值要求 `Send + Sync`，typed plan 由 `Arc` 共享；但 `CompilerDependencies` 本身未要求 `Send`/`Sync`，且所有有副作用方法接收 `&mut self`。因此单个 `Compiler` 的一次编译是顺序可变流程，本文件没有线程创建、锁、channel 或异步任务，也不承诺同一实例可并发调用。

region 生命周期由 RAII 守卫限定为整个 `Compile` 调用。info schema 与 prepared cache 由 `Box` 独占并在成功时转移给构建输入；计划由 `Box` 经 PointGet 改写、事务预热后转换成 `Arc`，避免在最终执行语句和 `TypedPlan` 之间复制计划。statement RU owner 在返回前安装，其后续消费和终结由 `adapter.rs`/`statement_ru_result.rs` 负责，不在本文件内完成。

## 与 Go 版本的对应关系

`pkg/executor/compiler.go` 是逐项对照基线。Rust `Compile` 保留了 Go 的 tracing region、选择性 panic 恢复、只读判定、preprocess、事务断言、事务 info schema、prepared EXECUTE、optimizer、staleness 断言、计数、低优先级判定、statement context 计划、计划 digest trace、PointGet 缓存复用、事务预热和最终 `ExecStmt` 构建顺序。

Rust 的主要结构差异是把 Go 对全局配置、metrics、sessionctx、planner、AST 类型断言及 failpoint 的直接调用抽进 `CompilerDependencies`，并用视图枚举表达计划/AST 分类；这是适配边界变化，不应解释为已存在具体生产实现。Rust 还显式设置 `TypedPlan`、必要时包装 `RuntimeExecute`，并安装 statement RU owner；Go 版本在构造 `ExecStmt` 后调用 `installStatementRUOwner`，其 plan 字段使用 Go 的具体接口对象。

Go 对 `CREATE MATERIALIZED VIEW` 在优化期间临时设置 `InMViewMaintenance`；Rust 核心中没有对应显式分支，只有 `Optimize` 依赖入口，是否由适配器实现该语义在本次搜索中未验证。Go 的数据库标签 switch 覆盖具体 AST 类型，Rust 则要求适配器先生成等价 `StatementDatabaseView`。Go 同路径测试未直接覆盖这些 helper；Rust 独立测试目前只覆盖 PointGet 缓存附加条件。

## 扩展指南

- 新增编译阶段：在 `CompilerDependencies` 增加无默认成功回退的方法，并在 `compileInner` 的语义正确位置调用；同时在独立 `pkg/executor/compiler_test.rs` 增加依赖 mock，验证顺序、错误短路和状态转移，不要把测试写回源文件。
- 新增语句/数据库标签类型：扩展 `StatementDatabaseView` 或 `ResultNodeView`、`getStmtDbLabel`，并同步负责 concrete AST downcast 的适配器；对空库名、join、多表和 binding origin/hinted 回退增加测试。
- 修改优先级策略：保持 `Execute` 递归、DML 查询子计划和整棵物理树遍历，重点评估 `f64 → i64`、阈值边界及深树遍历成本；与 Go `needLowerPriority` 同步。
- 修改 prepared/PointGet：同时审查 `ReusePointGetPlan`、`preparedCacheForExec`、最终 `TypedPlan` 包装和 `adapter.rs` 的 PointGet 构建路径。普通 EXECUTE 不得错误附带 prepared cache。
- 增加 trace 字段或 panic 类型：分别同步 `CompilerPlanTrace`/`EmitPlanTrace` 或 `RecoverCompilePanic`/`LogCompilePanic`，并维持脱敏与不可恢复 panic 重新抛出的兼容性。
- 接通生产实现前，应先证明具体 `CompilerDependencies` 完整映射 Go 行为，尤其是 materialized view maintenance、事务断言、stale read、metrics、计划缓存和 RU owner；现有测试覆盖不足，不能只以编译通过作为完成证据。

## 验证依据

- 源码：`pkg/executor/compiler.rs`，核对全部类型、trait、函数、`impl Compiler`、错误传播和生命周期；文件无条件编译项。
- crate/模块：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`，确认 crate 边界、直接依赖、`pub mod compiler` 和独立 `compiler_test` 挂载。
- 下游对象：`pkg/executor/adapter.rs` 的 `ExecStmt`/`TypedPlan`/PointGet 构建路径，`pkg/executor/statement_ru_result.rs` 的 `install_statement_ru_owner`，`pkg/planner/core/planbuilder_runtime.rs` 的 `RuntimeExecute`。
- Rust 测试：`pkg/executor/compiler_test.rs` 验证 `preparedCacheForExec(Some, false) == None`、`(Some, true) == Some`、`(None, true) == None`。
- Go 对照：`pkg/executor/compiler.go` 的 `Compiler.Compile`、`needLowerPriority`、`isPhysicalPlanNeedLowerPriority`、`CountStmtNode`、`getStmtDbLabel`、`getDbFromResultNode`。
- RustCodeGraph：`status` 显示索引包含 `pkg/executor/compiler.rs`；`query Compiler/needLowerPriority/CountStmtNode` 定位 Rust 与 Go 符号；`callees` 确认 `Compile`、`compileInner` 及辅助函数的下游边。`callers` 对跨语言同名方法未给出可靠 Rust 生产调用方，故以 `rg` 对 `.rs` 的直接引用补充并将生产接线标为未验证。
- 结构验证按任务命令执行；本任务是纯文档分析，依计划未运行 Cargo 或代码测试。
