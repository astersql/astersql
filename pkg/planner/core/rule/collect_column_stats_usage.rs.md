# `pkg/planner/core/rule/collect_column_stats_usage.rs`

## 文件定位

本文件对应真实源文件 [`collect_column_stats_usage.rs`](collect_column_stats_usage.rs)，位于 `astersql-planner-core-rule` crate 的逻辑优化规则层，由同目录 `lib.rs` 通过 `pub mod collect_column_stats_usage` 暴露。它读取 `rule_init.rs` 定义的精简逻辑计划 IR（`Plan`、`PlanKind`、`Expr`、`AggKind`），在优化阶段统计哪些表和列可能需要统计信息，并把结果返回给上层规则；它本身不加载统计、不估算基数，也不改写计划。

生产侧直接入口在 `rule_collect_plan_stats.rs` 的 `CollectPredicateColumnsPoint::optimize`：该规则调用 `collect_column_stats_usage`，再根据 `visited_tables` 和 `predicate_columns` 为每张表构造 `UsedStats` 并写回 `plan.used_stats`。因此本文件处于“逻辑计划已经形成 → 统计加载/使用决策”之间。`Cargo.toml` 将该目录声明为 `astersql-planner-core-rule`，本文件所用计划类型是 crate 内部模块；清单中的路径依赖服务于规则 crate 的其他模块，并没有为本文件设置独立 feature。

## 核心职责

`collect_column_stats_usage` 对一棵 `Plan` 做一次同步深度优先遍历，汇总五类信息：表达式引用的列及其 full/meta 需求、访问过的逻辑表、静态裁剪选择的物理分区、访问的算子数，以及可帮助索引裁剪的 WHERE/JOIN/ORDER BY/GROUP BY/MIN/MAX 列。

统计需求分两级。DataSource 自身的 `plan.predicates` 代表已下推条件，调用 `add_expression(..., true)`，需要完整统计；Join、Sort、Selection、GROUP BY 等位置调用 `add_expression(..., false)`，只要求 NDV 等元信息。同一列重复出现时，`add_expression` 对旧值与新值做逻辑 OR，因而 full 需求一旦出现不会被后续 meta 需求降级。

该实现是 Go 收集器在精简 Rust IR 上的可运行子集，不等同于 Go 完整逻辑算子实现。尤其是列仅由 `i64` 标识，没有 Go 的 `(table_id, column_id)` 键和输出列血缘图；有关能力边界必须结合“与 Go 版本的对应关系”理解。

## 主要符号

