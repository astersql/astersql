# `pkg/session/runtime/explain_query.rs`

源码：[explain_query.rs](./explain_query.rs)

## 文件定位

本文件属于 `astersql-session` crate 的具体会话运行时。crate 入口是 `pkg/session/lib.rs`，`pkg/session/runtime.rs` 以私有模块 `mod explain_query;` 装入本文件，并通过 `use explain_query::*;` 把其中的 `pub(super)` 辅助函数提供给同一 `runtime` 模块的其他实现文件。它不是独立执行器，也不定义 SQL 语法；它位于解析后的 AST、会话状态、优化器物理计划与 EXPLAIN 文本结果之间。

直接入口可从三个位置复核：`pkg/session/runtime/explain_select.rs::explain_relational_select` 对点查调用 `explain_point_get_select`，对禁止求值的标量子查询调用 `explain_scalar_subquery_plan`；`pkg/session/runtime/explain_analyze.rs::explain_analyze_relational_select` 复用标量子查询路径；`pkg/session/runtime/dispatch.rs` 分别为 UPDATE/DELETE 点查调用 `explain_point_get_dml`，并从通用 EXPLAIN 分派进入关系 SELECT。复杂关系查询最终由本文件的 `ConcreteSession::explain_optimized_relational_select` 构建真实逻辑/物理计划。

## 核心职责

1. 识别 EXPLAIN 所需的谓词结构：`equality_columns`、`flatten_and`、`flatten_or`、`explain_range`、`explain_indexed_branch_matching` 等从 AST 中提取连接键、范围和候选索引。
2. 复用真实规划链：`explain_optimized_relational_select` 与 `explain_scalar_subquery_plan` 都构造 `SessionDomainDataSourceProvider`，调用 `astersql_planner_core::NewPlanBuilder`、`buildResultSetNode` 和 `DoOptimize`，避免用固定桩代替优化器结果。
3. 把 `PhysicalPlan` 递归渲染为 TiDB 风格计划树：`explain_scalar_physical_tree_with_lookup_context` 维护树形缩进、root/cop/MPP 任务、Build/Probe 角色、估算行数或成本、访问对象和算子信息。
4. 稳定公开文本：`normalize_scalar_query_column_ids`、`normalize_relational_plan_ids`、`shift_explain_marker_ids`、`remap_explain_marker_id` 等隔离内部计划 ID、列 ID、CTE 存储 ID 的分配差异；若某些已知回归形态无法仅靠通用渲染对齐，函数内还保留窄范围兼容分支。
5. 处理 CTE、标量子查询、LATERAL、索引连接和点查等特殊形态，并将最终字符串交给 `explain_plan_tree_rows` 转成 `ConcreteRecordSet`。

## 主要符号

