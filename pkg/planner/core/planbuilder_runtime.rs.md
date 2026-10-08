# `pkg/planner/core/planbuilder_runtime.rs`

## 文件定位

`planbuilder_runtime.rs` 是 `astersql-planner-core` crate 中把已解析、已解析名字的 SQL AST 转换为运行时计划的上层分派器和状态容器。crate 根在 `pkg/planner/core/lib.rs` 中以 `mod planbuilder_runtime` 纳入本文件，再通过 `pub use planbuilder_runtime::*` 对外暴露其类型和入口。`pkg/planner/core/Cargo.toml` 说明它属于 `astersql-planner-core`，并直接依赖 parser AST、InfoSchema、expression、hint、logicalop、physicalop、rule、resolve 和 planner errors 等内部 crate。

在完整规划链中，`pkg/planner/optimize.rs::buildPlan` 先清理会话级瞬时状态和 Plan/Column ID，然后调用 `NewPlanBuilder().withResultSetBuilder(...).Init(...)` 与 `PlanBuilder::Build`。本文件返回 `BuiltRuntimePlan`：普通结果集语句是 `Logical`，交由上层继续逻辑/物理优化；INSERT、UPDATE、DELETE 和 EXPLAIN 是已包装或已优化的 `NonLogical`，上层直接交给执行阶段。

## 核心职责

- 保存一次计划构建中的会话上下文、InfoSchema 快照、外层 schema/列名/CTE 栈、hint 状态、优化标志、查询块偏移、权限访问信息和 DML 模式等状态（`PlanBuilder`）。
- 通过 `PendingPlanBuilder` 把“配置”与“持有会话上下文的可用 builder”分开，支持注入 `ResultSetBuildFn` 和 `DataSourceProvider`，并在 `ResetForReuse` 后只保留可安全复用的配置。
- 以 `Build`/`BuildNodeRef` 作为 AST 顶层分派点：为 INSERT/REPLACE、UPDATE、DELETE、EXPLAIN 构建专用非逻辑计划，其他运行时结果集节点委托给 `logical_plan_builder_runtime::BuildBorrowedResultSetNode`。
- 提供 `RuntimeExplain`、`RuntimeExecute`、`RuntimeAnalyze`、`RuntimeSimple` 这些实现 `base::Plan` 的运行时包装，让执行器能保留 EXPLAIN 目标、prepared plan、ANALYZE 任务或原始事务控制 AST。
- 把与逻辑计划构建紧密相关的辅助协议放在统一边界：CTE 子查询标志、handle 作用域、join hint 命中、权限访问记录、全文匹配替代轮次信号和 DataSource 统计/访问路径填充。

## 主要符号

- `CteInfo`/`CteInfoRef`：用 `Rc<RefCell<_>>` 共享 `isBuilding` 和 `enterSubquery` 两个 CTE 构建标志；`prepareCTECheckForSubQuery` 返回本次被修改的引用，供 `logical_plan_builder_runtime::resetCTECheckForSubQuery` 恢复。
- `VisitInfo`：同时表示静态表权限和动态权限；`dynamic` 构造器将静态 privilege 留空，并记录动态权限列表、GRANT OPTION 和错误。
- `HandleColHelper`：当前 Rust 实现只维护作用域深度；`pushMap`/`popMap` 守护结果集或子查询的 handle 范围，`resetForReuse` 清空状态。
- `ResultSetBuildFn`：可注入的结果集计划构建函数指针；`NewPlanBuilder` 默认安装 `logical_plan_builder_runtime::BuildResultSetNode`。
- `DataSourceProvider`/`DataSourceProviderRef`：将已解析表的统计和访问路径填充隔离成 `Send + Sync` 注入点；`populateDataSource` 在安装 provider 时委托 `Populate`，未安装时保留默认伪统计路径。
- `BuiltRuntimePlan`：显式区分 `Logical(LogicalPlanRef)` 和 `NonLogical(Box<dyn base::Plan>)`，防止上层对 DML/EXPLAIN 重复走普通逻辑优化流程。
- `RuntimeExplain`：持有已优化的 `TargetPlan`、格式和 Analyze 标志；计划缓存克隆会先尝试物理计划克隆，再回退到普通 `clone_for_plan_cache`。
- `RuntimeExecute`：用 `Arc<dyn Plan>` 保持 prepared plan 生命周期，复制目标 schema，并支持缓存克隆。
- `RuntimeAnalyze`/`RuntimeSimple`：分别保留 `crate::Analyze` 和原始 `ast::NodeRef`；两者都显式拒绝计划缓存克隆。
- `PlanBuilder`：已初始化的构建状态；除了 `Build`/`BuildNodeRef`，还暴露访问信息、hint 状态、优化标志、FOR UPDATE 状态、FTS 信号和查询块偏移操作。
- `PendingPlanBuilder`：未持有 session/InfoSchema 的配置阶段；`noExecution`、`allowCastArray`、`withResultSetBuilder`、`withDataSourceProvider` 是链式选项，`Init` 才生成 `PlanBuilder`。

