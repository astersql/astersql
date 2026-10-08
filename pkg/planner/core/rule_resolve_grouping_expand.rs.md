# `pkg/planner/core/rule_resolve_grouping_expand.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate，由 [`lib.rs`](lib.rs) 以 `pub mod rule_resolve_grouping_expand` 对外暴露。它移植 Go `ResolveExpand` 的核心意图：在逻辑优化末尾为 `ROLLUP`、`CUBE` 或 `GROUPING SETS` 产生的 Expand 节点补齐逐层投影。这里使用的是 [`rule_aggregation_elimination.rs`](rule_aggregation_elimination.rs) 中的精简 `LogicalPlan` 枚举，而不是完整运行时的 `logicalop::LogicalPlan` trait 对象。

需要区分两条 Rust 路径：本文件的 `ResolveExpand::Optimize -> genExpand` 是可执行、可独立测试的移植模型；当前完整逻辑优化主链在 [`optimizer_runtime.rs`](optimizer_runtime.rs) 的 `LogicalRule::ResolveExpand` 分支调用 `resolve_expand_descendants`，再对真实 `logicalop::LogicalExpand` 调用 `GenLevelProjections`。RustCodeGraph 对本文件的直接上游只找到 [`rule_resolve_grouping_expand_test.rs`](rule_resolve_grouping_expand_test.rs)，因此没有证据表明本文件的 `ResolveExpand` 已直接注册进完整运行时规则表。

## 核心职责

- `ResolveExpand::Optimize` 以所有权方式接收一棵精简逻辑计划树，委托 `genExpand` 后序遍历，并固定返回 `changed = false`，保持 Go 规则“补齐内部投影但不报告计划结构变化”的契约。
- `genExpand` 在每个 `LogicalPlan::Expand` 上重建 `level_projections`：普通子列按列引用透传；属于任一分组集合、但在当前层缺席的列改成带原返回类型的 NULL 表达式；末尾追加 GID，存在重复分组集合时再追加 GPos。
- 对 `Aggregation`、`Projection`、`Join`、`UnionAll` 递归处理子树并原样重建父节点；其他节点是递归终点。
- 规则只解决 Expand 层级投影生成，不构造 Expand、不改写 `GROUPING()`、不执行聚合，也不把逻辑 Expand 物理化。

## 主要符号

- `pub struct ResolveExpand`：无字段规则对象，并派生 `Default`；本文件没有可变规则配置。
- `ResolveExpand::Optimize(&self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)>`：公开规则入口。它传播 `genExpand` 的错误，但忽略内部返回的布尔值并固定返回 `false`。
- `ResolveExpand::Name(&self) -> &'static str`：返回稳定注册名 `"resolve_expand"`，与 Go `Name` 一致。名称沿用 Go 风格大写方法名，未采用 Rust snake_case。
- `pub fn genExpand(plan: LogicalPlan) -> Result<(LogicalPlan, bool)>`：公开递归实现。其输入 IR 的 `Expand` 变体包含 `child`、`grouping_sets: Vec<Vec<usize>>`、`level_projections` 和 `schema`。
- `Expression`：来自 [`task.rs`](task.rs) 的精简表达式。这里实际设置 `name`、`column` 与 `return_type`；GID/GPos 的数值编码在 `name` 中，未构造完整运行时常量表达式。

本文件无模块级常量、trait、条件编译项或私有辅助函数。

## 执行流程

1. `Optimize` 调用 `genExpand(plan)`。若递归成功，返回改写后的整棵树和 `false`。
2. 遇到 `Expand` 时先递归其唯一子节点，保证处理顺序是子节点优先；随后用 `child.schema().len()` 确定需要逐列投影的子输出宽度。
3. 将所有 `grouping_sets` 中的列下标汇入 `BTreeSet`。该集合既判断某列是否属于分组列，也给 GID 位分配提供稳定的升序顺序。
4. 每个分组集合先经 `BTreeSet` 规范化，再装入 `HashSet<Vec<_>>`。规范化会消除集合内部的重复列和顺序差异；规范化后的不同集合数少于原层数即表示存在重复 grouping set。
5. 清空传入的 `level_projections`，按原 `grouping_sets` 顺序逐层重建。对每个子列：若它是分组列但当前层不含它，则生成 `name = "null"`、`column = None` 的表达式；否则生成 `col_<下标>` 并保留 `Some(下标)`。两者均从子 schema 克隆返回类型。
6. 按 `all_group_columns` 的升序位置计算 64 位 GID：当前层出现的列将对应位设为 1。追加 `grouping_id:<值>` 表达式，其类型取 Expand schema 的 `column_count` 位置。
7. 若检测到重复集合，再追加 `grouping_position:<level>`；`level` 是原始集合序号，类型取 schema 的 `column_count + 1` 位置，用于区分内容相同的层。
8. 重建 `Expand` 并返回 `false`。`Aggregation` 与 `Projection` 递归单子节点；`Join` 依次处理左右子树；`UnionAll` 保持子节点顺序逐一处理；`Node` 等其他变体原样返回。

