# `pkg/planner/core/rule/rule_collect_plan_stats.rs`

## 文件定位

该文件属于 `astersql-planner-core-rule` crate（见同目录 `Cargo.toml`），负责“逻辑计划需要哪些统计信息”的一部分 Rust 迁移实现。模块由 `pkg/planner/core/rule/lib.rs` 以 `pub mod rule_collect_plan_stats` 公开。

文件必须分成两层理解：第 16～255 行是被块注释包住的 Go 全流程迁移草稿，不参与编译；第 261～407 行才是当前可执行 Rust。当前代码提供一个基于精简 `Plan` IR 的谓词列收集规则、一个可注入的同步加载边界，以及 Go `CollectDependingVirtualCols` 的实际移植。生产优化流水线在 `pkg/planner/core/optimizer_runtime.rs` 中注册同名阶段；其中规则主体目前由该文件外的 `collect_predicate_columns_descendants` 执行，而虚拟列补集会直接调用本文件的 `collect_depending_virtual_columns`。因此，本文件既不是完整 Go 规则的等价落点，也不是未接线的纯桩。

## 核心职责

1. `CollectPredicateColumnsPoint::optimize` 调用 `collect_column_stats_usage` 遍历精简计划，将访问到的每张表映射成 `UsedStats`，并写回根计划的 `used_stats`。
2. `columns_for_table` 解决不同表可能复用列 ID 的问题：只从目标表对应的 `DataSource` schema 中选择全局谓词列集合的交集，然后递归合并同表子树结果。
3. `StatsLoader` 与 `request_load_stats` 定义最小同步加载协议：调用方给出 `(table_id, item_id)` 列表和等待毫秒数，只有全部请求项都出现在加载结果中才成功。
4. `collect_depending_virtual_columns` 根据真实 `TableInfo` 元数据，为待加载普通列找出直接依赖它们的公开虚拟生成列，以便生产优化器继续发现表达式索引统计需求。

注释草稿还描述索引裁剪、静态分区展开、同步等待、异步加载和运行时统计记录等完整 Go 行为，但这些符号当前没有编译进本文件，不能视为 Rust 已支持能力。

## 主要符号

- `SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK: &str`：同步加载未覆盖全部请求项时返回的稳定原因字符串，值与 Go 的 `skipPlanCacheReasonSyncLoadFallback` 相同。
- `CollectPredicateColumnsPoint { collect_index_pruning_columns: bool }`：逻辑规则配置；布尔值原样传给 `collect_column_stats_usage`，控制是否额外收集索引裁剪相关列。
- `impl LogicalRule for CollectPredicateColumnsPoint`：`name()` 返回 `collect_predicate_columns_point`；`optimize(Plan)` 填充 `used_stats`，返回 `(plan, false)`，表示没有改变计划树结构。
- `columns_for_table(&Plan, i64, &BTreeMap<i64, bool>) -> BTreeSet<i64>`：私有递归辅助函数，按表隔离谓词列。
- `StatsLoader::load(&self, &[(i64, i64)], u64)`：外部加载器抽象；结果集合表示实际加载成功的项目。
- `request_load_stats(...) -> Result<(), String>`：公开加载包装；传播加载器错误，或在部分加载时生成回退原因。
- `collect_depending_virtual_columns(&BTreeMap<i64, TableInfo>, &[StatsLoadItem]) -> Vec<StatsLoadItem>`：公开元数据辅助函数，只产生新的虚拟列项目，并统一设置 `FullLoad = true`。

`Plan`、`PlanKind`、`UsedStats` 与 `LogicalRule` 定义在 `rule_init.rs`；`collect_column_stats_usage` 及其 `ColumnStatsUsage` 定义在 `collect_column_stats_usage.rs`。

## 执行流程

`CollectPredicateColumnsPoint::optimize` 的实际流程如下：

1. 以整个 `Plan` 和 `collect_index_pruning_columns` 调用 `collect_column_stats_usage`，得到 `predicate_columns`、`visited_tables` 等汇总信息。
2. 按 `visited_tables` 的有序集合逐表处理；对每个表递归调用 `columns_for_table`。
3. `columns_for_table` 只在当前节点是目标表 `DataSource` 时取 `plan.schema` 与 `predicate_columns` 的交集，并递归遍历所有子节点；这避免把另一张表上相同数值的列 ID 错归给当前表。
4. 为每张访问表构造 `UsedStats`：`columns` 是上一步结果；`indexes` 为空；只要任一归属列在 `predicate_columns` 中标为 `true`，`full_load` 即为真；`pseudo` 固定为假、`version` 固定为零。
5. 以新构造的 `BTreeMap` 整体替换 `plan.used_stats`，返回计划和 `changed = false`。

`request_load_stats` 先调用 `StatsLoader::load`。加载器报错时用 `?` 原样返回；加载成功后逐项检查 `needed` 是否均在返回集合中，全覆盖返回 `Ok(())`，否则返回固定回退原因。