本文件没有条件编译项；唯一的全局状态是惰性初始化、只读的 `EMPTY_EXPLAIN_STATS`。

## 执行流程

1. 调用者以 `NewPlanBuilder` 获得 `PendingPlanBuilder`，可覆盖结果集 builder 或注入 DataSource provider，然后调用 `Init(ctx, info_schema, hint_processor)`。`Init` 暂存原 `PlannerSelectBlockAsName`，按 hint 处理器的最大 SELECT offset 建立新数组，并从 session variables 读取 semi-join rewrite 与 no-decorrelate 开关。
2. `PlanBuilder::Build` 从 `resolve::NodeW` 取出 `NodeRef`并进入 `BuildNodeRef`。`NodeRef::with_node` 保证在 AST 尚未被消耗时借用节点；已消耗则返回明确错误。
3. `BuildNodeRef` 按 AST 具体类型分派 INSERT、DELETE、UPDATE、EXPLAIN，并包装为 `BuiltRuntimePlan::NonLogical`；其他节点调用 `BuildBorrowedResultSetNode`并包装为 `Logical`。
4. `buildExplain` 要求存在目标语句，先构建逻辑结果集，再用 `DoOptimize` 生成物理计划，最后建立 `RuntimeExplain`。
5. `buildUpdate` 把 UPDATE 的 FROM/WHERE/ORDER/LIMIT/hints 投影成内部 SELECT，在构建期间暂时置 `inUpdateStmt` 和 `isForUpdateRead`。它用额外 projection 冻结输入列序，按原语句顺序重写 assignment，记录被引用列，禁用 projection elimination 后调用 `DoOptimizeForUpdate`，最后填充 `physicalop::Update` 并 `ResolveIndices`。
6. `buildDelete` 先拒绝 ORDER BY 中的 window function，再构造内部 SELECT。它遍历逻辑树收集 `DataSource`，按单表/多表目标过滤，只保留 handle 和索引维护必需列，建立 `TblColPosInfo` 布局。对单表 PK handle 的简单 `=`/`IN` 常量谓词，它可用 `PointGetPlan`/`BatchPointGetPlan` 替换常规优化结果，再包装 `physicalop::Delete`。
7. `buildInsert` 要求目标是实体表且已选库，通过 InfoSchema 取 `TableInfo`，拒绝 view/sequence，解析目标列并检查重复/未知/隐藏列。VALUES 路径重写每个标量且禁止复杂子查询；INSERT SELECT 路径构建并优化 SELECT，为 ON DUPLICATE 扩展 old-row/new-row schema，按顺序重写 assignment，再补齐受依赖影响的 generated columns，最后 `ResolveIndices`。
8. 普通逻辑结果回到 `optimize.rs::optimizeBuiltLogicalPlanRound` 执行 `DoOptimize`；非逻辑结果在 `optimize` 中直接返回。执行层还会在 `pkg/executor/statement_ru_result.rs`、`statement_ru_plan_walk.rs` 和 `compiler.rs` 中按具体 Runtime 类型下转处理。

## 数据与状态

`PlanBuilder` 是单次构建过程的可变状态中心。`outerSchemas`/`outerNames`/`outerCTEs` 和 `outerBlockExpand` 描述嵌套查询环境；`qbOffset`、`tableHintInfo`、`hintState` 将 hint 绑定到查询块；`colMapper`、`allNames`、`correlatedAggMapper` 服务于列解析和相关聚合；`visitInfo` 在构建期间累积权限检查材料，并由 `optimize.rs::buildPlan` 去重后写入 `StmtCtx` 的逻辑计划表集合。