## 数据与状态

规则不保存跨调用状态；所有状态都随 `LogicalPlan` 所有权进入并在返回值中离开。`grouping_sets` 的外层顺序决定 level/GPos，单个集合内部按集合语义判断成员。`level_projections` 会先 `clear`，因此重复调用本实现会重算而不会继续追加旧层。

每层投影的预期宽度是 `child.schema().len() + 1`，若有重复集合则为 `child.schema().len() + 2`。非分组列在所有层透传；分组列只在当前集合包含它时透传，否则被 NULL 替代。GID 使用 `u64` 位掩码，位序取全部分组列下标的升序，而 GPos 使用原 grouping set 的层序号。

`schema` 与 `child.schema()` 只用于克隆 `FieldType`；实现不修改 schema 本身，也不缓存派生集合。`HashSet` 只参与成员或重复性判断，不影响输出层顺序；`BTreeSet` 保证规范化和位序确定性。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-planner-core`，库入口为 `lib.rs`，默认 feature 为空，`nextgen` 只转发部署/内核配置 feature。本文件仅使用同 crate 的 `rule_aggregation_elimination::{LogicalPlan, Result}`、`task::Expression` 与标准库集合，不直接依赖 Cargo 中列出的外部 crate。

RustCodeGraph 的精确节点结果为：`ResolveExpand::Optimize -> genExpand`；`genExpand` 构造 `Expression`，并在 `Aggregation`、`Projection`、`Join`、`UnionAll`、`Expand` 分支递归/重建精简 IR。图中 `ResolveExpand` 的直接导入者是独立测试文件。

完整应用主链的直接证据在 [`optimizer_runtime.rs`](optimizer_runtime.rs)：`LOGICAL_RULES` 将 `LogicalRule::ResolveExpand` 放在最后；`LogicalOptimize` 的对应分支调用 `resolve_expand_descendants`；该函数后序遍历 `Children_mut()`，再对 `LogicalExpand` 调用 `GenLevelProjections`。所以本文件表达相同规则意图，但完整主链使用另一套真实算子实现。

## 错误处理与边界

`Optimize` 和 `genExpand` 使用共享 `Result`，所有递归错误通过 `?` 原样向上返回。当前各 match 分支本身没有显式构造错误，因此在现有实现中错误通道主要为与同组规则保持统一签名、供未来校验扩展使用。

空 `grouping_sets` 是合法边界：循环不产生层，清空后的 `level_projections` 保持为空，独立测试明确覆盖该行为。重复集合按集合内容识别，集合内部次序和重复列不改变重复性判断；不同外层层仍分别产生投影，且依靠 GPos 区分。

schema 类型访问使用 `get(...).cloned()`，schema 过短不会 panic，而会让生成表达式的 `return_type` 成为 `None`；本函数没有主动报错验证“Expand schema 必含 GID/GPos”这一不变量。列下标也没有范围校验：超出 child schema 的下标会参与集合和 GID，但不会新增普通列投影。GID 用 `checked_shl`，第 64 位及以后返回 0 并被忽略；若扩展到超过 64 个不同分组列，必须先明确兼容策略，不能把当前结果当作无损编码。

## 并发与资源生命周期

实现是同步、单线程、纯所有权转换：没有锁、原子量、任务、通道、事务、网络或文件资源。`ResolveExpand` 是零大小无状态值，可安全地为每次优化临时构造；函数没有全局共享状态。

主要资源成本来自递归深度和集合/投影分配。每个 Expand 会构造全部分组列集合、规范化后的不同集合、每层成员集合以及每层表达式向量；时间大致随“计划节点数 + 各 Expand 的层数 × 子列数”增长，内存由生成的 `level_projections` 主导。树递归没有显式深度保护，极端深树受调用栈限制。

## 与 Go 版本的对应关系

直接对照文件是 [`rule_resolve_grouping_expand.go`](rule_resolve_grouping_expand.go)。两端的规则名均为 `resolve_expand`，都先递归子树、在所有必要逻辑优化（特别是列裁剪）完成后生成层级投影，并且 `Optimize` 都固定报告 `planChanged = false`。Go 优化器在 [`optimizer.go`](optimizer.go) 中注册 `&ResolveExpand{}`，规则 flag 由 [`rule/logical_rules.go`](rule/logical_rules.go) 定义；Rust 完整规则顺序则由 `optimizer_runtime.rs::LOGICAL_RULES` 对齐。

语义对应上，Rust 精简实现保留了 Go `LogicalExpand::GenLevelProjections` 的关键形状：当前层缺席的分组列置 NULL；普通列透传；每层追加 GID；内容重复的 grouping sets 额外追加按层序号生成的 GPos。Go 真实实现还维护 `DistinctSize`、`RollupGroupingIDs`、`RollupID2GIDS`，支持 `GroupingMode_ModeBitAnd` 与 `ModeNumericSet`，并创建带真实 `FieldType` 的表达式常量；本文件只用列下标、`u64` 位掩码和 `Expression.name` 表示这些信息，未覆盖上述完整元数据和 NumericSet 分支。

另一个可见差异是 Go `GenLevelProjections` 直接向 `LevelExprs` 追加，而本文件先清空 `level_projections`。因此本文件的重复调用具有重算语义；不能据此推断 Go 方法重复调用也会自动去重。当前完整 Rust 主链调用的是移植后的真实 `LogicalExpand::GenLevelProjections`，相关行为由 [`optimizer_logical_entry_aster_unit_test.rs`](optimizer_logical_entry_aster_unit_test.rs) 覆盖。

## 扩展指南

- 若只扩展精简移植模型（例如新增 schema 校验、GID 模式或更多计划变体），修改 `genExpand`，并在独立的 [`rule_resolve_grouping_expand_test.rs`](rule_resolve_grouping_expand_test.rs) 增加测试；不要把测试内嵌进本源文件。
- 若改变完整 SQL 优化行为，真正接入点是 `optimizer_runtime.rs::resolve_expand_descendants` 和 `operator/logicalop` 中的 `LogicalExpand::GenLevelProjections`，并需同步 Go `rule_resolve_grouping_expand.go`、`operator/logicalop/logical_expand.go` 的语义证据以及完整入口测试。只修改本文件不会改变当前完整主链。
- 新增计划节点时，要决定它是递归容器还是叶节点；容器必须在 `genExpand` 中显式遍历全部子树并保持节点字段、子节点顺序和错误传播。
- 改动 GID 位序、重复集合判定或 GPos 生成会影响 `GROUPING()` 语义和相同 grouping set 的可区分性；至少覆盖空集合、唯一集合、重复集合、集合内乱序/重复列、非分组列透传和超过 64 位的边界。
- 改动 schema 布局时，应把目前隐式的“GID 位于 child 列之后、重复集合时 GPos 再后一位”变成显式校验或同步调整索引；否则类型可能静默变成 `None`。
- 性能优化应保留确定性：可以减少每层 `HashSet` 或规范化分配，但不能改变外层 level 顺序、BTreeSet 确定的位序或子树后序处理。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/planner/core/rule_resolve_grouping_expand.rs`：读取目标文件 167 行全貌；`node rule_resolve_grouping_expand.rs::ResolveExpand` 确认公开入口及测试导入；`node rule_resolve_grouping_expand.rs::genExpand` 确认 `Optimize -> genExpand`、`Expression` 构造和递归分支。
- RustCodeGraph `node rule_aggregation_elimination.rs::LogicalPlan` 与 `node task.rs::Expression`：核对精简 IR 的各变体、Expand 字段以及表达式字段。
- crate/装配证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。目标包不存在 `doc.go`，因此没有额外包契约可读。
- Go 对照：[`rule_resolve_grouping_expand.go`](rule_resolve_grouping_expand.go)、[`operator/logicalop/logical_expand.go`](operator/logicalop/logical_expand.go)、[`optimizer.go`](optimizer.go)、[`rule/logical_rules.go`](rule/logical_rules.go)。
- Rust 主链证据：[`optimizer_runtime.rs`](optimizer_runtime.rs) 的 `LOGICAL_RULES`、`LogicalRule::ResolveExpand` 分支与 `resolve_expand_descendants`。
- 独立 Rust 测试：[`rule_resolve_grouping_expand_test.rs`](rule_resolve_grouping_expand_test.rs) 覆盖空 grouping sets、唯一集合的 NULL/GID 与重复集合的 GPos；[`optimizer_logical_entry_aster_unit_test.rs`](optimizer_logical_entry_aster_unit_test.rs) 覆盖完整逻辑优化入口生成真实 `LogicalExpand.LevelExprs`。Go `logical_plans_test.go` 证明 builder 会准备 `currentBlockExpand`、默认位与模式，但检索范围内未发现直接单测 `GenLevelProjections` 全部分支。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前以任务指定命令校验恰有 11 个固定二级章节，并人工复核主链边界、Go/Rust 差异和扩展风险。
