# [`pkg/planner/optimize.rs`](./optimize.rs)

## 文件定位

本文件是 `astersql-planner` crate 的顶层优化编排层：它接收已经解析并包装为 `resolve::NodeW` 的 AST，协调只读准入、两类计划缓存、语句 hint、快速点查、SQL Binding、逻辑计划构建、物理优化和诊断辅助，最终返回 `Box<dyn base::Plan>` 与输出列名。crate 根 `pkg/planner/lib.rs` 将 `Optimize`、`OptimizeAstNode`、`OptimizeAstNodeNoCache`、`OptimizeExecStmt`、`OptimizeForForeignKeyCascade` 及安装/诊断接口重新导出；`pkg/planner/Cargo.toml` 则表明它直接依赖 parser AST、infoschema、sessionctx、planner core、logical/physical operator 与 rule 等内部 crate。

它不是具体优化规则的实现位置。计划树由 `core::PlanBuilder::Build` 构造，规则和代价优化由 `core::DoOptimize` 执行；本文件负责决定何时、以什么会话状态和优化标志调用这些能力。跨 crate 的执行器、缓存、Binding、权限和事务能力通过 `OptimizeRuntimeService`、`NonPreparedPlanCacheService`、`PlannerDiagnosticRuntime` 及函数指针注入。

当前接线边界必须特别注意：`InstallOptimizeCallbacks` 会把本文件的两个 AST 优化入口发布给 `core::InstallOptimizeAstNode`，但全仓 Rust 搜索除定义与 `pkg/planner/lib.rs` 再导出外，没有发现 `InstallOptimizeCallbacks`、`InstallDefaultOptimizeCallbacks`、`InstallPlannerDiagnosticRuntime` 或 `InstallReadOnlyAdmissionCallbacks` 的调用点。因此源码具备真实编排实现和公开安装接口，但不能仅据此断言完整服务器启动路径已经安装这些服务；未安装时相关入口会返回明确错误。

## 核心职责

- `Optimize` 是公开总入口：先执行集群只读模式准入，再把 `ExecuteStmt` 分派给预处理计划缓存，把其他语句分派给非预处理缓存/无缓存管线；只有优化成功后才应用资源组和缓存计划中的 `SET_VAR` 副作用。
- `getPlanFromNonPreparedPlanCache` 掌控非预处理缓存的资格、hint-only 策略、参数化、查找、缺失时生成与写回的严格顺序；具体缓存对象和存储由 `NonPreparedPlanCacheService` 实现。
- `optimizeNoCacheInner` 负责普通优化前置编排：解析 hint、应用 `SET_VAR`、严格模式临时移除 TiFlash、尝试 FastPlan、匹配 Binding、增加额外 Limit、事务预热及可选基线演进。
- `optimize` 负责默认轮和最多七类替代逻辑计划轮次，以物理代价严格小于当前胜者为条件选优，并把胜者对应的逻辑构建状态提交回会话。
- `buildPlan` 与 `optimizeBuiltLogicalPlanRound` 分开承担 AST 到逻辑/非逻辑计划的构建，以及逻辑计划到物理计划的优化；这使每个替代轮都能从同一 AST 和初始状态重新构建。
- 安装函数管理进程级只安装一次的回调/服务；诊断函数为索引顾问、Binding 和简要计划输出提供代价、digest、相关变量/修复项及 brief plan。
- `allowInReadOnlyMode` 复刻 Go 的只读语句白名单、特权旁路与 COMMIT 事务判定。

## 主要符号

公共数据边界包括：

- `OptimizeOutput` / `OptimizeResult`：统一的“计划 + 输出列名”结果及 `expression::Error` 错误类型。
- `ParameterizedPlanCacheStatement`、`NonPreparedCachedStatement`、`NonPreparedCacheability`：非预处理缓存参数化结果、不透明缓存对象和可缓存性判定。
- `StatementHintEffects`：优化成功后需要落入会话/事务的资源组与 `SET_VAR` 副作用。
- `BriefPlanData`：诊断接口输出的 digest、hint 字符串和二维计划文本。

服务 trait 包括：

- `LogicalPlanSessionStateService`：保存/恢复五组逻辑构建状态，并清理替代轮信号。
- `OptimizeRuntimeService`：承载 FastPlan、Binding、hint、资源组、严格模式 TiFlash、假想索引列查找等跨包能力。
- `OptimizeSessionService`：上述两类服务的组合约束；`DefaultOptimizeSessionService<R>` 自己管理五组状态，其他行为委托给 `R`。
- `NonPreparedPlanCacheService`：缓存资格、参数化 SQL、解析、读写缓存及取计划的强类型边界。
- `PlannerDiagnosticRuntime`：预处理、优化、物理代价、digest、brief plan 与 explain ID 控制。

