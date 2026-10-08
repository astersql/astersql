# `pkg/planner/core/rule_decorrelate.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod rule_decorrelate` 公开该模块，Cargo 边界由 [`Cargo.toml`](Cargo.toml) 定义。它在 `rule_join_reorder.rs` 提供的简化 `JoinPlan`/`JoinNode` 计划模型上实现一组 Apply 去相关辅助逻辑：识别外层相关列、尝试把 Apply 改写为 Join、剪除一种冗余 Apply，并保护 Left Outer Apply 上投影的 NULL 扩展语义。

当前接线范围需要特别说明：仓库内对 `DecorrelateSolver::Optimize` 的直接 Rust 调用位于 `casetest/windows/window_with_exist_subquery_test.rs`；同目录单元测试直接调用相关列提取与投影保护函数。生产逻辑计划的另一条去相关实现位于 `optimizer_runtime.rs`。因此，本文件是公开、可独立测试的简化迁移实现，但现有引用证据不能证明它已经取代 Go `rule_decorrelate.go` 或成为完整生产优化流水线的主入口。

## 核心职责

- `ExtractOuterApplyCorrelatedCols` 与 `extractOuterApplyCorrelatedColsHelper` 遍历简化计划树，收集相关列，同时排除可由树内任一 Apply 左侧 schema 解析的列，只留下应由当前子树之外的 Apply 提供的列。
- `DecorrelateSolver::Optimize`/`optimize` 自顶向下递归处理有限的节点种类；当 Apply 未被 `no_decorrelate` 禁止、相关列都由左子树提供，且右侧能提供相关等值边（或本来没有相关列）时，将 Apply 改写为相同连接类型的 Join。
- `collect_correlated_edges` 把右侧 `Leaf` 或 `Selection` 中名为 `correlated_eq` 的简化谓词转成 `JoinEdge`；`strip_correlated_conditions` 随后从对应位置删除已提升的谓词，避免重复过滤。
- `pruneRedundantApply` 在严格的简化条件下直接返回 Left Outer Apply 的左子树。
- `skipDecorrelateProjectionForLeftOuterApply` 识别常量投影或完全引用外侧列的投影，防止投影上提后把外连接未匹配行应有的 NULL 重新计算成常量或外侧值。

## 主要符号

- `pub fn ExtractOuterApplyCorrelatedCols(plan: &JoinPlan) -> Vec<usize>`：公开便利入口，只返回筛选后的外部相关列。
- `pub fn extractOuterApplyCorrelatedColsHelper(plan: &JoinPlan) -> (Vec<usize>, Vec<HashSet<usize>>)`：除相关列外，还返回遍历到的 Apply 左侧列集合快照。内部 `walk` 覆盖 Apply、Join、Projection、Selection、Aggregation、Window、UnionAll 和 Leaf。
- `pub struct DecorrelateSolver`：无字段、可 `Default` 构造的规则对象，不保存跨调用状态。
- `DecorrelateSolver::aggDefaultValueMap(&JoinPlan) -> HashMap<usize, String>`：若节点是 Aggregation，则克隆其 `default_values`；否则返回空映射。当前文件内没有调用它。
- `DecorrelateSolver::Optimize(JoinPlan) -> Result<(JoinPlan, bool)>`：以空分组列集合启动递归，返回改写后的计划和“是否发生变化”标志。
- `DecorrelateSolver::optimize(JoinPlan, &HashSet<usize>)`：核心递归函数，只显式下降到 Apply、Aggregation、Projection、Join；其他节点原样保留，随后统一尝试剪枝。
- `DecorrelateSolver::Name() -> &'static str`：返回规则名 `decorrelate`。
- `pub fn pruneRedundantApply(...)`：仅匹配 `LeftOuter`、`no_decorrelate == false`、无相关列、右侧估算行数不超过 1，且传入分组列都由左侧提供的直接 Apply。
- `pub fn skipDecorrelateProjectionForLeftOuterApply(...)`：仅对 Left Outer Apply 与 Projection 组合生效；空表达式列表不会被当作“全常量”。
- `collect_correlated_edges`、`strip_correlated_conditions`：模块私有的谓词提升配对函数。
- `_expression(Expression)`：未调用的迁移占位函数，仅保留 `Expression` 类型引用，不承载运行时逻辑。