`inUpdateStmt`、`inDeleteStmt`、`isForUpdateRead`、`inStraightJoin` 是影响下游构建决策的动态模式位。UPDATE/DELETE 构建在调用内部结果集 builder 前保存旧值，调用后立即恢复，因此出错也不会泄漏语句模式。`nonViableFTSMatch` 与 `predicateMatchSeen` 是上层多轮优化的信号；它们属于当前 builder，不是全局状态。

`RuntimeExecute::Plan` 使用 `Arc` 共享 prepared plan；CTE 标志使用单线程 `Rc<RefCell<_>>`；InfoSchema、PlanContext、DataSourceProvider 使用 `Arc`。`RuntimeExplain` 拥有其 `TargetPlan`。`ResetForReuse(self)` 消费已初始化 builder，丢弃本次 AST 相关状态，只携带可重用选项和 provider 回到 `PendingPlanBuilder`。

## 依赖与调用关系

上游主调用者是 `pkg/planner/optimize.rs::buildPlan`；直接表达式重写路径也会在 `pkg/planner/core/expression_rewriter.rs` 中初始化 builder。`pkg/planner/core/main_test.rs`、`integration_test.rs`、`logical_plans_test.rs` 以 `BuiltRuntimePlan` 区分逻辑和非逻辑结果。会话运行时在 `pkg/session/runtime/typed_adapter_bridge.rs` 初始化 builder，并在 prepared execution 路径创建 `RuntimeExecute`。

下游关系分为四类：

- AST 和名字解析：`crate::ast`、`resolve::NodeW`、`logical_plan_builder_runtime::BuildResultSetNode/BuildBorrowedResultSetNode`。
- 表与列元数据：`infoschema::InfoSchema`、`TableInfo2SchemaAndNames`、`DataSourceProvider::Populate`。
- 表达式与计划：`expression_rewriter::rewrite`、`rewriteInsertOnDuplicateUpdate`、`logicalop::{DataSource, LogicalProjection, LogicalTableDual}`、`physicalop::{Insert, Update, Delete, PointGetPlan, BatchPointGetPlan}`。
- 优化和规则：`DoOptimize`、`optimizer_runtime::DoOptimizeForUpdate`、`rule::FLAG_PRUNE_COLUMNS`、`rule::FLAG_ELIMINATE_PROJECTION`。

RustCodeGraph 索引能找到 `planbuilder_runtime.rs::NewPlanBuilder`、`PlanBuilder` 和 `RuntimeExplain`的定义，也把 `pkg/planner/optimize.rs::buildPlan` 识别为独立函数；但 `files --filter pkg/planner/core/planbuilder_runtime` 未返回文件，带路径的 callers/callees 查询未产生边输出。因此上述跨文件调用边又以调用点直接检索复核，不把缺失的图边当作“无调用者”。

## 错误处理与边界

公开构建入口统一使用 `Result<_, expression::Error>` 传播错误。`BuildNodeRef` 报告已被消耗的 AST；`buildResultSetNode` 报告未安装结果集 builder；`buildExplain` 报告缺少目标。名字和 DML 语义错误优先使用 `plannererrors` 中的结构化错误，尚无对应类型的边界则用 `expression::errors::New`。

当前实现的显式边界包括：UPDATE/DELETE with CTE 在未经 CTE builder 时拒绝；DELETE ORDER BY 不允许 window function，且必须能找到可更新基表；INSERT 目标不能是 join/query source、view 或 sequence，必须有当前数据库；列名不能重复或未知，值数必须匹配，generated column 只接受允许的 DEFAULT 语义；VALUES/SET 标量重写不接受会改变逻辑计划的复杂子查询。所有下游 expression、InfoSchema、优化和 `ResolveIndices` 错误保持原因向上传播。

`HandleColHelper::popMap`、query-block offset 的 `popSelectOffset`以及 Runtime plan 的 schema accessor 使用 `expect`；这些是内部栈/schema 已正确建立的程序不变量，不是 SQL 用户错误。新分支必须成对维护 push/pop 和 schema 初始化，否则会从可报告错误退化为 panic。

## 并发与资源生命周期

`PlanBuilder` 不是并发 builder：它需要 `&mut self`、含 `Rc<RefCell<CteInfo>>`，应在一条语句构建链内顺序使用。`DataSourceProvider` 要求 `Send + Sync`，因而可在不同 builder 之间以 `Arc` 共享，但 provider 内部仍必须自行保证并发安全。PlanContext 和 InfoSchema 也由 `Arc` 持有，其快照生命周期至少覆盖 builder 与由它创建的计划。