公开入口包括 `InstallOptimizeCallbacks`、`InstallDefaultOptimizeCallbacks`、`InstallPlannerDiagnosticRuntime`、`InstallReadOnlyAdmissionCallbacks`、`Optimize`、`OptimizeAstNode`、`OptimizeAstNodeNoCache`、`OptimizeExecStmt`、`OptimizeForForeignKeyCascade`、`queryPlanCost`、`calculatePlanDigestFunc`、`recordRelevantOptVarsAndFixes`、`genBriefPlanWithSCtx` 和 `allowInReadOnlyMode`。

内部控制结构包括 `StrictTiFlashGuard`、`ScopeRestore`、`LogicalPlanBuildContext`、`OptimizationStateGuard`、`AlternativeRoundKind`、`AlternativeRound`、`AlternativeSignals`、`AlternativeRoundGuard` 与 `RoundResult`。`ALTERNATIVE_ROUNDS` 固定按 non-decorrelate、order-aware-reorder、correlate、semi-join-rewrite、fts-like-fallback、tikv-only、tiflash-only 的顺序枚举候选。

条件编译仅出现在文件尾：`#[cfg(test)] #[path = "optimize_aster_unit_test.rs"]` 将测试保持在独立文件，生产实现中没有 feature-gated 分支。

## 执行流程

1. 启动接线时，调用方应先安装诊断运行时、只读回调以及完整优化回调。`InstallOptimizeCallbacks` 依次写入 `RESULT_SET_BUILDER`、`EXECUTE_OPTIMIZER`、`NON_PREPARED_PLAN_CACHE`、`OPTIMIZE_SESSION_SERVICE`，最后调用 `core::InstallOptimizeAstNode(OptimizeAstNode, OptimizeAstNodeNoCache)`；任一重复安装都会报错。
2. `Optimize` 获取会话服务。当会话不是 restricted SQL 且任一集群只读开关开启时，它调用 `allowInReadOnlyMode`；拒绝写语句时返回 `ErrSQLInReadOnlyMode`。
3. 对 `ExecuteStmt`，`OptimizeExecStmt` 先再次验证 AST 类型，再调用已安装的 `EXECUTE_OPTIMIZER`。其他语句进入 `optimizeCache`。
4. `getPlanFromNonPreparedPlanCache` 先依据开关、语句形态、restricted SQL、EXPLAIN、事务自动重试和多语句模式判断初始资格。hint-only 策略还要求 SQL 或 Binding 中出现 `use_plan_cache`。不可缓存时只在 `EXPLAIN FORMAT=plan_cache` 下附加旁路警告。
5. 缓存合格时，服务先参数化 AST，再按参数化 SQL 查缓存。未命中时解析参数化 AST；解析失败被降级为 warning 并回退普通优化。解析成功后先写入参数值，再生成、存储缓存语句，最后以原参数值取出计划。
6. 无缓存路径由 `optimizeNoCache` 用 `catch_unwind` 包住，panic 被转换成规划错误。`optimizeNoCacheInner` 解析 SQL hint、安装语句 hint、登记 `SET_VAR` 恢复信息，并用 `StrictTiFlashGuard` 在严格写模式下临时移除 TiFlash。
7. 仅当隔离读引擎包含 TiKV 时尝试 FastPlan；命中立即返回。否则匹配 SQL Binding（匹配查询错误按 Go 行为忽略）、在匹配之后添加额外 Limit，并对启用的 Binding 临时替换 query hint。Binding 优化失败被记为 warning，不直接中止默认计划。
8. `advise_txn_warmup` 成功后，若 Binding 产出了计划则返回该计划；符合 evolve 开关、无限 `SelectLimit`、SELECT、Binding 无 `READ_FROM_STORAGE` 时，还会恢复原始 hint 生成默认计划供演进方观察。未选中 Binding 时恢复原 hint 并进入 `optimize`。
9. `optimize` 先通过 `OptimizationStateGuard` 保存初始状态并清空替代信号。`buildPlan` 清理瞬态状态、重置 PlanID/列 ID、构造 `PlanBuilder`、记录 rewrite 耗时和访问表。非逻辑计划直接以代价 0 返回；逻辑计划由 `core::DoOptimize` 生成物理计划和代价。
10. 默认物理计划若同时使用 TiKV/TiFlash 且不是 single-scan index join，会记录混合引擎信号。默认轮产生的信号一次性决定七类替代轮是否启用，避免后续轮次的信号反向改变候选集合。
11. 每个启用轮次都恢复初始状态、临时调整规则位或会话覆盖值、从 AST 重新构建并优化。候选仅在 `candidate.cost < current.cost` 时替换胜者；替代轮错误被记住并继续。默认轮存在不可执行 FTS 匹配时不会成为胜者；若最终无胜者，则优先返回最后一个替代轮错误。
12. 胜者的五组状态由 `OptimizationStateGuard::commit` 恢复并保留。回到 `Optimize` 后，仅成功结果触发资源组和 `SET_VAR` 副作用。