## 执行流程

1. 调用者构造 `DecorrelateSolver` 并调用 `Optimize`；入口创建空 `HashSet<usize>` 作为祖先聚合分组列上下文。
2. `optimize` 对 Apply 先递归优化左右子树。若允许去相关且全部 `correlated_columns` 都存在于优化后左子树 schema，则扫描右子树的相关等值谓词。
3. 扫描只识别谓词名严格等于 `correlated_eq` 的项目。每个命中项使用谓词的 `column` 作为内侧列，并始终使用 `outer.first()` 作为外侧列，生成 `null_equal == false` 的 `JoinEdge`。
4. 若产生至少一条边，或者 Apply 本来没有相关列，则构造同 `join_type` 的 `JoinNode::Join`，保留左右子树，清除已提升的右侧谓词，并把 `other_conditions` 置空、`preferred_method` 置为 `None`。否则保留 Apply。
5. 遇到 Aggregation 时，将本节点 `group_by` 与祖先集合并后传给子树；Projection 和 Join 只递归重建子节点。Selection、Window、UnionAll、Leaf 等在此优化递归中不下降。
6. 每个已处理节点最后调用 `pruneRedundantApply`。匹配条件成立时以左子树替代整个 Apply；最终变化标志为子树改写、当前改写或剪枝结果的逻辑或。
7. 独立的相关列提取流程不走 `Optimize`：它遍历所有支持的节点，先按遇见顺序去重收集相关列，再删除出现在任一内部 Apply 左侧 schema 中的列。

## 数据与状态

核心数据全部由值传递的 `JoinPlan` 及其 `JoinNode` 枚举承载。列身份被简化为 `usize`；schema 是 `Vec<usize>`；相关列是 `Vec<usize>`；连接边是 `JoinEdge { left_column, right_column, null_equal }`。`JoinPlan::contains_column` 与 `columns` 是本规则判断列归属的基础。

`optimize` 拥有传入计划并重建节点，只有 `groupByColumn: &HashSet<usize>` 是只读借用的递归上下文。Aggregation 分支创建新的合并集合，不会回写祖先。相关列提取器使用局部 `Vec` 和 `HashSet`；`Vec::contains` 实现首次出现保序去重。`aggDefaultValueMap` 返回克隆，调用者修改结果不会改变原计划。

变化标志只表达“该次规则遍历是否报告改写”，不记录改写种类。计划的 `id`、顶层 `schema` 和 `row_count` 保存在外层 `JoinPlan` 中；把 Apply 节点改为 Join 时沿用这些外层字段，而剪枝时则直接采用左子树自己的全部字段。

## 依赖与调用关系

直接下游依赖均为 crate 内部模型：`crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result}` 和 `crate::task::{Expression, JoinType}`，另使用标准库 `HashMap`/`HashSet`。本文件没有直接使用 `Cargo.toml` 中的外部 crate；`Result<T>` 实际是 `std::result::Result<T, String>` 的别名。

模块由 `lib.rs` 公开。RustCodeGraph 对目标文件列出了 `planbuilder.rs`、`point_get_plan.rs`、`rule_decorrelate_test.rs` 的文件级使用关系，但精确调用查询没有给出可用调用边；用仓库引用搜索补证后，明确的函数级上游是：

- `rule_decorrelate_test.rs` 调用相关列提取和投影保护函数；
- `casetest/windows/window_with_exist_subquery_test.rs` 调用 `DecorrelateSolver::Optimize`，验证窗口子查询、嵌套 Apply 和缺失外侧相关列等计划形状；
- `optimizer_runtime.rs` 只在注释中提及 Go 同名投影保护规则，其生产运行时使用自身的 `skip_left_outer_apply_projection` 等实现，并未直接调用本模块函数。

