# `pkg/planner/core/rule_eliminate_empty_selection.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 将 crate 根指定为 `lib.rs`，而 [`pkg/planner/core/lib.rs`](lib.rs) 通过 `pub mod rule_eliminate_empty_selection` 公开本模块，并仅在 `cfg(test)` 下挂载同目录的独立测试文件 `rule_eliminate_empty_selection_test.rs`。

本文件提供基于 [`task::PlanNode`](task.rs) 的空 Selection 消除规则。这里的 `PlanNode` 是按值拥有子节点的通用计划树表示；完整应用的生产逻辑优化流水线使用 `logicalop::LogicalPlanRef`，并在 [`optimizer_runtime.rs`](optimizer_runtime.rs) 中由 `LogicalRule::EmptySelectionEliminator` 分支调用独立的 `eliminate_empty_selection_descendants`。因此，本文件是公开的 `PlanNode` 版规则实现和 Go 语义移植面，不是生产流水线对 trait-object 逻辑计划执行改写时直接调用的函数。

## 核心职责

`EmptySelectionEliminator` 负责移除“作为其他节点孩子出现、且 `conditions` 为空”的 `PlanKind::Selection`。空过滤没有谓词，不改变其唯一孩子产生的行，因此父节点可以直接接管该孩子；带谓词的 Selection、非 Selection 节点以及调用入口自身的根节点均保留。

规则还保留 Go 版本的两个外部契约：`Optimize` 固定报告 `changed == false`，`Name` 固定返回注册名 `"eliminate_empty_selection"`。这里的布尔值不能被理解为实际结构没有变化；独立测试证明树可能已被改写而返回值仍为 `false`。

## 主要符号

- `pub struct EmptySelectionEliminator`：无字段、零状态的规则对象，派生 `Default`。调用方既可写 `EmptySelectionEliminator` 直接构造，也可使用默认构造。
- `pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：消费一棵 `PlanNode` 树，调用 `recursivePlan` 改写其后代，返回新树以及固定的 `false`。方法名沿用 Go 风格；crate 根的 `#![allow(non_snake_case)]` 允许该命名。
- `pub fn recursivePlan(&self, mut p: PlanNode) -> PlanNode`：核心递归。它消费 `p.children`，逐个决定是否用空 Selection 的零号孩子替换当前孩子，再把递归后的节点收集回 `p.children`。
- `pub fn Name(&self) -> &'static str`：返回静态规则名，不分配字符串，也不读取运行时配置。
- `PlanKind::Selection`、`PlanNode::{kind, children, conditions}`：规则实际读取或修改的计划数据。其余统计、schema、表达式、代价等字段随保留节点整体移动，不在本文件中重算。

## 执行流程

1. 调用方把根 `PlanNode` 按值传给 `Optimize`。
2. `Optimize` 把根交给 `recursivePlan`；它不会先判断根是否为空 Selection，因此规则只检查根的孩子，根节点自身始终保留。
3. `recursivePlan` 通过 `into_iter()` 取得每个孩子的所有权。
4. 若孩子同时满足 `kind == PlanKind::Selection` 和 `conditions.is_empty()`，则移除该 Selection 的 `children[0]`，递归处理这个零号孩子，并把递归结果放回父节点。被移除 Selection 自身的其他字段以及除零号以外的孩子随该节点一起丢弃；合法计划要求 Selection 为一元算子。
5. 其他孩子原样递归，因此其更深层后代仍会接受同一规则检查。
6. 收集出的新孩子向量替换 `p.children`，随后返回 `p`；`Optimize` 再包装为 `(p, false)`。

连续空 Selection 的处理遵循 Go 实现的递归位置：父节点遇到外层空 Selection 时先用其零号孩子替换，再对该孩子的后代递归，而不会重新把这个替代节点作为当前父节点的孩子检查一次。因此一趟调用可能保留连续链中靠内的一层空 Selection；`recursive_plan_eliminates_only_empty_selection_children` 正是这一行为的回归证据。

## 数据与状态

规则对象自身没有可变状态。计划树完全由入参拥有，改写通过移动 `PlanNode` 和重建每个节点的 `children: Vec<PlanNode>` 完成，没有共享引用、全局注册表或隐式缓存。

判定只依赖 `PlanNode.kind` 与 `PlanNode.conditions`；替换只操作 `PlanNode.children`。对于保留下来的节点，`stats`、`schema`、`expressions`、`by_items`、`group_items`、`agg_funcs`、物理落点、标志和代价字段均随节点移动而保持原值。对于被消除的 Selection，这些节点级元数据不会合并到孩子，故正确性依赖“空 Selection 只是语义透明包装层”这一计划构造不变量。

## 依赖与调用关系

本文件只有一个直接 Rust 依赖：`crate::task::{PlanKind, PlanNode}`。它没有直接使用 `pkg/planner/core/Cargo.toml` 中的外部 crate，也不受 `nextgen` feature 条件编译控制。

RustCodeGraph 显示 `Optimize -> recursivePlan` 的直接调用边，而 `recursivePlan` 通过自身递归遍历子树。索引同时显示本模块被 `rule_eliminate_empty_selection_test.rs` 导入；跨 crate 的 `pkg/planner/core/casetest/rule/rule_eliminate_empty_selection_test.rs` 也从公开模块导入该规则并直接执行。

生产优化主链的对应接线位于 `optimizer_runtime.rs`：`LOGICAL_RULES` 把 `LogicalRule::EmptySelectionEliminator` 排在 `EliminateUnionAllDualItem` 之后、`FullTextIndexResolveReject` 之前；`logical_optimize_in_place` 在对应 flag 开启时匹配该枚举值并调用 `eliminate_empty_selection_descendants(plan.as_mut())`。该调用关系是“同一规则语义的生产实现”，不是对本文件 `Optimize` 的调用。