`RuntimeExecute` 以 `Arc` 保证 prepared plan 在语句 owner 构建和执行期间不被释放。`RuntimeExplain` 独占已优化目标计划，缓存克隆时产生新目标；`RuntimeAnalyze` 和 `RuntimeSimple` 不支持缓存克隆，避免跨语句复用时携带任务或事务控制状态。`EMPTY_EXPLAIN_STATS` 由 `LazyLock` 线程安全地初始化一次，各包装只返回共享不可变引用。

`ResetForReuse` 消费 `PlanBuilder` 而不是就地清理，这从类型层防止清理后继续误用旧 builder。它不保留权限、hint 栈、CTE、schema、DML 标志或 FTS 信号，只保留两个配置开关、函数指针和 provider。

## 与 Go 版本的对应关系

主要 Go 对照是 `pkg/planner/core/planbuilder.go`，DML 的 UPDATE/DELETE 对照还在 `pkg/planner/core/logical_plan_builder.go`。Rust 的 `PlanBuilder`、`VisitInfo`、`CteInfo`、`HandleColHelper`、`NewPlanBuilder`、`Init`、`ResetForReuse`、`Build`、`buildInsert`、`buildUpdate`、`buildDelete`、`buildExplain` 都能对应 Go 同名概念或函数。Rust 保留了重要语义：从 session 初始化 no-decorrelate/semi-join 开关，UPDATE assignment 顺序重写，DELETE 列裁剪与 handle/索引布局，INSERT SELECT 的 old/new row schema，ON DUPLICATE 和 generated-column 依赖，以及 EXPLAIN 在 builder 边界完成目标优化。

两版不是结构上一对一的完整复制：

- Go `PlanBuilder::Build` 直接分派管理、DDL、PREPARE/EXECUTE、SHOW、ANALYZE 等大量语句；本 Rust 文件只专项处理 INSERT/UPDATE/DELETE/EXPLAIN，其他已支持的结果集语句由 `logical_plan_builder_runtime.rs` 处理，非结果集能力不应从 Go 实现推定为 Rust 已支持。
- Go `cteInfo` 含完整的递归计划、统计、storage ID、inline 策略和 consumer count；本文件的 `CteInfo` 只承担子查询可见性检查所需的两个标志，更完整的 Rust CTE 环境在 `logical_plan_builder_runtime::CteEnvironment`。
- Go `handleColHelper` 保存 table ID 到 handle columns 的 map 栈，可合并并读取尾部 map；本 Rust `HandleColHelper` 仅保存作用域计数。Rust DELETE 直接从已构建的 `DataSource` 树收集 handle 与列布局，不可把这一精简误解为 Go helper 能力已完整移植。
- Go builder 保留 expression-rewriter pool 和更多 view/partition/CTE/lateral 状态；Rust 注释明确说每次 rewrite 拥有已重置的本地 rewriter，因此不含 pool。
- Go `GetOptFlag` 还会在 sampling 时返回 0；本 Rust 实现直接返回 `optFlag`。扩展 sampling 语义时需专门对齐，不应假设现有 getter 已等价。

Rust 直接回归 `pkg/planner/core/planbuilder_runtime_test.rs::init_uses_the_session_no_decorrelate_setting_like_go` 只验证了 Init 的 no-decorrelate 对齐。更广的 Go 语义证据来自 `planbuilder_test.go`、`logical_plans_test.go` 和 `integration_test.go`，其中包括 INSERT 权限、UPDATE 等值条件、DELETE 列裁剪、EXPLAIN ANALYZE DML 和嵌套虚拟 generated-column UPDATE。

## 扩展指南

