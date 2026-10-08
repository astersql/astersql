# `pkg/planner/core/rule_correlate.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `pkg/planner/core/lib.rs` 以 `pub mod rule_correlate` 公开本模块。它在 `rule_join_reorder.rs` 定义的紧凑 `JoinPlan`/`JoinNode` 模型上实现“相关化”：把带有 correlate 偏好的半连接重新构造成 `Apply`，为逐外层行的相关索引访问提供计划形态。

需要区分两个实现层次：本文件的 `CorrelateSolver` 是紧凑计划模型的公开实现，RustCodeGraph 与源码搜索发现其直接使用者是 `rule_correlate_test.rs` 和 `casetest/rule/rule_correlate_test.rs`；完整 Rust 逻辑优化流水线中的 `LogicalRule::Correlate` 实际接到 `optimizer_runtime.rs::correlate_descendants`。因此，本文件承载可执行的移植语义与独立测试契约，但不是完整 `logicalop::LogicalPlan` 主链的直接执行体。

## 核心职责

- `CorrelateSolver::Optimize`/`correlate` 后序遍历紧凑计划树，寻找满足安全门槛的 `Semi` 或 `AntiSemi` `JoinNode::Join`。
- 只处理 `preferred_method == Some("correlate")`、至少有一个普通等值条件、没有 `other_conditions`、且所有连接边都不是 null-safe equality 的候选。
- 按左右 schema 校正每条连接边方向，将外层列记入新 `Apply` 的 `correlated_columns`，并把表示相关等值的 `Expression { name: "correlated_eq", column: Some(inner) }` 下推到包含内层列的叶子。
- 在改写前合并“直接覆盖 Leaf 的 Selection”条件；改写后调整已标记相关列叶子的统计下界。
- 对不满足全部条件的候选采取保守跳过，保持原 Join 结构。

## 主要符号

- `CorrelateSolver`：无字段、可默认构造的规则对象。
- `Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)>`：公开入口，直接委托 `correlate`；返回改写后的树与“任意后代或当前节点是否改变”的布尔值。
- `correlate(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)>`：公开递归实现。显式覆盖 `Apply`、`Join`、`Projection`、`Selection`、`Aggregation`、`Window`、`UnionAll`，其它节点原样返回。
- `buildCorrelatedCond(&self, outerColumn, innerColumn) -> JoinEdge`：构造 `null_equal = false` 的普通等值边；当前主改写流程没有调用它，主要由对齐测试验证该辅助契约。
- `Name(&self) -> &'static str`：返回稳定注册名 `"correlate"`；本模块没有实现 `rule_init.rs` 中另一套 `LogicalRule` trait。
- `liftDataSourceConds(plan) -> JoinPlan`：仅当根节点是 `Selection` 且其直接子节点是 `Leaf` 时，将 Selection 条件追加到叶子已有 `predicates` 后并移除 Selection；它不是通用递归 lifting。
- `push_correlated_predicate(plan, inner_column) -> bool`：私有深度优先下推函数。在 schema 含目标列的首个 Leaf 上追加相关谓词；二叉节点先左后右，`UnionAll` 使用 `any`，找到首个成功分支即停止。
- `resetStatsForCorrelatedDS(&mut plan) -> bool`：递归查找 `correlated_columns` 非空的 Leaf，把其 `row_count` 提升到至少 `1.0`，并返回是否命中；它不会修改祖先的 `row_count`。

## 执行流程

1. `Optimize` 调用 `correlate`；后者先递归改写当前节点的所有子树，并把子树的 `changed` 汇总到当前结果。
2. 对 `JoinNode::Join`，检查连接类型、偏好字符串、等值条件、其它条件及 `null_equal`。任一条件不满足即重建原 Join，仅保留子树已发生的变化。
3. 对每条等值边，用左右子计划的 `schema` 判断方向。只有每条边都能唯一解释为“左侧外层列、右侧内层列”时才形成 `columns: Vec<(outer, inner)>`；任一边无法定向就放弃当前节点改写。
4. 克隆右子树并调用 `liftDataSourceConds`。随后逐个内层列调用 `push_correlated_predicate`；要求所有列均能到达某个 Leaf，否则丢弃候选副本并保留原右子树。
5. 全部相关谓词成功落到叶子后，调用 `resetStatsForCorrelatedDS`，再构造 `JoinNode::Apply`：保留原 `join_type` 和左右子树，把所有外层列写入 `correlated_columns`，并设 `no_decorrelate = false`。
6. `Projection`、`Selection`、`Aggregation`、`Window` 只递归并原样恢复自身字段；`UnionAll` 逐个处理全部分支；Leaf 等其它节点终止递归。