因此，安全的架构结论是“本模块向 crate 导出简化规则并被测试覆盖”，而不是“所有 SQL 请求必经本模块”。

## 错误处理与边界

`Optimize` 的签名返回 `Result`，但当前实现没有构造 `Err`；递归中的 `?` 只传播未来可能加入的错误。调用者仍应按可失败 API 处理，不能依赖永远成功这一实现细节。

主要边界来自模型简化：

- `collect_correlated_edges` 只看 Leaf，或沿 Selection 子链向下；遇到 Projection、Aggregation、Window、Join、Apply、UnionAll 即停止。它不解析真实表达式，仅依赖 `Expression.name == "correlated_eq"` 和 `column`。
- 多个外侧相关列存在时，所有边都绑定 `outer.first()`；当前代码没有按谓词关联到不同外侧列，也不生成 NULL-safe 等值边。
- Apply 改写会把 `other_conditions` 初始化为空，无法携带简化模型之外的连接条件；扩展时必须防止条件丢失或重复执行。
- `strip_correlated_conditions` 与收集器覆盖相同的 Leaf/Selection 形状，且会删除所有同名谓词；如果同名谓词并未成功转成边，错误扩展可能改变语义。
- `pruneRedundantApply` 依赖浮点 `row_count <= 1.0` 和空相关列，而 Go 版本检查的是 Selection + Apply、谓词恒真、连接类型及 lateral 等更复杂条件；两者不能互换证明。
- 投影保护函数只判断“全部常量”或“全部为左侧列”。它不实现 Go 版本对相关表达式、混合外侧表达式以及内侧表达式 NULL 保持性的完整检查。
- 相关列提取没有 Go 版本的 nil 计划分支，也没有 PhysicalCTE 的 seed/recur 专门语义；它只遍历 `JoinNode` 能表示的形状。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件或网络资源。`DecorrelateSolver` 无内部可变状态，可由调用者重复构造或复用；单次调用的所有集合和计划所有权都局限在同步调用栈中。

递归深度与计划树高度成正比。相关列的保序去重使用 `Vec::contains`，筛除阶段又逐列扫描所有 Apply schema；在相关列或内部 Apply 很多时可能出现二次量级比较。`strip_correlated_conditions` 为递归更新 Selection 子树而克隆 child 计划，深链可能增加分配成本。当前没有缓存、取消或递归深度保护。

## 与 Go 版本的对应关系

同路径 [`rule_decorrelate.go`](rule_decorrelate.go) 是语义对照源，但 Rust 文件只迁移了名称和部分意图。

- 相关列提取的核心不变量一致：收集子计划相关列，并排除由内部 Apply 外侧 schema 拥有的列。Go 在 `base.PhysicalPlan` 上递归任意 Children，并特殊遍历 `PhysicalCTE` 的 Seed/Recur；Rust 在封闭的 `JoinNode` 枚举上逐变体遍历并以列编号代表列对象。
- Go `DecorrelateSolver.optimize` 处理真实 `LogicalPlan`，包含无相关 Apply、Selection、MaxOneRow、Projection、Limit、Aggregation、Sort、CTE、lateral、默认聚合值与多类连接条件。Rust `optimize` 只在简化树上把可识别 `correlated_eq` 的 Apply 变 Join，并递归四类节点；不能视为 Go 算法的完整端口。
- Go `aggDefaultValueMap` 按聚合函数生成 COUNT/BIT_OR/BIT_XOR 的零值和 BIT_AND 的无符号最大值；Rust 方法只读取已存入 `JoinNode::Aggregation.default_values` 的映射，而且当前未被优化流程调用。
- Go `pruneRedundantApply` 要求上层 Selection 简化为恒真，并处理 LeftOuter/LeftOuterSemi、lateral Apply 链和分组列保留；Rust 直接检查一个 LeftOuter Apply 的相关列、右侧估算行数和分组列，语义明显更窄且判据不同。
- Go 投影保护会拒绝全常量、涉及外侧列的表达式，以及对 NULL 扩展不保持 NULL 的内侧表达式；Rust 只覆盖测试确认的“全常量”和“全部简单外侧列”两类。