## 数据与状态

进程级状态存放在多个 `OnceLock`：四个优化服务/回调、诊断运行时以及两个只读准入回调。它们只允许初始化一次，读取时返回 `'static` 服务引用；未安装和重复安装都是显式错误，而不是回退到空实现。

每轮优化必须隔离五组会话状态：`StmtCtx::LogicalPlanBuildState`、`PlannerSelectBlockAsName`、标量子查询集合、扩展列 hash 到 unique ID 映射、`RewritePhaseInfo`。`DefaultLogicalPlanBuildState` 是不透明快照载体，`LogicalPlanBuildContext` 封装快照，`OptimizationStateGuard` 在每轮前恢复初始值、成功后提交胜者状态、异常退出时回滚初始状态。

`AlternativeSignals` 是从默认轮一次采集的值对象，包含解相关、顺序感知 join、correlate、semi join、FTS、谓词上下文及存储引擎信号。`AlternativeRoundGuard` 只在单轮生命周期内覆盖 correlate、semi-join、FTS fallback 或隔离读引擎集合，析构时恢复先前值。

`buildPlan` 每轮将 `PlanID` 和 `PlanColumnID` 置零，清空标量子查询、扩展列映射和 rewrite 信息，并把去重后的 `(db, table)` 写入 `StmtCtx`。`RoundResult` 把物理计划、列名、代价、胜者状态、优化标志及 FTS 信号绑定为不可拆分的候选。

## 依赖与调用关系

RustCodeGraph 对 `pkg/planner/optimize.rs` 报告 217 个符号。精确节点查询给出的内部主边为：`Optimize` 调用 `allowInReadOnlyMode`、`OptimizeExecStmt` 或 `optimizeCache`，成功后调用 `applySuccessfulOptimizeEffects`；`optimizeCache` 调用 `getPlanFromNonPreparedPlanCache`，未命中再调用 `optimizeNoCache`；`optimizeNoCache` 调用 `optimizeNoCacheInner`；后者调用多个 `OptimizeRuntimeService` 方法并最终调用内部 `optimize`。`getPlanFromNonPreparedPlanCache` 的被调方包含 `cacheable`、`parameterize`、`lookup_cached_statement`、`parse_parameterized_ast`、`set_parameter_values`、`generate_cached_statement`、`store_cached_statement` 与 `get_plan`，顺序与缓存算法一致。

向下依赖中，`core::NewPlanBuilder`/`PlanBuilder::Build` 负责构建，`core::DoOptimize` 负责规则与物理优化，`physicalop::StorageEngineUsage` 和 `HasSingleScanIndexJoin` 负责混合引擎候选判断，`hint::ParseStmtHints` 负责解析语句 hint，`vardef` 提供全局/会话开关，`stmtctx` 保存 warning、表信息和替代轮信号。

向上边界中，`pkg/planner/lib.rs` 是公开再导出点，`pkg/planner/core/optimizer_runtime.rs::InstallOptimizeAstNode` 是回调发布目标。RustCodeGraph 显示 `InstallOptimizeCallbacks` 的直接调用者只有同文件的 `InstallDefaultOptimizeCallbacks`；仓库文本搜索也未找到本文件之外的安装调用。`pkg/executor/Cargo.toml` 和若干 planner 测试 crate 依赖 `astersql-planner`，但依赖声明不等同于运行时完成安装。

测试边界位于独立文件 `pkg/planner/optimize_aster_unit_test.rs`。它直接覆盖内部纯判定与状态辅助；更下游的具体规则、计划缓存和物理计划测试分布在 `pkg/planner/core/**/_test.rs`，但不是本文件总入口的端到端接线证明。

## 错误处理与边界

大部分构建、优化和服务错误使用 `?` 立即传播为 `expression::Error`。服务未安装、重复安装、空 resolved AST、错误的 `OptimizeExecStmt` AST 类型、替代轮生成非逻辑计划、无有效胜者及诊断对象不是物理计划都有明确错误文本。