- `ScalarSubqueryRawRow`、`ScalarSubqueryRequestContext`、`SCALAR_SUBQUERY_RESULTS`：保存 EXPLAIN ANALYZE 规划期间预取的标量子查询首行。队列按规划器回调顺序消费，`last` 允许同一结果被重复请求。
- `eval_session_scalar_subquery_first_row`：规划器求值钩子；校验返回列数与物理计划 schema 一致，并按 `column.GetType(eval)` 把文本转换为 `Datum`。无上下文、队列耗尽、列数不符或类型转换失败均返回表达式错误。
- AST 辅助函数：`equality_columns` 仅接受列到列的 `=`/`==`；`has_index_equality` 递归寻找索引首列等值项；`flatten_and`/`flatten_or` 展平布尔树；`empty_strict_integer_interval` 用 `i128` 差值判断相邻严格整数边界，避免 `i64` 溢出；`explain_range` 支持字面量、NULL 和会话用户变量；`explain_indexed_branch_matching` 在同首列多个索引中应用调用者过滤器。
- `explain_plan_tree_rows`：把计划文本包装为记录集。具体列拆分行为由 `ConcreteRecordSet::new` 实现；本文件的独立测试验证树形前缀仍留在算子列且拼接后可还原原行。
- ID 稳定化函数：`normalize_scalar_query_column_ids`、`render_scalar_query_column_ids`、`normalize_relational_plan_ids`、`account_for_cte_scalar_columns`。它们只重写展示文本，不改变物理表达式身份。
- 物理树渲染入口：`explain_scalar_physical_tree`（短格式）、`explain_scalar_physical_tree_with_costs`（成本信息）、`explain_cbo_physical_tree_with_costs`（估算行数）最终汇入 `explain_scalar_physical_tree_with_role` 和私有递归器 `explain_scalar_physical_tree_with_lookup_context`。
- `explain_physical_plan_cost`：把任务名映射为 Root、CopSingleRead、MPP task，优先调用 `GetCanonicalPlanCostVer1`；成本无效时以非负 `RowCount` 回退。
- CTE/投影适配：`preserve_mpp_cte_seed_output_projection` 保住 MPP CTE 聚合种子的输出 schema；`collapse_scalar_aggregate_explain_projections` 与 `collapse_inlined_cte_scan_projections` 消除 Rust 优化路径额外产生、但 Go 展示不保留的纯适配投影。
- 表名限定函数族：`qualify_explain_table_source`、`qualify_explain_join`、`qualify_explain_select_tables` 递归展开视图并补真实目录表的默认库名，同时保留未限定的 CTE 名供 CTE 解析器处理。
- 主入口：`explain_optimized_relational_select`、`explain_scalar_subquery_plan`、`explain_relational_join`、`explain_point_get_select`、`explain_point_get_dml`。
- 其他策略函数：`result_set_contains_lateral`/`node_contains_lateral`/`select_contains_lateral` 递归识别 LATERAL；`parallel_apply_concurrency` 读取并规整并行 Apply 并发度；`optimizer_fix_control_enabled` 解析 `编号:布尔值` 修复开关。

## 执行流程

### 普通复杂关系 EXPLAIN

1. `explain_optimized_relational_select` 清空本语句的标量子查询注册表，解析输入并要求恰好一个语句；若外层是 `ExplainStmt`，取出其子语句并要求为 `SelectStmt`。
2. 收集源表原始大小写和 `<=>` 两侧顺序；从 EXPLAIN 子 SQL 生成 binding 匹配键，通过 `MatchSQLBinding` 查找绑定，并把绑定 SQL 的表 hint 与 query-block offset 复制到 SELECT AST。
3. `qualify_explain_select_tables` 为目录表补库名、展开视图，并递归处理 WITH/集合操作。
4. `plan_context_with_params_and_explain` 创建 EXPLAIN 规划上下文；重置 `PlanID`/`PlanColumnID`，再由 `NewPlanBuilder`、`buildResultSetNode`、`DoOptimize` 得到逻辑计划和物理计划。构建期间新增的 planner warning 被映射为会话 warning 1815。
5. 对 CTE 生产者单独优化：递归 CTE分别优化 seed/recursive part；非递归 MPP CTE走 `LogicalOptimizeForMpp`、`PhysicalOptimizeForMpp`，并用 `preserve_mpp_cte_seed_output_projection` 保持 schema。暂时从 `LogicalCTE` 取走的计划在所有正常结果路径上写回。
6. 根据 `plan_tree`、`cost_trace`、`verbose` 或默认格式选择渲染器；随后追加 CTE producer 和已注册的 `ScalarSubqueryEvalCtx` 子树。
7. 最后统一 CTE/列 ID、源表大小写、NULL-safe equality 顺序及少量浮点/动态范围文本，再返回 `ConcreteRecordSet`。

### 标量子查询 EXPLAIN

1. `explain_scalar_subquery_plan` 区分普通 EXPLAIN 与 EXPLAIN ANALYZE。后者先由嵌套的 `prepare_select`/`prepare_expression` 深度遍历 WHERE、HAVING、投影及 CTE，实际执行标量子查询并缓存至多一行；多于一行返回 `Subquery returns more than 1 row`。
2. 安装 `eval_session_scalar_subquery_first_row` 后，把请求数据放入线程局部槽；同时快照并清空会话的标量子查询注册表。
3. 使用同一个 `PlanContext` 构建和优化主树，使主查询与构建时注册的标量子查询共享计划 ID 序列。单次引用、非递归 CTE 可在 AST 层内联后重新规划，再折叠只为类型/schema 适配而产生的投影。
4. 渲染主树，再按注册顺序追加 `ScalarSubqueryEvalCtx` 的标题和子树。代码针对空输入、EXISTS/NOT EXISTS、Limit/Selection、Cascades 和内联 CTE 调整公开估算值与列号，但不修改被渲染的表达式语义。
5. 闭包结束后无论成功失败都恢复线程局部槽和原注册表；若失败时留下新的嵌套注册项，再清空它们，维持普通会话清理不变量。