这些差异是当前代码事实，不应在文档或新测试中用“等价实现”概括。若要继续移植，应以 Go 分支逐项建立独立 Rust 回归测试，而不是扩大现有简化判据来勉强通过单个用例。

## 扩展指南

扩展本规则时优先修改最小责任点，并把测试放在独立文件中：模块级行为补充到 `rule_decorrelate_test.rs`；跨算子计划形状可补到 `casetest/windows/window_with_exist_subquery_test.rs` 或对应的独立 casetest 文件，不能把测试内嵌进生产 `.rs`。

- 新增可穿透节点时，同时审查 `DecorrelateSolver::optimize`、`collect_correlated_edges` 和 `strip_correlated_conditions` 的遍历对称性；只扩展收集或只扩展删除都会带来漏改写或条件丢失风险。
- 支持多个相关列时，应先让表达式模型能明确表示外侧列，再修改边提取，不能继续默认 `outer.first()`。
- 增加 Projection、Aggregation、MaxOneRow、Limit 或 Sort 的 Go 对齐逻辑时，应逐分支复制 Go 的语义前置条件，特别验证 Left Outer Join 的 NULL 扩展、标量子查询基数、HAVING 与空输入默认值。
- 修改剪枝前应建立“错误剪枝会丢行”的反例，包括 lateral、多行右侧、分组列只存在于 Apply 输出以及非恒真过滤；`row_count` 是估计值，不宜被扩展为更激进的正确性证明。
- 若将本规则接入生产优化器，需要在明确的优化规则注册/调度点接线，并确认它与 `optimizer_runtime.rs` 的现有去相关实现不会重复运行或采用冲突模型。
- 性能变化应关注深树递归、`Vec::contains` 去重和子树克隆；兼容性则以 Go 同路径实现与 SQL 结果/计划回归为准。

## 验证依据

本说明基于以下直接证据（仅做静态分析，按任务要求未运行 Cargo）：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可由 `node --file pkg/planner/core/rule_decorrelate.rs` 完整读取。
- RustCodeGraph `query`：确认 Rust 与 Go 两侧的 `DecorrelateSolver`、`ExtractOuterApplyCorrelatedCols`、`pruneRedundantApply`、`skipDecorrelateProjectionForLeftOuterApply`，以及 Rust 私有 `collect_correlated_edges`、`strip_correlated_conditions` 的路径和签名。精确 callers/callees 查询未产出结果并被停止，因此调用者结论另由仓库引用搜索核验。
- 源与模块边界：`pkg/planner/core/rule_decorrelate.rs`、`rule_join_reorder.rs`、`task.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`pkg/planner/core/rule_decorrelate.go` 的相关列提取、默认聚合值、剪枝、主优化递归和 Left Outer Apply 投影保护实现。
- 独立 Rust 测试：`pkg/planner/core/rule_decorrelate_test.rs` 验证内部 Apply 相关列排除、常量/外侧/内侧列投影判断；`pkg/planner/core/casetest/windows/window_with_exist_subquery_test.rs` 验证相关和非相关 Apply 改写、嵌套计划保形及外侧列缺失时不改写。
- 生产路径旁证：`pkg/planner/core/optimizer_runtime.rs` 包含自身的 MaxOneRow 与 Left Outer Apply 投影保护逻辑，说明完整运行时行为不能仅由本文件推断。

人工复核结论：本文能从真实符号回答该文件为何存在、当前如何运行、哪些语义尚未迁移以及扩展时应同步哪些独立测试；未把测试覆盖之外的能力表述为已支持。