有意降级的错误必须与硬错误区分：Binding 查找错误被忽略；Binding 优化失败和无 Binding 计划写入 warning 后继续默认优化；参数化 AST 恢复/解析失败写 warning 并旁路非预处理缓存；替代轮失败保留最后错误但继续其他轮次；资源组未启用或权限不足、`SET_VAR` 应用失败也只写 warning。默认轮被判定为不可执行 FTS 计划且所有替代轮失败时，最后一个替代轮错误才成为最终错误。

`optimizeNoCache` 捕获 Rust panic：字符串或 `&str` payload 保留原消息，其他 payload 统一为 `panic during planner optimization`。这防止 panic 穿出规划 API，但不替代正常的 `Result` 错误路径。

只读准入把 SET、ANALYZE、USE、SHOW、Binding DDL、PREPARE、BEGIN、ROLLBACK 列为允许；COMMIT 必须交给事务回调判断；其他 AST 使用 `ast::util::IsReadOnly(node, false)`，其中 `false` 保证仍可通过全局变量关闭只读模式。若 COMMIT 回调未安装则报错。特权回调返回 true 时直接旁路。

## 并发与资源生命周期

进程级服务使用 `OnceLock` 和 `Arc<dyn ... + Send + Sync>`，避免运行中替换实现及数据竞争。`OptimizeRuntimeService`、`OptimizeSessionService`、`NonPreparedPlanCacheService`、`PlannerDiagnosticRuntime` 都要求 `Send + Sync`；但单次请求中的 `Rc<dyn Any>` 标量子查询快照仍保持会话线程亲和，不被跨线程共享。

三个 RAII 结构保证提前返回和错误路径也恢复状态：`ScopeRestore` 在 `Drop` 时执行一次闭包；`StrictTiFlashGuard` 恢复严格模式临时删除的 TiFlash；`AlternativeRoundGuard` 恢复会话覆盖值和隔离读引擎；`OptimizationStateGuard` 未 commit 时恢复初始快照。Binding query hint 也通过 `ScopeRestore` 恢复。

非预处理缓存对象用 `Arc` 跨服务共享，命中路径不创建/写回，未命中路径创建后将同一引用存入缓存并用于取计划。PlanID 与列 ID 使用 `SeqCst` 原子存储重置；这些 ID 仍是会话变量的一部分，不代表整个优化过程可在同一会话上并发执行。

