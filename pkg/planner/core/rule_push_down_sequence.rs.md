# `pkg/planner/core/rule_push_down_sequence.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 由 [`pkg/planner/core/Cargo.toml`](Cargo.toml) 声明，库根 [`pkg/planner/core/lib.rs`](lib.rs) 通过 `pub mod rule_push_down_sequence` 将其作为公开模块编译和暴露。文件在 [`task.rs`](task.rs) 的轻量 `PlanNode`/`PlanKind` 计划树上实现 Sequence 下推规则，目标是把承载 CTE 与主查询执行顺序的 `Sequence` 节点沿主查询的一元算子链下移。

需要区分“本文件提供的公开轻量规则”与“完整 SQL 优化主链中的生产实现”。RustCodeGraph 显示，本文件的 `PushDownSequenceSolver` 当前只由独立测试 [`rule_push_down_sequence_test.rs`](rule_push_down_sequence_test.rs) 导入；完整逻辑计划的规则枚举、调度和下推实现位于 [`optimizer_runtime.rs`](optimizer_runtime.rs) 的 `LogicalRule::PushDownSequence`、`push_down_sequence` 与 `push_down_sequence_owned`。因此，本文件不是当前真实 `logicalop::LogicalPlanRef` 优化链的直接入口，而是同一 Go 规则在简化计划模型上的独立移植。

## 核心职责

1. 提供稳定规则名 `push_down_sequence`，与 Go `LogicalOptRule.Name` 保持一致。
2. 遍历整棵 `PlanNode` 树，在每个尚无待下推 Sequence 的子树中寻找 `PlanKind::Sequence`。
3. 遇到 Sequence 时把最后一个孩子视为主查询，把此前孩子视为需先执行的 CTE；沿主查询继续递归。
4. 连续遇到嵌套 Sequence 时，按“外层 CTE 在前、内层 CTE 在后、最终主查询在末尾”的顺序合并孩子。
5. 待下推 Sequence 可穿过恰有一个孩子的任意节点；到达零孩子或多孩子节点时停止，并把该子树重新挂为 Sequence 的最后一个孩子。

本文件只重排节点层级和 `children`，不改变表达式、schema、统计、代价或其他 `PlanNode` 字段，也不判断优化是否实际改变了树。

## 主要符号

- `PushDownSequenceSolver`：无字段的零大小规则类型，派生 `Default`，不保存跨调用状态。
- `PushDownSequenceSolver::Name(&self) -> &'static str`：返回固定注册名 `push_down_sequence`。方法是公开 API，但当前轻量优化器 [`optimizer.rs`](optimizer.rs) 的规则数组没有实例化该类型。
- `PushDownSequenceSolver::Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：公开入口，以 `pushedSequence = None` 调用递归实现；返回重写后的计划，第二个值始终为 `false`，对应 Go 实现固定不报告 `planChanged`。
- `PushDownSequenceSolver::recursiveOptimize(&self, pushedSequence: Option<PlanNode>, p: PlanNode) -> (PlanNode, bool)`：公开的递归核心。`pushedSequence` 表示已从祖先位置取下、等待重新附着的 Sequence；返回值中的布尔量在所有分支均为 `false`。

文件没有模块级常量、trait 实现、条件编译项、错误类型或可变静态状态。命名保留 Go 风格的 `Name`、`Optimize`、`recursiveOptimize`；crate 根通过 `#![allow(non_snake_case)]` 允许这种命名。

## 执行流程

`Optimize` 从根节点启动一次深度优先重写：

1. 若当前节点是 `PlanKind::Sequence`，先用 `children.pop()` 取最后一个孩子作为 `main_query`。若没有孩子，保持空 Sequence 原样返回。
2. 若此前已有外层 `pushedSequence`，先删除外层最后一个主查询占位，再取出外层剩余 CTE；随后把当前 Sequence 的剩余孩子追加到这些 CTE 后。当前 `p` 保留内层节点除孩子之外的字段，并以合并后的 CTE 替换其孩子。
3. 把 `main_query.clone()` 放回当前 Sequence 的末尾，使其成为新的待下推 Sequence；递归时同时按值传入原 `main_query` 作为当前节点。最终重新附着时会删除这份占位并用重写后的主查询替换，因此不会在结果树中重复主查询。
4. 若当前不是 Sequence 且没有待下推节点，则对每个孩子分别以 `None` 递归，收集重写后的孩子并保持当前节点。这保证根下不同分支中的 Sequence 独立优化。
5. 若已有待下推 Sequence 且当前节点恰有一个孩子，则取出该孩子继续携带 Sequence 递归，并把返回结果作为当前节点唯一孩子；由此 Sequence 穿过 Projection、Selection、Sort、Limit 等一元节点，也同样会穿过轻量模型中的任何其他一元节点。
6. 若当前节点有零个或多个孩子，则停止下推：删除待下推 Sequence 原来的最后一个主查询占位，把当前完整子树追加为新末尾孩子，并返回 Sequence。