`collect_depending_virtual_columns` 分两阶段运行：先忽略索引项，将能在对应 `TableInfo.Columns` 中找到的普通列 ID 转成小写规范名并按表去重；再遍历相关表的所有列，保留状态为 `StatePublic`、确为虚拟生成列、尚未属于输入集合且其 `Dependences` 至少命中一个输入列名的列，生成 `FullLoad = true` 的非索引 `StatsLoadItem`。该算法只查一层依赖，不做传递闭包。

## 数据与状态

- 主要容器使用 `BTreeMap`/`BTreeSet`，因此表、列和加载结果具有确定性排序；函数内部没有随机性或全局可变状态。
- `optimize` 消费并返回 `Plan`，唯一持久修改是整体替换根节点 `used_stats`。它不改写 `kind`、`children`、schema、谓词或估算行数。
- `UsedStats.indexes`、`pseudo`、`version` 在当前实现中分别固定为空、`false`、`0`；这表明它是统计需求摘要，而非已从统计子系统读取出的完整运行时状态。
- `full_load` 是表级聚合：只要该表归属的任一列在 `predicate_columns` 映射中要求完整统计即置真。
- 虚拟列收集依赖 `astersql_meta_model::TableInfo` 的 `Columns`、`State`、`IsVirtualGenerated()`、`Dependences`，输出保留表 ID/列 ID，并明确 `IsIndex = false`、`IsSyncLoadFailed = false`。
- 文件前半的注释草稿提到 session、domain、异步直方图集合及等待耗时，但实际编译代码不读写这些状态。

## 依赖与调用关系

直接 Rust 依赖包括：

- `crate::collect_column_stats_usage::collect_column_stats_usage`：规则的下游计划遍历入口。
- `crate::rule_init::{LogicalRule, Plan, UsedStats}`：规则接口与精简计划/统计摘要。
- `std::collections::{BTreeMap, BTreeSet}`：确定性映射和集合。
- `astersql_meta_model::{TableInfo, StatsLoadItem, TableItemID, StatePublic}`：虚拟列元数据与加载项；`Cargo.toml` 将其声明为普通路径依赖。

上游及应用主链证据：

- `pkg/planner/core/rule/lib.rs` 公开本模块，并在测试配置下装入 `rule_collect_plan_stats_test.rs` 和聚合测试文件 `rule_aster_unit_test.rs`。
- `pkg/planner/core/optimizer_runtime.rs::LOGICAL_RULES` 把 `LogicalRule::CollectPredicateColumnsPoint` 放在分区处理之后、聚合下推之前；其 match 分支调用运行时自己的 `collect_predicate_columns_descendants`，并非这里的精简 `CollectPredicateColumnsPoint::optimize`。
- 同一运行时的统计需求构造代码会调用 `rule::rule_collect_plan_stats::collect_depending_virtual_columns`，随后把结果加入 statement context 的 `StatsLoad.NeededItems`，这是本文件当前明确接入生产路径的函数。
- RustCodeGraph 对 `request_load_stats` 找到 `rule_aster_unit_test.rs` 的部分加载回退测试；对 `collect_depending_virtual_columns` 找到 `optimizer_runtime.rs` 与 `casetest/planstats/plan_stats_test.rs` 的使用。图查询对若干精简 IR 调用没有返回调用边，因此这些关系又以模块引用和源码位置核验。

## 错误处理与边界

- `CollectPredicateColumnsPoint::optimize` 的当前代码路径自身不产生错误，始终返回 `Ok`；其 `Result<_, String>` 来自 `LogicalRule` trait 统一签名。
- `request_load_stats` 区分两类失败：`StatsLoader::load` 返回的错误保持原文传播；加载器成功但结果不完整时返回 `SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK`。它不在这里写警告、修改 statement context 或自动转用伪统计。
- 空 `needed` 会自然成功，因为 `all` 对空迭代器为真。加载器返回额外项目不影响结果。
- `columns_for_table` 可遍历任意深度并合并同一表的多个数据源；非 `DataSource` 节点只负责继续递归。它依据 schema 归属过滤，无法区分同一数据源 schema 内语义不同但 ID 相同的列，这依赖上游 IR 的列 ID 约束。
- 虚拟列函数静默跳过索引输入、未知表、未知列 ID、非公开列、存储生成列/普通列、已在输入中的虚拟列和无直接命中的依赖。输出不去重同一 `TableInfo.Columns` 内的重复元数据；正常元数据应保证列唯一。
- 只处理直接依赖是有意边界，与 Go 注释及九组对照测试一致；若未来需要间接依赖，必须显式设计闭包与循环依赖处理。

## 并发与资源生命周期

当前可执行实现完全同步，不创建线程、任务、通道、锁、事务或 I/O 资源。`optimize` 拥有 `Plan` 并在函数返回时转移其所有权；辅助函数只在调用期间借用计划或元数据。`request_load_stats` 的等待策略和资源管理由传入的 `StatsLoader` 实现负责，本函数仅转交 `wait_ms`，自身不启动计时器也不取消请求。