这里有一个重要的原子性边界：谓词写入发生在克隆的右子树上，所以部分下推成功、后续下推失败时，不会污染最后返回的原始 Join 右子树。

## 数据与状态

规则自身无持久状态，所有变换通过拥有所有权的 `JoinPlan` 值和局部可变副本完成。`JoinPlan.schema: Vec<usize>` 提供列归属判断；`JoinEdge` 保存两端列号与 NULL 相等语义；`JoinNode::Leaf.predicates` 接收相关谓词；`JoinNode::Leaf.correlated_columns` 决定统计是否需要调整；`JoinPlan.row_count` 是本实现唯一修改的统计字段。

生成的 `Expression` 只设置 `name = "correlated_eq"` 和 `column = Some(inner_column)`，其余字段使用默认值。`Apply.correlated_columns` 只保存外层列号，不保存内外列成对映射；多条条件按原 `equal_conditions` 顺序保留，未去重。

## 依赖与调用关系

- 直接上游：`pkg/planner/core/lib.rs` 导出模块；`pkg/planner/core/rule_correlate_test.rs` 和 `pkg/planner/core/casetest/rule/rule_correlate_test.rs` 构造 `JoinPlan` 并调用公开 API。RustCodeGraph 对 `CorrelateSolver` 的 Rust trail 也只显示测试导入，仓库搜索未发现紧凑求解器被生产优化入口实例化。
- 直接下游：`crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result}` 和 `crate::task::Expression`。其中 `Result<T>` 是 `std::result::Result<T, String>`。
- 完整主链对照：`optimizer_runtime.rs` 的规则数组把 `FLAG_CORRELATE` 映射为 `LogicalRule::Correlate`，再调用同文件的 `correlate_descendants`；`expression_rewriter.rs` 会依据 alternative logical plans 状态设置 `LogicalJoin.PreferCorrelate`。这条主链使用 `logicalop` 类型而非本文件的 `JoinPlan`。
- crate 边界：本文件没有直接使用第三方 crate；所需类型都在 `astersql-planner-core` 内部。`Cargo.toml` 的 `autotests = false` 意味着根测试由 `lib.rs` 的 `#[cfg(test)] mod rule_correlate_test` 显式挂载；case test 位于独立的 `casetest/rule` crate。

## 错误处理与边界

递归调用使用 `?` 传播 `Result<_, String>`，但当前文件内部没有主动构造 `Err` 的分支；因此现状下错误通道主要是接口兼容预留。所有不安全或无法表达的改写都返回 `Ok((原形态, changed))`，而不是报错。

明确跳过的情况包括：非 `Semi`/`AntiSemi`、偏好不是精确小写字符串 `"correlate"`、没有等值边、存在其它条件、任一边为 `null_equal`、列不能分别定位于左/右 schema、或相关谓词找不到包含内层列的 Leaf。已有 `Apply` 只递归其孩子，不会再次相关化。

本紧凑实现没有 Go 版本针对 CTE、Left/RightConditions、NAEQConditions、LeftOuterSemi/AntiLeftOuterSemi 可空列、克隆失败、谓词下推错误、Limit 1、hint/schema/output name 保留等完整边界；调用者不能据此推断完整 SQL 优化器已经通过本文件覆盖这些行为。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、I/O 或外部资源。一次调用独占传入的计划树；递归过程中通过移动和 `Box` 重建节点，通过克隆右子树隔离候选改写。生命周期完全受 Rust 所有权管理，函数返回后只有返回的 `JoinPlan` 保留。

时间复杂度通常与遍历节点数及连接边数线性相关，但每个可相关化 Join 会克隆整棵右子树，且每条等值边都可能重新深度搜索该副本，因此该节点最坏约为 `O(E * N_inner)`，另加一次统计遍历。递归深度等于计划树深度，代码没有显式深度保护。

## 与 Go 版本的对应关系