- `pub struct ColumnStatsUsage`：一次遍历的值对象，派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`。`predicate_columns: BTreeMap<i64, bool>` 保存列 ID 到 full 标志；`visited_tables: BTreeSet<i64>` 保存逻辑表 ID；`table_partitions: BTreeMap<i64, BTreeSet<i64>>` 保存表到物理分区 ID；`operator_count: usize` 计数计划节点；`interesting_columns: BTreeMap<i64, BTreeSet<i64>>` 保存每表的索引裁剪候选列。
- `pub fn collect_column_stats_usage(plan: &Plan, collect_index_pruning_columns: bool) -> ColumnStatsUsage`：公开无错误返回入口。它创建默认结果，以空 Join/排序上下文调用 `collect`，最后按值返回汇总。
- `fn collect(plan, usage, join_columns, ordering_columns, collect_indexes)`：核心递归。它先增加算子计数，再按 `PlanKind` 收集当前节点信息；Join、Sort 和开启索引收集的 Aggregation 自行带新上下文递归并提前返回，其他分支走统一的子节点递归。
- `fn add_expression(usage, expression, full)`：通过 `Expr::columns` 展开表达式树中的列，向 `predicate_columns` 插入或升级需求。常量不产生列；Scalar/Cast 的递归展开逻辑定义在 `rule_init.rs`。

文件没有模块级常量、trait、条件编译项或异步函数；公开 API 只有结果结构及顶层收集函数，其余两函数是文件内部实现。

## 执行流程

1. `collect_column_stats_usage` 初始化空的 `ColumnStatsUsage`，以空 Join/排序列切片进入根节点。
2. 每次 `collect` 调用先令 `operator_count += 1`，保证包括叶子和不识别的 `Other` 在内，每个实际访问节点恰好计数一次。
3. `DataSource` 登记 `table_id`。若同时存在 `partition` 与 `selected_partitions`，它把分区定义下标安全映射成定义中的物理 ID；越界下标由 `filter_map` 忽略。开启索引裁剪收集时，它只接纳属于本 DataSource schema 的祖先 Join/排序列，并接纳“其全部列均属于本 schema”的本地谓词列。最后将 DataSource 谓词标为 full。
4. `Join` 合并等值和其他条件中的列作为新的 Join 上下文，将条件列标为 meta，然后把同一上下文分别传给所有子树。该分支自行递归并 `return`，避免统一尾递归再次访问子节点。
5. `Sort` 将 `by` 表达式列标为 meta，并用当前排序列替换传向子树的排序上下文；它同样自行递归并提前返回。
6. `Aggregation` 将 GROUP BY 列标为 meta；只有 `distinct` 聚合参数按 GroupNDV 契约登记为 meta。开启索引收集时，它把 GROUP BY 列以及 MIN/MAX 参数列组成排序上下文，传给子树后提前返回；关闭时则保留父级上下文，走统一尾递归。
7. `Selection` 把附着谓词标为 meta。Projection、Limit、UnionAll、PartitionUnion、TableDual 和 Other 仅处理节点上通用的 `plan.predicates`，再递归子节点。
8. 遍历结束后返回结果；生产调用者 `CollectPredicateColumnsPoint::optimize` 再按 DataSource schema 将全局列集合限制到对应表并填充 `UsedStats.full_load`。

## 数据与状态

所有可变状态都集中在本次调用栈持有的 `ColumnStatsUsage` 中，没有全局或线程局部状态。`BTreeMap`/`BTreeSet` 同时提供去重和确定性顺序，便于测试及稳定输出；同一表多次出现时，分区和 interesting columns 通过 `entry(...).or_default().extend(...)` 做并集。

`join_columns` 与 `ordering_columns` 是递归参数，描述祖先算子对当前子树提出的索引候选需求。Join 用新 Join 列替换此前的 Join 上下文，Sort 用新排序列替换此前的排序上下文；Aggregation 在索引模式下以 GROUP BY/MIN/MAX 列替换排序上下文。这是当前代码的真实传播策略，而不是累加所有祖先同类列。

列身份只有 `i64`。`predicate_columns` 因而是整棵计划的全局列 ID 集合；真正按表归属的过滤由上游 `rule_collect_plan_stats.rs::columns_for_table` 利用各 DataSource 的 schema 完成。若不同表复用同一列 ID，精简 IR 的表达能力弱于 Go 的 `model.TableItemID`，扩展时不能假定这里已经具有完整表列身份。

## 依赖与调用关系

上游模块装配关系是 `rule/lib.rs → collect_column_stats_usage`。已接线的生产调用边是 `CollectPredicateColumnsPoint::optimize → collect_column_stats_usage → collect → add_expression/Expr::columns`；`collect` 还会递归调用自身。直接 Rust 测试调用者包括 `collect_column_stats_usage_test.rs`、`rule_aster_unit_test.rs` 和 `pkg/planner/core/casetest/stats_test.rs`。

本文件直接依赖 `rule_init::{AggKind, Expr, Plan, PlanKind}` 和标准库 `BTreeMap/BTreeSet`。下游语义分别来自：`Expr::columns` 的表达式列展开、`PlanKind` 的算子分派、`Plan.schema` 的 DataSource 列归属判断、`PartitionInfo.definitions` 的分区下标到物理 ID 映射。`indexes` 字段被显式忽略；开启索引裁剪只收集候选列，不在此选择或裁剪索引。

RustCodeGraph 的文件节点把目标文件识别为 7 个索引符号，并能展示源文件和相关测试；其 `callers`/`callees` 子命令在本次检查中对精确函数节点持续无输出至超时。因此调用边另由仓库引用搜索和调用点源码核实，不能把图命令无输出解释为没有调用者。

## 错误处理与边界

公开函数不返回 `Result`，当前 IR 遍历没有可恢复错误源。分区数据不完整时，只有 `partition` 和 `selected_partitions` 同时存在才记录分区；选中下标越界会被忽略，不会 panic。表达式不含列时不产生记录，集合自然消除重复列和重复分区。

本文件不验证计划树无环；`Plan` 以拥有式 `Vec<Plan>` 表示正常情况下天然形成有限树，递归深度仍受线程栈限制。它也不检查无效表 ID、非正列 ID或 schema 内重复 ID。与 Go 不同，当前 Rust DataSource 分支没有系统表过滤，也没有显式跳过 `_tidb_rowid` 一类非持久列；调用者必须只提供符合精简 IR 契约的计划，或在扩展 IR 时补上对应规则与测试。

`interesting_columns` 的本地谓词判定要求表达式中的所有列都属于当前 DataSource；跨表表达式不会被当成本地 WHERE 候选。Join/排序候选则逐列以 schema 过滤。普通非 DISTINCT 聚合参数不会自动成为谓词列，避免仅因索引存在或聚合读取列就触发统计需求。

## 并发与资源生命周期

遍历完全同步，函数只借用不可变 `&Plan`，结果累加器仅由当前调用独占可变借用；没有锁、线程、任务、通道、事务、I/O 或缓存生命周期。返回时递归上下文切片失效，`ColumnStatsUsage` 独立拥有所有集合。

时间成本主要是访问每个计划节点及展开相关表达式；有序集合操作引入对数级插入成本。Join/Sort/Aggregation 会为传播列构造临时 `Vec<i64>`，其生命周期仅覆盖对应子树递归。非常深的计划可能带来递归栈风险，列/表很多时 `BTreeMap/BTreeSet` 的确定性排序成本也高于哈希集合；当前实现没有并行化或显式资源上限。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `collect_column_stats_usage.go`。两版共同保留的主语义包括：遍历逻辑计划、DataSource 下推条件需要 full 统计、Join/Selection/排序/GROUP BY 通常只需 meta、记录逻辑表与静态分区、计算算子数，以及可选收集 WHERE/JOIN/ORDER/GROUP/MIN/MAX 的索引候选列。Rust 的 full 标志 OR 合并对应 Go `addPredicateColumn` 中“已是 full 则不降级”的规则。

Go 使用 `columnStatsUsageCollector`、`model.TableItemID` 和 `colMap` 维护输出列到基表列的血缘，并以后序遍历支持 Projection、Aggregation、Window、UnionAll/PartitionUnion、Apply、CTE/CTETable、IndexScan/TableScan 等完整算子；它还跳过系统表和非正 ID 伪列，把 interesting columns 写回具体 DataSource，并可为 plan replayer 记录运行时表统计。Rust 当前 `PlanKind` 不表达其中多种算子，Projection 分支也不读取其 `expressions` 建立血缘，因此这些能力不能从 Go 实现推断为 Rust 已支持。

Go 测试 `TestSkipSystemTables`、`TestCollectPredicateColumns`、`TestCollectHistNeededColumns` 覆盖系统表、投影血缘、窗口、Apply、CTE、TopN、静态/动态分区以及 full/meta 差异。Rust 独立测试聚焦现有 IR：`collect_column_stats_usage_test.rs` 验证两表 Join、分区映射、算子计数、普通聚合参数不误收集和索引开关；`rule_aster_unit_test.rs::join_predicate_columns_only_need_meta_stats` 验证纯 Join 的 meta 需求；`casetest/stats_test.rs` 验证 DISTINCT GroupNDV 与 Join 两侧。两组覆盖范围差异反映迁移状态，不应通过删减 Go 语义来消除。

## 扩展指南

新增统计使用规则时，优先修改 `collect` 中对应的 `PlanKind` 分支，并明确三个问题：该表达式需要 full 还是 meta、其列是否应向哪个子树传播为 interesting columns、分支自行递归后是否必须提前返回以避免重复计数。新增 `PlanKind` 时还应检查通用尾递归是否足够，或是否需要像 Join/Sort/Aggregation 一样建立子树上下文。

若要继续对齐 Go，不能只在此文件添加 match 分支；需要先在 `rule_init.rs` 为缺失算子和表列身份建立足够的 IR，再实现 Projection/Aggregation/Union/Window/CTE 的列血缘传播、系统表与伪列过滤，并同步调整 `rule_collect_plan_stats.rs::columns_for_table`。这是跨文件能力扩展，不属于本文件当前已支持行为。

测试必须放在独立文件，不要内嵌到生产源。直接单元回归应追加到 `collect_column_stats_usage_test.rs`；规则接线和按表归属可追加到 `rule_aster_unit_test.rs` 或 `rule_collect_plan_stats_test.rs`；Go 对齐的较完整场景还需检查 `collect_column_stats_usage_test.go` 与 `pkg/planner/core/casetest/stats_test.rs`。重点兼容风险是 full/meta 降级、跨表列 ID 混淆、子树上下文泄漏、重复递归导致算子数翻倍以及静态分区下标误映射；性能风险是表达式重复展开与有序集合增长。

## 验证依据

- RustCodeGraph：`status` 确认索引包含目标目录；`files --filter pkg/planner/core/rule` 确认 Rust、Go 与测试文件均已索引；`node --file pkg/planner/core/rule/collect_column_stats_usage.rs --offset 1 --limit 240` 读取 195 行完整实现；`query ColumnStatsUsage` 与 `query collect_column_stats_usage --kind function --limit 30 --json` 核对结构体及三个函数节点。对精确节点运行 `callers`/`callees` 时无输出并超时，故没有用该结果作“无调用边”的结论。
- Rust 源与模块：`pkg/planner/core/rule/collect_column_stats_usage.rs`；`pkg/planner/core/rule/rule_init.rs` 中 `Expr::columns`、`PlanKind`、`Plan`；`pkg/planner/core/rule/lib.rs` 的公开模块声明；`pkg/planner/core/rule/rule_collect_plan_stats.rs` 中 `CollectPredicateColumnsPoint::optimize` 和 `columns_for_table`。
- crate 边界：`pkg/planner/core/rule/Cargo.toml` 的 package、lib path、porting metadata 与依赖区；本文件没有条件编译依赖或独立 feature。
- Go 对照：`pkg/planner/core/rule/collect_column_stats_usage.go` 的 `columnStatsUsageCollector`、`collectFromPlan`、`CollectColumnStatsUsage`；`pkg/planner/core/rule/collect_column_stats_usage_test.go` 的三个顶层测试。
- Rust 测试：`pkg/planner/core/rule/collect_column_stats_usage_test.rs`、`pkg/planner/core/rule/rule_aster_unit_test.rs`、`pkg/planner/core/casetest/stats_test.rs`。本任务是纯文档分析，按任务约束未运行 Cargo。