所有集合均为函数局部值；没有静态可变缓存。`SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK` 是只读静态字符串。生产路径中 statement context 的互斥锁与统计加载生命周期存在于 `optimizer_runtime.rs` 及统计子系统，不由本文件管理；注释草稿中提到的异步全局集合也不属于当前编译实现。

## 与 Go 版本的对应关系

Rust `CollectPredicateColumnsPoint::name` 与 Go `CollectPredicateColumnsPoint.Name` 的注册语义一致，谓词列收集入口也源自 Go `CollectColumnStatsUsage`。但 Rust 精简实现目前仅生成 `Plan.used_stats`；Go `Optimize` 还会更新 statement context、查 InfoSchema、保证每表至少一次 full load、裁剪 access paths、收集索引、展开静态分区并选择同步或异步加载。文件前 255 行保存了这些步骤的迁移草稿，但它们没有参与编译，不能据此声称已对齐。

Rust `request_load_stats` 只是可测试的最小抽象。Go `RequestLoadStats` 会用最大执行时间截断等待、向 domain stats handle 发请求、设置 `IsSyncStatsFailed`、记录日志/警告，并受 `StatsLoadPseudoTimeout` 控制是否吞掉超时错误。Rust 当前仅验证请求项是否全部加载，并以同一原因字符串报告部分加载。

Rust `collect_depending_virtual_columns` 是 Go `CollectDependingVirtualCols` 的直接行为移植：忽略索引输入、按表和列名匹配、只考虑公开虚拟列及直接依赖、跳过已需列、输出 full-load 项。Rust `pkg/planner/core/casetest/planstats/plan_stats_test.rs::test_collect_depending_virtual_columns_from_table_metadata` 复现 Go `plan_stats_test.go::TestCollectDependingVirtualCols` 的九组真实元数据场景，覆盖 JSON 多值索引、表达式索引和多层虚拟列的“一层依赖”边界。

## 扩展指南

- 扩展谓词统计摘要时优先修改 `CollectPredicateColumnsPoint::optimize` 与 `columns_for_table`，并同步独立测试 `rule_collect_plan_stats_test.rs` 或 `rule_aster_unit_test.rs`；不要把测试嵌入生产文件。
- 若要让精简规则进入真实优化主链，必须同时检查 `optimizer_runtime.rs::LOGICAL_RULES`、对应 match 分支和 `collect_predicate_columns_descendants`，明确替换或桥接关系，避免两套同名规则产生行为漂移。
- 若增加索引、分区、pseudo/version 或真实加载状态，应先扩展 `rule_init.rs::UsedStats`/`Plan` 的状态契约，再与 Go `Optimize` 的相应增量逐项对齐；不要仅把注释草稿取消注释，因为草稿中的 domain/session 类型尚未接线。
- 修改加载协议时保持“加载器错误”和“成功但不完整”两类错误可区分，并补充完整加载、空输入、加载器失败及部分加载测试。若接入真实等待/回退，还需同步 statement context 与计划缓存语义。
- 修改虚拟列依赖算法时同步 Rust `casetest/planstats/plan_stats_test.rs` 和 Go `plan_stats_test.go` 的场景。引入传递依赖必须考虑循环、重复输出、稳定顺序和额外统计加载的性能成本。
- 性能风险集中在 `columns_for_table` 对每张访问表重复递归整棵计划树，以及虚拟列阶段按表线性扫描列元数据；大计划或宽表上的扩展应先评估复杂度。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已索引并报告 13 个符号。
- RustCodeGraph `node --file`：完整读取 `rule_collect_plan_stats.rs` 第 1～407 行，确认注释草稿边界和实际实现；读取 `rule_init.rs` 的 `Plan`、`UsedStats`、`LogicalRule`；读取 `collect_column_stats_usage.rs` 的汇总结构和入口。
- RustCodeGraph `query`：确认 Rust 的 `CollectPredicateColumnsPoint`、`columns_for_table`、`StatsLoader`、`request_load_stats`、`collect_depending_virtual_columns`，并定位 Rust/Go 同名符号及相关测试。
- RustCodeGraph `callers`/`callees`：对精确 Rust 符号执行查询但未返回文本边；因此没有把缺失图边当成“无调用者”，而是用 `optimizer_runtime.rs`、`lib.rs` 和测试源码中的直接引用补证。
- crate/模块证据：`pkg/planner/core/rule/Cargo.toml`、`pkg/planner/core/rule/lib.rs`。
- Rust 调用与测试证据：`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/rule/rule_collect_plan_stats_test.rs`、`pkg/planner/core/rule/rule_aster_unit_test.rs`、`pkg/planner/core/casetest/planstats/plan_stats_test.rs`。
- Go 对照证据：`pkg/planner/core/rule/rule_collect_plan_stats.go`、`pkg/planner/core/casetest/planstats/plan_stats_test.go`。
- 目标目录不存在 `doc.go`，因此无更近的 Go package contract 可读；模块定位由 Cargo、`lib.rs`、运行时注册与 Go 对照共同确认。
- 本任务为纯文档分析，按任务要求未运行 Cargo；最终结构检查应确认文件存在且恰好包含规定的 11 个二级标题。