计划构建与替代轮本身是同步流程；文件没有启动线程、异步任务或通道。`Instant` 仅记录 rewrite 时长。诊断函数 `recordRelevantOptVarsAndFixes` 和严格模式/替代轮覆盖都依赖作用域恢复，因此新增提前返回点时必须保持 guard 存活范围正确。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/optimize.go`。Rust 保留了 Go 的主要阶段：只读准入、EXECUTE 分流、非预处理缓存、panic 恢复、hint/Binding/FastPlan/事务预热、默认与替代逻辑计划轮次、五组逻辑构建状态、诊断钩子和 PlanID 提取。七个替代轮的顺序、主要启用条件、规则位调整、引擎限制及严格小于代价比较均能在两端对应。

实现形态有明显差异：Go 通过包级函数变量、`init()` 和直接访问 session/executor/bindinfo 完成接线；Rust 为消除 crate 环依赖，把这些能力抽象为 trait 和 `OnceLock` 安装接口。Go 的 `sync.Pool` 复用 `PlanBuilder`，Rust `buildPlan` 每轮调用 `core::NewPlanBuilder()` 新建 builder。Go 的 panic 恢复用 `defer/recover`，Rust 用 `catch_unwind`；Go 的轮次 cleanup 用闭包 `defer`，Rust 用 `Drop` guard。

还存在不可忽略的语义/接线差异。Go `OptimizeForForeignKeyCascade` 只构建 statement plan，并明确绕过缓存、Binding 和权限检查；Rust 同名函数直接调用内部 `optimize`，即执行构建加 `DoOptimize` 的多轮优化，但仍绕过公开 `Optimize` 的只读准入、缓存和 Binding。Go `buildAndOptimizeLogicalPlanRound` 在首次构建后执行 privilege、table lock、table mode 检查并 `RecheckCTE`；本 Rust 文件没有这些直接调用，是否由注入服务或更下层承担不能从本文件确认。Go `init()` 会立即设置优化/诊断钩子，而 Rust 当前仓库中未发现安装函数调用点。这些均应作为当前迁移状态记录，不能描述为完全等价。

Go 对 Binding 的 EXPLAIN 内层 hint、verbose note、baseline evolution task 等细节比本文件可见逻辑更丰富；Rust 将其中一部分委托给服务，且 `plan_contains_read_from_storage_hint` 的结果目前只被读取后丢弃。因此扩展或修复时应逐分支对照 Go，而不能只看公共函数名称。

## 扩展指南

- 新增顶层阶段时，优先放入 `Optimize`（全语句准入/成功副作用）、`optimizeCache`（缓存选择）或 `optimizeNoCacheInner`（普通优化前置）中最窄的正确层级，避免 `OptimizeAstNode` 回调重复执行只读准入和成功副作用。
- 新增跨 crate 能力时扩展相应 trait，并同步 `DefaultOptimizeSessionService<R>` 的委托实现与真实运行时实现；同时明确其安装顺序和未安装错误。若完成生产接线，应新增独立测试证明安装和总入口，而不把测试写进 `optimize.rs`。
- 修改非预处理缓存时保持“资格 → hint_only → cacheability → parameterize → lookup → parse → set values → generate → store → get plan”的顺序，并在 `pkg/planner/optimize_aster_unit_test.rs` 扩充命中、未命中、警告与错误边界测试。
- 新增替代轮时，需要同步 `AlternativeRoundKind`、`ALTERNATIVE_ROUNDS`、`AlternativeRound::enabled`/`adjust_flag`、必要的 `AlternativeRoundGuard` setup/drop，以及默认轮信号采集。必须保证候选集合只由默认轮信号决定、每轮从初始快照开始、临时状态在 panic/错误时恢复，并补充资格与恢复测试。
- 修改状态快照时应同时更新 `DefaultLogicalPlanBuildState` 的保存/恢复和独立幂等测试；遗漏任一组可能使低价胜者携带错误的表、hint、子查询 ID 或 rewrite 计时。
- 修改只读准入时同步 Go 的白名单、COMMIT 语义与 `false` 参数约束，并扩展 `read_only_mode_*`/`commit_decision_*` 测试。
- 修改 Go 对齐行为前，重点审查当前已知差异：外键级联是否应仅 build、权限/锁/table mode/CTE 检查的真实所有者、诊断/优化回调的生产安装点、Binding evolution 的下游副作用。性能上重点关注 builder 每轮新建、替代轮重建次数和缓存 miss 路径。

## 验证依据

- Rust 源码：`pkg/planner/optimize.rs`，完整检查 1–1836 行；关键符号包括 `Optimize`、`getPlanFromNonPreparedPlanCache`、`optimizeNoCacheInner`、`optimize`、`buildPlan`、`InstallOptimizeCallbacks`、各 guard 和诊断函数。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边，目标文件含 217 个符号；`files --filter pkg/planner/optimize.rs` 确认文件入图；`node optimize.rs::Optimize`、`node optimize.rs::InstallOptimizeCallbacks`、`node optimize.rs::getPlanFromNonPreparedPlanCache`、`node optimize.rs::optimizeNoCacheInner`、`node optimize.rs::OptimizeForForeignKeyCascade` 给出了上述调用/被调用边。宽泛 `explore` 因 `Optimize` 名称高歧义产生跨仓库噪声，故用限定节点与 `rg` 补充上游接线搜索。
- crate 与模块边界：`pkg/planner/Cargo.toml`、`pkg/planner/lib.rs`、`pkg/planner/core/optimizer_runtime.rs`；Cargo 元数据将 Go package 标为 `pkg/planner`，core 侧以两个 `OnceLock<OptimizeAstNodeFn>` 接收回调。
- Go 对照：`pkg/planner/optimize.go`，核对 `Optimize`、`optimizeCache`、`optimizeNoCache`、`optimize`、`alternativeRounds`、`buildAndOptimizeLogicalPlanRound`、`OptimizeExecStmt`、`OptimizeForForeignKeyCascade`、`allowInReadOnlyMode`、诊断函数与 `init()`。
- 独立 Rust 测试：`pkg/planner/optimize_aster_unit_test.rs`，覆盖只读分类/COMMIT 错误传播、非预处理缓存资格与 hit/miss、warning 条件、scope 恢复、替代轮和引擎轮资格、引擎覆盖恢复、FastPlan/Binding/Baseline 条件、五组状态反复恢复、构建前瞬态清理和 PlanID 非计划值拒绝。
- 接线核验：全仓 `rg` 对四个 `Install*` 函数未发现 `pkg/planner/optimize.rs` 定义和 `pkg/planner/lib.rs` 再导出之外的调用；因此文档将生产安装状态标为“未发现接线”，而非“已支持”。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在，且固定的十一个二级标题恰好各出现一次。