独立测试验证了三条主路径：非 Sequence 根的每棵子树都会被访问；Sequence 穿过一元 `HashAgg` 且主查询不重复；嵌套 Sequence 合并后保持外层 CTE、内层 CTE、主查询的顺序。

## 数据与状态

输入和输出均是按值拥有的 `PlanNode`。算法用 `Option<PlanNode>` 携带待下推 Sequence，没有共享可变引用或全局状态；孩子通过 `pop`、`remove(0)`、`append` 和 `push` 在所有权下移动。嵌套合并时 `outer.children` 被整体移出，避免逐项克隆；只有当前 Sequence 的 `main_query` 在建立占位时克隆一次。

关键结构不变量是：非空 Sequence 的最后一个孩子是主查询，之前所有孩子是按执行顺序排列的 CTE。携带中的 `pushedSequence` 也必须保留一个末尾主查询占位，因为停止下推时会无条件 `pop()` 再追加当前子树。这个不变量只由 `recursiveOptimize` 的内部调用路径建立；虽然该方法是 `pub`，外部调用者若直接传入没有孩子的 `pushedSequence`，`pop()` 会返回 `None` 但不会 panic，随后仍会追加当前节点。

本文件只读取 `PlanNode.kind` 和 `children`；Sequence 自身的其余字段在单层场景保留原值，在嵌套场景则以最内层当前 `p` 的字段为主体。轻量 `PlanNode` 不具有 Go `LogicalPlan` 的 session context、query block offset、输出名称等专用元数据。

## 依赖与调用关系

直接 Rust 依赖只有同 crate 的 `crate::task::{PlanKind, PlanNode}`；本文件没有直接使用 `Cargo.toml` 中声明的第三方依赖或 `nextgen` feature。`lib.rs` 公开声明本模块，并在 `#[cfg(test)]` 下装配相邻的独立测试模块。

RustCodeGraph 的文件关系显示 [`rule_push_down_sequence_test.rs`](rule_push_down_sequence_test.rs) 是本文件唯一直接使用方。图中 `Optimize -> recursiveOptimize`，后者通过自身递归处理 Sequence 主查询、普通节点孩子和一元算子链；图对标准容器方法的候选解析存在跨文件噪声，因此 `pop`、`append`、`remove`、`push` 的确切语义以本文件源码为准。

完整应用中的对应调用链是 [`optimizer_runtime.rs`](optimizer_runtime.rs) 的 `LOGICAL_RULES` 包含 `LogicalRule::PushDownSequence`，逻辑规则分派将其交给 `push_down_sequence`，再由 `push_down_sequence_owned` 递归并由 `attach_pushed_sequence` 重建真实 `LogicalSequence`。这些函数实现相同规则意图，但不是本文件方法的调用者。另一套轻量入口 [`optimizer.rs`](optimizer.rs) 的固定规则名数组不包含 `push_down_sequence`，也没有引用 `PushDownSequenceSolver`。

## 错误处理与边界

本文件没有 `Result`、日志或显式错误传播；所有分支都返回计划和 `false`。主要边界如下：

- 空 Sequence：`children.pop()` 返回 `None` 时原样返回，不会像直接索引最后一个孩子那样 panic；但这属于防御行为，空 Sequence 没有正常的主查询语义。
- 零孩子节点：存在待下推 Sequence 时归入停止分支，作为 Sequence 的末尾主查询；这覆盖常量折叠可能产生的无孩子叶节点。
- 多孩子节点：Join 等分支节点阻断下推，避免把 Sequence 放进某一条分支而改变执行顺序。
- 一元节点：仅按孩子数量判定可穿透，不检查节点种类或副作用；新增一元算子若不允许 Sequence 穿透，必须显式增加边界条件。
- 嵌套 Sequence：合并顺序明确，但通过 `main_query.clone()` 产生一次整棵子树克隆，深或宽主查询可能增加瞬时内存与时间开销。
- 公开递归入口：外部可以构造不满足“末尾孩子为主查询”的 `pushedSequence`；当前类型系统不编码该不变量。
- 递归深度：与计划树深度及可穿透一元链长度一致，没有深度限制或迭代化保护。

## 并发与资源生命周期

规则不创建线程、异步任务、通道、锁、事务、文件或网络资源。`PushDownSequenceSolver` 无内部状态，可以在不同调用中独立复用；单次调用的全部状态由递归栈和按值拥有的计划节点构成。

重写过程中，被取出的孩子和 Sequence 都由当前栈帧拥有并在返回值中重新组装；未进入返回树的临时值在离开作用域时释放。`main_query.clone()` 是唯一显式共享数据复制点，但复制后两份节点仍各自拥有，不存在别名可变性。规则本身没有并发同步需求；调用方若在多线程间共享计划，需在进入本按值 API 前完成所有权协调。

## 与 Go 版本的对应关系

直接对照文件是 [`rule_push_down_sequence.go`](rule_push_down_sequence.go)。两侧都使用相同规则名，`Optimize` 都从无待下推 Sequence 开始，并固定报告 `planChanged = false`；都把 Sequence 最后一个孩子视为主查询、把此前孩子视为 CTE，都按外层到内层顺序合并嵌套 CTE，并允许穿过恰有一个孩子的普通逻辑算子。