## 错误处理与边界

本文件不返回 `Result`，也没有可恢复错误分支。主要边界如下：

- 空 Selection 必须至少有一个孩子；代码执行 `child.children.remove(0)`，零孩子会因越界而 panic。这与 Go 版本读取 `sel.Children()[0]` 的计划树不变量一致。
- 代码没有显式验证 Selection 恰有一个孩子。若输入非法地包含多个孩子，只保留零号孩子，其余孩子被丢弃。调用方必须在进入规则前维护一元 Selection 不变量。
- 根节点即使是无条件 Selection 也不会被消除，因为递归函数只检查 `p.children`。同目录测试 `optimize_matches_go_root_and_change_contract` 固定了该行为。
- `conditions` 非空时，无论表达式内容为何都保留 Selection；规则不做常量折叠、真假判定或谓词简化。
- `changed` 始终为 `false`，不能用于判断树是否发生结构变化。

生产流水线的 `eliminate_empty_selection_descendants` 额外要求 `Children().len() == 1` 才替换节点，所以面对非法多孩子或零孩子 Selection 时比本文件更保守。扩展本文件时不可假设两套实现对畸形输入完全一致。

## 并发与资源生命周期

该实现没有线程、异步任务、锁、通道、事务或外部资源。`&self` 只借用一个无状态对象，实际生命周期由被消费的 `PlanNode` 树决定。

遍历期间，每个节点的孩子向量被移动并重新收集；被消除节点在其零号孩子移出后于当前闭包结束时释放，保留节点在返回树中继续存活。递归深度等于相关计划树深度，极端深树可能受调用栈限制；时间复杂度通常为访问节点数的线性量级，重建孩子向量会产生与各节点子节点数量相称的分配/移动成本。

## 与 Go 版本的对应关系

直接对照文件是 [`rule_eliminate_empty_selection.go`](rule_eliminate_empty_selection.go)。符号逐项对应：Rust `EmptySelectionEliminator` 对应 Go 同名结构体，Rust `Optimize` 对应 `Optimize(context.Context, base.LogicalPlan)`，Rust `recursivePlan` 对应 Go 同名递归方法，Rust `Name` 与 Go 返回完全相同的规则名。

两版的正常输入语义一致：不消除根、只消除作为孩子出现的无谓词 Selection、递归遍历其他孩子，并固定报告 `planChanged=false`。类型与错误面不同：Go 接口返回 `(plan, bool, error)` 且当前错误为 `nil`；Rust `PlanNode` 版省略上下文和错误，只返回 `(PlanNode, bool)`。Go 通过 `*logicalop.LogicalSelection` 类型断言识别算子，Rust 通过 `PlanKind::Selection` 枚举值识别。

Go 的 SQL 级回归位于 `pkg/planner/core/casetest/rule/rule_eliminate_empty_selection_test.go`，在 cascades 开/关两种模式下校验复杂查询的 `EXPLAIN plan_tree`。Rust 的同路径 casetest 移植了这组期望，并增加直接构造 `PlanNode` 的无条件/有条件分支检查；同目录单元测试则更精确地覆盖根保留、连续空 Selection 的单趟行为、条件 Selection 保留、固定 change flag 和规则名。

## 扩展指南

若要调整本规则，首先确认修改针对哪个表示层：`PlanNode` 版应改本文件，并同步 `rule_eliminate_empty_selection_test.rs`；真实 `logicalop::LogicalPlanRef` 流水线行为还必须审查和同步 `optimizer_runtime.rs::eliminate_empty_selection_descendants` 及 casetest，避免两套实现漂移。

新增可消除条件时，最可能修改 `recursivePlan` 的分支判定。必须保留 Selection 一元性、根节点契约和谓词语义；若希望一次消除整条连续空 Selection 链，需要明确这是相对现有 Go 行为的改变，并同时更新 Go 对照或记录有意差异。若要改变 `changed`，还需检查逻辑规则调度方对该返回值的约定，不能仅根据树发生改写就自行改成 `true`。

测试应继续放在独立文件中，不得嵌入生产 `.rs`。最低覆盖应包括：根空 Selection、父节点下单层与连续多层空 Selection、带谓词 Selection、零孩子/多孩子等不变量边界（若决定使其可恢复）、多分支父节点以及 `Name`/change flag。性能修改应关注深树递归栈和每层 `Vec` 重建，但不应以跳过后代遍历换取速度。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可由 `node --file pkg/planner/core/rule_eliminate_empty_selection.rs` 完整读取。
- RustCodeGraph `query eliminate_empty_selection` 与 `node/callers/callees`：确认本文件的 `EmptySelectionEliminator`、`Optimize`、`recursivePlan`、`Name`，以及 `Optimize -> recursivePlan` 调用边；通用名称的 callers/callees 查询存在歧义，因此又用文件路径和仓库内精确引用交叉核对。
- 源码证据：`pkg/planner/core/rule_eliminate_empty_selection.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/optimizer_runtime.rs`。
- crate/装配证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/casetest/rule/Cargo.toml`、`pkg/planner/core/casetest/rule/lib.rs`。
- Go 对照与测试证据：`pkg/planner/core/rule_eliminate_empty_selection.go`、`pkg/planner/core/casetest/rule/rule_eliminate_empty_selection_test.go`。
- Rust 测试证据：`pkg/planner/core/rule_eliminate_empty_selection_test.rs`、`pkg/planner/core/casetest/rule/rule_eliminate_empty_selection_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行任务规定的 11 章节结构检查，并人工复核上述源码、调用关系、边界和扩展入口。