### 连接与点查快捷路径

`explain_relational_join` 从两侧表源、ON 条件、hint、谓词传播和表元数据推导连接展示；对可直接确认的 IndexJoin/MergeJoin/HashJoin 形态产生兼容行，否则组合投影、左右扫描和过滤条件。`explain_point_get_select` 对单列主键点查生成 `Point_Get` 或由修复开关 52592 控制的 `TableReader + TableRangeScan`；无符号主键负值直接为 `TableDual rows:0`。`explain_point_get_dml` 为 UPDATE/DELETE 增加顶层算子，并按内核代际与同一修复开关决定 `Point_Get lock`、`SelectLock` 或范围扫描形态。

## 数据与状态

- 输入数据主要是 `ast::SelectStmt`/`ast::ExprNode`、`TableInfo`/`IndexInfo`、`PhysicalPlan` trait object、原始 SQL 文本和 `ConcreteSession` 的 `domain`、`session_vars`、`state`、`bindings`。
- 输出统一为 `ConcreteRecordSet`；计划先累积为 `Vec<String>`，再由 `explain_plan_tree_rows` 转换成行列结果。
- `session_vars` 承载 `PlanID`、`PlanColumnID`、`StmtCtx` warning、隔离读取引擎、ScalarSubQueries 注册表以及 EXPLAIN 相关系统变量。标量路径明确快照/恢复注册表；复杂关系路径在语句开始清空注册表，并在渲染尾部读取它。
- `domain` 提供 info schema、表/索引元数据和统计信息；扫描渲染根据 `StatsVersion` 或 stats handle 中是否存在有效统计决定是否追加 `stats:pseudo`。
- `SCALAR_SUBQUERY_RESULTS` 是线程局部 `RefCell<Option<Arc<...>>>`。其中两个 `Mutex` 分别保护待消费队列和最后一次结果；毒化锁通过 `PoisonError::into_inner` 继续取回数据。
- 计划树渲染自身以局部 `Vec`、克隆的物理节点和递归参数传递状态，没有持久化新的全局计划对象。

## 依赖与调用关系

上游调用链可由以下真实调用点确认：

- `pkg/session/runtime/dispatch.rs` → `explain_point_get_dml` / `explain_relational_select`。
- `pkg/session/runtime/explain_select.rs::explain_relational_select` → `explain_point_get_select` / `explain_scalar_subquery_plan`，并在通用路径中使用本文件的优化/渲染辅助。
- `pkg/session/runtime/explain_analyze.rs::explain_analyze_relational_select` → `explain_scalar_subquery_plan`。
- `pkg/session/runtime/source.rs` → `explain_query::flatten_and`，说明谓词展平也被相邻关系数据源逻辑复用。

主要下游依赖为：`astersql-parser`/`astersql-parser-ast` 负责重解析和 AST；`astersql-planner-core`、`-base`、`operator-logicalop`、`operator-physicalop` 负责 PlanBuilder、优化、计划 trait 与具体算子；`astersql-expression`/`astersql-types` 负责表达式展示和 Datum 转换；`astersql-infoschema`、`astersql-meta-model` 与 `SessionDomainDataSourceProvider` 提供目录和表/索引元数据；`astersql-bindinfo` 与 `astersql-util-hint` 提供 binding/hint 对齐。这些均在 `pkg/session/Cargo.toml` 中是直接 path dependency；`nextgen` feature 只把 nextgen 传给部署/内核类型 crate，点查 DML 通过 `astersql_config_kerneltype::IsNextGen()` 观察该模式。