Rust 轻量版本与 Go 完整实现存在这些明确差异：

- Go 使用 `base.LogicalPlan` 与 `logicalop.LogicalSequence`，重建嵌套 Sequence 时保留当前逻辑计划的 session context 和 query block offset；Rust 使用通用 `PlanNode`，只能保留节点结构和其中已有字段。
- Go 对 `DataSource`、`LogicalCTE` 设置显式停止点，并会先以 `nil` 递归处理该节点；Rust 仅以孩子数量判断，所以带一个孩子的 `PlanKind::Cte` 也会被穿透，零孩子 `TableScan`/`Cte` 才自然停止。
- Go 假设 Sequence 至少有一个孩子并直接访问最后一个孩子；Rust 用 `let Some(...)` 防御空 Sequence。
- Go 重建嵌套 Sequence 使用新的 `LogicalSequence{}.Init(...)`；Rust 复用当前内层 `p` 的非孩子字段。
- Go 通过接口方法 `SetChildren`/`SetChild` 操作多态计划；Rust 直接移动 `Vec<PlanNode>`。Rust 为占位显式克隆主查询，而 Go 保存逻辑计划接口引用。

Go 生产规则注册在 [`optimizer.go`](optimizer.go) 的 `optRuleList`，标志位是 `rule.FlagPushDownSequence`。Go 回归 [`issuetest/panicrisk_tier2_test.go`](issuetest/panicrisk_tier2_test.go) 的 `TestPushDownSequenceWithTableDual` 验证共享 CTE 与常量假条件产生无孩子 `LogicalTableDual` 时不会错误索引孩子。Rust 完整主链的 SQL 级对应测试位于 [`optimizer_logical_entry_aster_unit_test.rs`](optimizer_logical_entry_aster_unit_test.rs) 的 `push_down_sequence_moves_sequence_below_unary_main_query`，但它验证的是 `optimizer_runtime.rs` 的真实 `LogicalPlanRef` 实现，而非本文件轻量 Solver。

## 扩展指南

若修改本轻量规则，规则名与入口契约应改在 `Name`/`Optimize`，Sequence 识别、嵌套合并和停止条件应改在 `recursiveOptimize`。Rust 测试不要写回生产源文件；应同步扩展同目录 [`rule_push_down_sequence_test.rs`](rule_push_down_sequence_test.rs)，至少覆盖空 Sequence、零孩子叶节点、多孩子 Join、嵌套顺序和新增的一元阻断算子。

任何面向完整 SQL 计划的功能扩展还必须同步审视 [`optimizer_runtime.rs`](optimizer_runtime.rs) 的 `push_down_sequence_owned`/`attach_pushed_sequence` 及其独立测试，因为生产调度不调用本 Solver。若未来要消除双实现，应让公开轻量 API 明确委托生产规则或删除不再需要的入口，而不是让两份递归逻辑继续漂移。

兼容性风险主要是错误穿透具有执行顺序或副作用语义的一元算子，以及改变 CTE 合并顺序；正确性风险还包括丢失真实逻辑节点的 context、query block、输出名称或 schema。性能风险主要来自主查询深克隆和深树递归。新增节点种类时，应同时回答它是否允许穿透、停止后 Sequence 应挂在何处、是否需要独立优化其孩子，并对照 Go 行为补充测试。

## 验证依据

- 目标源码与数据模型：[`rule_push_down_sequence.rs`](rule_push_down_sequence.rs)、[`task.rs`](task.rs)；模块装配与 crate 边界：[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`node --file pkg/planner/core/rule_push_down_sequence.rs --offset 1 --limit 400` 读取完整 68 行源码；`query PushDownSequenceSolver`、`query recursiveOptimize` 定位 Rust/Go 同名定义；`node PushDownSequenceSolver` 显示 Rust 类型仅由测试模块导入；`callers`/`callees` 查询用于核对递归关系并识别标准容器方法解析噪声。
- Rust 轻量规则测试：[`rule_push_down_sequence_test.rs`](rule_push_down_sequence_test.rs)，覆盖普通树递归、一元穿透不重复主查询、嵌套 CTE 顺序。
- Rust 生产接线与 SQL 级测试：[`optimizer_runtime.rs`](optimizer_runtime.rs) 中 `LogicalRule::PushDownSequence`、`LOGICAL_RULES`、规则分派、`push_down_sequence_owned`、`attach_pushed_sequence`；[`optimizer_logical_entry_aster_unit_test.rs`](optimizer_logical_entry_aster_unit_test.rs) 中 `push_down_sequence_moves_sequence_below_unary_main_query`。
- Go 对照与回归：[`rule_push_down_sequence.go`](rule_push_down_sequence.go)、[`optimizer.go`](optimizer.go)、[`issuetest/panicrisk_tier2_test.go`](issuetest/panicrisk_tier2_test.go)。
- 本任务只新增说明文档，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工确认文档回答了文件为何存在、算法如何运行、生产接线在哪里、Go 差异与安全扩展位置。