- 新增顶层 AST 语句类型时，先决定其产物是需要上层继续优化的 `Logical`，还是已组装的 `NonLogical`。只有后者才应在 `BuildNodeRef` 增加专用分支；结果集逻辑应优先放入 `logical_plan_builder_runtime.rs`。
- 扩展 DML 时必须与 Go 对应 builder 逐分支核对，尤其是 schema/output names、handle 布局、generated columns、权限 `VisitInfo`、FOR UPDATE 状态、hint/optFlag 以及 `ResolveIndices` 时机。不得用“能生成计划”代替这些执行器合约。
- 新增临时 builder 状态时，要在成功和错误路径上对称恢复，并审查 `ResetForReuse` 是否应保留它。语句相关、AST 相关或可变累积状态默认不应保留。
- 扩展 `DataSourceProvider` 时保持 `Send + Sync`、InfoSchema 快照语义和默认伪统计回退；新 provider 错误必须传播，不应静默产生不完整 access path。
- 扩展 Runtime plan 时必须实现完整 `base::Plan` 边界，明确 schema/output names、统计、context、noncacheable reason 和 plan-cache clone 策略；同时检查 `flat_plan.rs`、executor RU 结果/计划遍历和 compiler 中的类型分派。
- 回归测试不要内嵌到本文件。初始化和局部状态对齐放在 `planbuilder_runtime_test.rs`；真实 parser AST 与 provider 边界放在 `real_result_set_builder_aster_unit_test.rs`；完整 DML/EXPLAIN 路径放在 `integration_test.rs` 或邻近独立测试文件，并与 Go 对应测试保持语义一致。

主要风险是 DML 列布局与 executor 不兼容（正确性）、Go/Rust 分派差异导致功能误判（兼容性）、过早增加/删除 projection 或列使优化和索引维护退化（性能与正确性），以及在多轮优化间泄漏 builder/session 状态（稳定性）。

## 验证依据

源码与配置证据：

- `pkg/planner/core/planbuilder_runtime.rs`：完整阅读 1–1871 行，核对全部模块级类型、trait、常量、impl 和函数；文件无 `cfg` 分支。
- `pkg/planner/core/lib.rs`：确认模块声明、测试文件独立声明和 `pub use planbuilder_runtime::*`。
- `pkg/planner/core/Cargo.toml`：确认 crate 名、`lib.rs` 入口、`nextgen` feature 以及 base/expression/hint/infoschema/logicalop/physicalop/resolve/rule 等直接依赖；本文件没有自身 feature gate。
- `pkg/planner/optimize.rs`：确认 `buildPlan -> NewPlanBuilder/Init -> Build`、`BuiltRuntimePlan` 分派、权限表信息写入和逻辑计划后续优化。
- `pkg/planner/core/logical_plan_builder_runtime.rs`：确认默认结果集 builder、借用 AST 构建入口、CTE 环境和 reset 配合点。
- `pkg/executor/statement_ru_result.rs`、`pkg/executor/statement_ru_plan_walk.rs`、`pkg/executor/compiler.rs`、`pkg/planner/core/flat_plan.rs`：确认 Runtime plan 在执行和扁平化阶段的真实消费者。

RustCodeGraph 证据：`status` 报告索引可用（11467 文件、307296 节点、1848419 边）；`query NewPlanBuilder --kind function` 找到 Go、旧 Rust `planbuilder.rs` 和本文件三个定义；`node NewPlanBuilder` 返回本文件 756–759 行源码；`query RuntimeExplain --kind struct` 定位到本文件 137 行；`query PlanBuilder --kind struct` 定位到 577 行；`query buildPlan --kind function` 定位到 `pkg/planner/optimize.rs:1509`。`files --filter` 和带路径 callers/callees 未返回目标边，所以调用点另用直接源码搜索核验。

测试证据：`pkg/planner/core/planbuilder_runtime_test.rs` 验证 Init 从 session 传递 no-decorrelate；`real_result_set_builder_aster_unit_test.rs` 验证 parser AST 的 SELECT 构建、真实表解析和 provider 调用；`integration_test.rs::test_insert_select_planbuilder_runtime`、`test_explain_analyze_dml2`、`test_explain_analyze_dml_commit`、`test_nested_virtual_generated_column_update` 覆盖关键 DML/EXPLAIN 路径；`logical_plans_test.rs::unique_key_and_delete_fixtures_are_typed_and_reach_real_planner_paths` 覆盖 DELETE 真实路径；`flat_plan_test.rs` 覆盖 Runtime EXPLAIN/ANALYZE 的后续消费。Go 对照读取了 `planbuilder.go`、`logical_plan_builder.go`、`planbuilder_test.go`、`logical_plans_test.go` 和 `integration_test.go`。

本任务是纯文档分析，按计划不运行 Cargo 或 Go 测试；验收依赖结构检查与上述静态事实复核。