RustCodeGraph 的文件索引确认 `pkg/session/runtime/explain_query.rs` 已纳入图，符号查询确认 `explain_scalar_subquery_plan` 定义在本文件并在 `explain_select.rs` 有调用点。`callers/callees` 查询在本次环境中无输出超时，因此上面的边以相邻源码调用点交叉核验，不把图中未返回的边写成结论。

## 错误处理与边界

- 解析入口要求单语句且子节点类型正确；空 EXPLAIN 子语句、非 SELECT、非法视图定义或不支持的 CTE query 类型都转为带上下文的 `SessionError`。
- planner/expression 错误通过 `session_error("阶段", error)` 加入阶段说明，例如构建关系计划、优化标量计划、克隆 CTE 投影。
- 标量子查询严格限制最多一行，并验证列数等于计划 schema；NULL 用默认 `Datum` 表示，非 NULL 文本按目标字段类型转换。
- 表、索引、谓词或 AST 形态不足时，多数快捷识别器返回 `None`，让调用者进入更一般路径；例如复合主键不走 `explain_point_get_select` 快捷路径。
- `explain_physical_plan_cost` 只认识 root、cop[tikv]、mpp[tiflash]；未知任务返回 `None`。成本非有限值时仅在行数有限时回退。
- 本文件包含不少由 SQL 文本或已知计划形态触发的窄兼容分支。新增分支必须保持条件足够具体，避免在通用优化器输出前错误截获其他查询。
- `explain_scalar_subquery_plan` 对某类本应被优化器短路、但预取时出现 `Unknown column` 的嵌套分支写入 `None` 而不是提前失败；其他执行错误仍传播。

## 并发与资源生命周期

此文件不创建异步任务或工作线程。主要生命周期风险来自临时会话状态和递归计划所有权：

- 标量子查询结果用线程局部槽隔离同步规划请求，槽内 `Arc` 允许求值钩子持有共享上下文，两个 `Mutex` 提供内部可变性。该设计依赖规划和求值发生在同一线程局部上下文；跨线程传播不由本文件提供。
- `explain_scalar_subquery_plan` 在执行闭包前保存旧 TLS 值与 ScalarSubQueries 快照，闭包后显式恢复；错误路径也执行恢复，并清理构建期间新注册的上下文。
- CTE 优化会暂时 `take()` `SeedPartLogicalPlan`/`RecursivePartLogicalPlan`，完成后把计划与 `SeedPartLogicalOptimized` 写回 `LogicalCTE`。递归收集使用原始指针，但注释和作用域约束表明：递归借用在下探前释放，逻辑树一直由当前函数持有，指针仅立即使用。
- 渲染物理树通过 `clone_physical` 或具体算子 `Clone` 制作展示用副本；投影折叠和 MPP CTE schema 修正不夺走会话中其他持有者的计划对象。
- `parallel_apply_concurrency` 只读取会话状态并返回至少为 1 的并发配置，本文件本身不启动 Apply worker。

## 与 Go 版本的对应关系

`pkg/session/Cargo.toml` 的 `[package.metadata.porting]` 明确把该 crate 对应到 Go 包 `pkg/session`，但本文件没有一个同路径、同粒度的 Go 文件。语义由多个 Go 层共同承担：

- `pkg/planner/core/planbuilder.go::buildExplain` 对 EXPLAIN 子语句调用真实优化器，再由 `buildExplainPlan` 构造 `core.Explain`；Rust 的 `explain_optimized_relational_select`/`explain_scalar_subquery_plan` 对应“构建真实目标计划”这一职责。
- `pkg/executor/explain.go::ExplainExec` 在 `Next` 首次生成结果；EXPLAIN ANALYZE 先完整驱动并关闭目标执行器，再调用 `RenderResult`。Rust 普通复杂 EXPLAIN主要复用规划器并渲染物理树；标量 EXPLAIN ANALYZE 只在本文件范围内预取标量子查询首行，完整执行统计的其他部分位于 `explain_analyze.rs`，不能把本文件单独等同于 Go `ExplainExec`。
- `pkg/planner/core/scalar_subq_expression.go::ScalarSubQueryExpr::String` 和 `ScalarSubqueryEvalCtx::ExplainInfo` 产生 `ScalarQueryCol#<id>` 与 `Output: ...`；Rust 的 ID 规范化与 ScalarSubQueries 遍历以这些公开文本契约为对齐目标。
- Go 物理算子的 `ExplainInfo` 分散在 `pkg/planner/core/operator/physicalop/*.go`；Rust 递归器读取对应 Rust `PhysicalPlan::explain_info()`，再补任务、角色、成本、访问对象和稳定化处理。