`pkg/planner/core/rule_correlate.go` 是同名、面向完整 `base.LogicalPlan` 的 Go 实现；两者共同遵守“后序遍历、跳过已有 Apply、仅相关化带偏好的半连接、把等值条件变成相关谓词、重新处理内侧条件与统计、最终生成 Apply”的核心方向，`Name()` 也都返回 `correlate`。

Rust 本文件是明显收窄的紧凑模型移植：Go 会捕获 panic 并转错、跳过 CTE、校验更多条件类别和外半连接 NULL 语义、克隆完整内子树、重新运行 PPD、重建 access paths/清空沿途统计、补 `Limit 1`，并保留 hint、schema 与 output names；本文件只合并直接 `Selection -> Leaf`、添加占位 `Expression`、将已带 `correlated_columns` 的 Leaf 行数下限设为 1，也只接受两种半连接类型。完整 Rust 主链中的 `optimizer_runtime.rs::correlate_descendants` 更接近 Go 数据模型与规则接线，因此评估线上行为时应以该实现及其测试为准。

`rule_correlate_test.rs` 验证紧凑实现的方向校正、成功改写、null-equal/其它条件安全跳过和已有 Apply 保持；`casetest/rule/rule_correlate_test.rs` 进一步验证辅助条件、Selection 合并、统计调整及 Go fixture 清单对齐。Go 的 `casetest/rule/rule_correlate_test.go` 还覆盖 cost factor、真实 SQL 结果以及 parallel apply 交互，这些不由本文件的紧凑测试直接证明。

## 扩展指南

- 新增候选门槛时，集中修改 `CorrelateSolver::correlate` 的 `can_correlate` 与列定向逻辑，并在同目录独立测试文件 `rule_correlate_test.rs` 增加“接受”和“保守跳过”成对用例。
- 若扩展可穿透的计划节点，必须同时更新 `correlate`、`push_correlated_predicate` 和 `resetStatsForCorrelatedDS` 三处匹配；否则可能出现子树未遍历、谓词无法落地或统计未刷新。测试仍应放在独立 `*_test.rs`，不要内嵌到生产文件。
- 若允许一个内层列出现在多个 `UnionAll` 分支，需重新审视当前 `any` 的短路行为；若语义要求每个分支都收到谓词，应改为全分支验证并保持失败原子性。
- 若要接入真实 SQL 优化主链，不能仅注册此 `JoinPlan` 求解器；必须与 `optimizer_runtime.rs::correlate_descendants`、`LogicalRule::Correlate`、`FLAG_CORRELATE` 和 `logicalop` 数据结构一起设计，避免形成两套漂移实现。
- 与 Go 扩展保持一致时，优先补齐明确的语义缺口（NULL 三值逻辑、PPD、Limit、统计/access path、hint/输出元数据），而不是仅让紧凑测试通过。主要风险是错误结果（尤其 NULL/NOT IN）、候选计划条件丢失和代价估算陈旧；性能风险来自右子树克隆和按连接边重复遍历。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`node --file pkg/planner/core/rule_correlate.rs` 读取完整 257 行；`query CorrelateSolver`、`node CorrelateSolver`、`node JoinPlan`、`node JoinNode`、`node JoinEdge` 和 `node rule_join_reorder.rs::Result` 核对符号、类型与引用 trail。`explore`/部分 `callers` 查询未返回额外调用边，随后用精确节点查询与 `rg` 补证。
- 源码与边界：`pkg/planner/core/rule_correlate.rs`、`pkg/planner/core/rule_join_reorder.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`。
- 完整 Rust 接线：`pkg/planner/core/optimizer_runtime.rs` 中 `LogicalRule::Correlate` 与 `correlate_descendants`，以及 `pkg/planner/core/expression_rewriter.rs` 中 `PreferCorrelate` 标记逻辑。
- Go 对照：`pkg/planner/core/rule_correlate.go`；测试证据：`pkg/planner/core/rule_correlate_test.rs`、`pkg/planner/core/casetest/rule/rule_correlate_test.rs`、`pkg/planner/core/casetest/rule/rule_correlate_test.go`。
- 按任务约束未运行 Cargo；本任务仅生成说明文档。任务指定的结构检查已执行，目标文件存在、固定二级标题恰好 11 个，退出码为 0；`git diff --check -- pkg/planner/core/rule_correlate.rs.md` 退出码亦为 0。