差异点是 Rust 文件仍包含较多按 SQL/计划形态进行的兼容输出和 ID 修正，而 Go 通常由 `core.Explain.RenderResult` 与各物理算子直接生成。维护时应优先扩大真实 planner/renderer 的一般能力，不能把这些兼容分支误写成新的优化规则。

## 扩展指南

- 新增物理算子展示：优先扩展 `explain_scalar_physical_tree_with_lookup_context` 的算子 downcast、任务传播和子节点角色逻辑；同时确认普通、`plan_tree`、`verbose`/`cost_trace` 三类格式是否需要不同成本或 ID 处理。
- 新增 CTE/标量子查询行为：修改 `explain_optimized_relational_select` 或 `explain_scalar_subquery_plan` 时，必须保持计划注册表、TLS 上下文、临时取出的 CTE 计划和计划 ID 序列在成功/失败两条路径都恢复。
- 新增谓词或索引识别：在 `flatten_*`、`explain_range`、`explain_indexed_branch_matching` 或 `explain_join_constraints` 中接入；注意组合索引只以首列判定的现有约束，以及同首列多个索引不能先选后过滤。
- 新增点查代际差异：接入 `explain_point_get_select`/`explain_point_get_dml`，明确与 optimizer fix control 和 `IsNextGen` 的交互。
- 测试应继续放在独立文件 `pkg/session/runtime/explain_query_test.rs`，不要内嵌到生产源文件；跨模块 EXPLAIN/ANALYZE 行为还应同步相邻 Rust 测试或 Go 的 `pkg/executor/explain_test.go`、`pkg/executor/explain_unit_test.go` 与相关 planner casetest。
- 性能风险主要来自重复解析/重复规划、深树递归克隆、统计锁读取和大量字符串替换；兼容性风险主要是输出列号、树形缩进、Build/Probe 顺序、CTE producer 顺序和 Go golden 文本漂移。

## 验证依据

- 源文件与模块：`pkg/session/runtime/explain_query.rs`（完整函数清单及关键实现）、`pkg/session/runtime.rs`（模块声明和可见性）、`pkg/session/runtime/explain_select.rs`、`pkg/session/runtime/explain_analyze.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/source.rs`（直接调用点）。目标目录不存在 `doc.go`，因此没有额外包契约文件可读。
- crate 边界：`pkg/session/Cargo.toml` 的 package、`nextgen` feature、`package.metadata.porting.go-package` 及 planner/expression/parser/infoschema/meta/bindinfo/hint 直接依赖。
- 独立 Rust 测试：`pkg/session/runtime/explain_query_test.rs` 覆盖计划树列拆分与缩进、HashJoin 键顺序、`i64` 全域严格区间不溢出、任务范围内的等值条件换序和生成列键顺序；同文件也调用相邻 `explain_analyze` 的 read-pool/RU 辅助测试。
- Go 对照：`pkg/planner/core/planbuilder.go::{buildExplain,buildExplainPlan}`、`pkg/executor/explain.go::{ExplainExec,generateExplainInfo}`、`pkg/planner/core/scalar_subq_expression.go::{ScalarSubQueryExpr::String,ScalarSubqueryEvalCtx::ExplainInfo}`，以及 `pkg/executor/explain_test.go`、`pkg/executor/explain_unit_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标文件有 832 个符号；`files --filter pkg/session/runtime` 确认目标和独立测试均被索引；`query explain_scalar_subquery_plan --json` 返回本文件定义及 `explain_select.rs` 调用位置。自然语言 `explore` 与 `callers/callees` 在本次运行中无输出超时，故未将缺失图边当作事实。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文恰有规定的 11 个二级标题；交付前还需检查 Git 差异只包含本文，并人工复核每个主要结论可追溯到上述符号或路径。
