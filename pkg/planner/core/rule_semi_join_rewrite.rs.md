# `pkg/planner/core/rule_semi_join_rewrite.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate，模块由 `pkg/planner/core/lib.rs` 以 `pub mod rule_semi_join_rewrite` 对外公开。它实现一套建立在 `rule_join_reorder.rs::JoinPlan`/`JoinNode` 简化计划模型上的半连接改写规则：把满足条件的 `Semi` Join 变为“右侧按等值连接键去重的 Aggregation + Inner Join + 恢复外表输出的 Projection”。

当前接线必须分两层理解：`SemiJoinRewriter` 被 `rule_semi_join_rewrite_test.rs` 和 `casetest/rule/rule_outer_to_semi_join_test.rs` 直接调用，但 Rust 生产逻辑优化流水线 `optimizer_runtime.rs::logical_optimize_in_place` 对 `LogicalRule::SemiJoinRewrite` 的分派实际调用的是同文件内的 `semi_join_rewrite_descendants`，没有调用本文件的 `SemiJoinRewriter`。因此，本文件是公开且可测试的简化规则实现，并非当前真实 `LogicalPlanRef` 生产执行路径。

## 核心职责

- `SemiJoinRewriter::Optimize` 是规则入口，递归处理整棵 `JoinPlan`，同时保持 Go 规则的特殊返回契约：即使发生结构改写，`planChanged` 仍返回 `false`。
- `SemiJoinRewriter::recursivePlan` 进行后序遍历，先改写子树，再判断当前节点。
- 当前节点仅在 `JoinType::Semi` 且 `other_conditions.is_empty()` 时改写；其他 Join 类型或带其它连接条件的半连接保留原结构，但其后代仍会被处理。
- 改写通过右侧聚合消除同一连接键的重复行，使 Inner Join 不会因内表重复匹配而放大外表行数；最外层 Projection 再把输出 schema 收回到左侧（外表）列，维持半连接“不输出内表列”的语义。

## 主要符号

- `pub struct SemiJoinRewriter`：无字段、可 `Default` 构造的规则对象，不保存跨调用状态。
- `pub fn Optimize(&self, plan: JoinPlan) -> Result<(JoinPlan, bool)>`：公开入口。调用 `recursivePlan`，丢弃内部布尔值，并固定返回 `(plan, false)`。
- `pub fn Name(&self) -> &'static str`：返回稳定规则名 `"semi_join_rewrite"`。本文件内没有注册表消费该名称；Go 对照规则用同名字符串。
- `pub fn recursivePlan(&self, mut plan: JoinPlan) -> Result<(JoinPlan, bool)>`：公开的递归实现。它覆盖 `Join`、`Projection`、`Selection`、`Aggregation`、`Apply`、`Window`、`UnionAll` 的子树遍历，其余节点（当前即 `Leaf`）直接保留。
- 关键输入类型来自 `rule_join_reorder.rs`：`JoinPlan` 保存 `id`、`node`、`schema`、`row_count`；`JoinNode::Join` 保存 Join 类型、左右孩子、等值条件、其它条件和首选方法；`JoinEdge::right_column` 提供内表分组键。
- 投影表达式使用 `task.rs::Expression`，每个外表 schema 列被转换为 `Expression { column: Some(column), ..Default::default() }`；Join 类型使用同文件的简化 `task.rs::JoinType`。

## 执行流程

1. `Optimize` 将根 `JoinPlan` 交给 `recursivePlan`。
2. 对 `JoinNode::Join`，先递归改写 `left` 和 `right`，因此本规则按后序处理，父节点即使不适用也不妨碍子节点改写。
3. 若当前 Join 不是 `JoinType::Semi`，或 `other_conditions` 非空，则用已经处理过的孩子重建原 Join，并保留 `equal_conditions`、`preferred_method` 等字段。
4. 若适用，复制左孩子 schema 为 `outer_schema`，并从每个 `equal_conditions` 收集 `right_column` 作为 `group_by`。当前实现不去重该向量，也不验证列一定属于右孩子。
5. 用原右孩子构造 `JoinNode::Aggregation`：节点 `id` 取右孩子 id，schema 直接设为 `group_by`，`default_values` 为空，`row_count` 沿用待改写 Join 的估计值。
6. 用左孩子和聚合节点构造 `JoinType::Inner` Join。其 schema 从 `outer_schema` 开始，仅追加尚未包含的聚合输出列；Join 的 `id`、`row_count`、等值条件、其它条件和首选连接方法沿用原节点。
7. 构造 Projection，只投影 `outer_schema` 中的列，孩子为新 Inner Join。随后把外层原 `JoinPlan.schema` 改回 `outer_schema`，而外层 `id` 与 `row_count` 保持原值。
8. `Projection`、`Selection`、`Aggregation`、`Window` 递归其唯一孩子；`Apply` 递归左右孩子；`UnionAll` 按原顺序递归全部孩子并收集结果。
9. 每一层都返回布尔值 `false`，最终 `Optimize` 也固定返回 `false`。

## 数据与状态

规则以所有权方式接收并重建 `JoinPlan`，不共享可变状态。`SemiJoinRewriter` 本身是零大小类型；递归过程中的状态只存在于栈帧局部变量中，包括 `rewritten_schema`、`outer_schema`、`group_by` 和新建节点。

重要数据不变量是：改写后顶层 schema 等于原左孩子 schema；右侧聚合 schema 等于所有等值条件的右列序列；Inner Join schema 是外表列加上其中尚未出现的聚合列；最外层 Projection 的表达式顺序严格跟随外表 schema。`id` 的选择也具有稳定含义：外层 Projection/Inner Join 沿用原 Join id，Aggregation 沿用原右孩子 id，但代码没有声明 id 全局唯一性。

`row_count` 没有重新估算：Aggregation、Inner Join 和外层容器都沿用原 Join 的 `row_count`。这是简化模型的事实，不能据此推断生产统计估算已经完成。

## 依赖与调用关系

直接依赖只有 crate 内部模块：`crate::rule_join_reorder::{JoinNode, JoinPlan, Result}` 和 `crate::task::{JoinType, Expression}`。`Cargo.toml` 声明本 crate 名为 `astersql-planner-core`、`lib.rs` 为库入口、`autotests = false`；测试由 `lib.rs` 的显式 `#[cfg(test)]` 模块接入。

RustCodeGraph 的文件反向关系显示，本文件由以下两个测试使用：

- `pkg/planner/core/rule_semi_join_rewrite_test.rs`：直接验证计划形状、schema、连接方法保留和固定的 `changed == false` 契约。
- `pkg/planner/core/casetest/rule/rule_outer_to_semi_join_test.rs`：通过公开 crate API 构造简化计划，验证 Semi Join 变为 Projection(Inner Join(..., Aggregation))。

生产侧对应链为 `optimizer_runtime.rs::logical_optimize_in_place` → `LogicalRule::SemiJoinRewrite` → `semi_join_rewrite_descendants`。该实现操作 `logicalop::LogicalPlanRef`，承担会话变量、hint、真实表达式与输出名等生产语义；它与本文件概念相同但类型和接线独立。

## 错误处理与边界

返回类型 `Result<T>` 是 `rule_join_reorder.rs` 定义的 `std::result::Result<T, String>`。本文件自身没有主动构造错误；错误只会从递归调用以及 `UnionAll` 的 `collect::<Result<Vec<_>>>()` 向上传播。以当前全部分支看，实际路径没有本地失败点，但保留 `Result` 使接口与同一简化规则族一致，也允许未来校验返回错误。

适用边界比 Go/生产 Rust 更窄也更少防护：只认 `JoinType::Semi`，不支持 LeftOuterSemi（简化枚举也无此成员）；只检查 `other_conditions`，模型中没有独立的 left/right conditions、hint、session variable、CTE 或真实 Apply 子类型判定；空 `equal_conditions` 仍会生成空分组键的 Aggregation；`right_column` 未验证归属；重复右键不会去重。代码也不会产生 hint 不适用警告。

输入若带其它条件，当前节点不改写，但 `inapplicable_parent_still_recurses_into_children` 证明其子树仍按规则处理。`preferred_method` 会原样传给替代 Inner Join。未知/叶节点保持不变。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、文件句柄、网络连接或事务资源。调用者把计划树所有权移入规则，规则同步递归并返回一棵重建后的树；`Box` 子节点随所有权移动，由 Rust 自动释放被替换的旧容器。

并发安全主要取决于值所有权：规则对象无内部状态，可被多个调用分别借用；每次调用消费独立的 `JoinPlan`，不会在本文件形成共享写入。递归深度与计划树深度一致，极深的人工计划树可能增加栈使用；`UnionAll` 的临时结果向量和每次改写产生的新节点带来与树规模相关的分配成本。

## 与 Go 版本的对应关系

Go 入口位于同目录 `rule_semi_join_rewrite.go`：`Optimize` 同样递归并故意让 `planChanged` 保持 `false`，`Name` 同样返回 `semi_join_rewrite`，`recursivePlan` 同样先处理孩子再调用 Join 的改写逻辑。真正的 Go 结构变换在 `operator/logicalop/logical_join.go::LogicalJoin.SemiJoinRewrite`。

共同的核心语义是：以内表等值键分组并用聚合去重，转为 Inner Join，再投影外表 schema；连接算法偏好被带入新 Join。Go 版本还具备本简化文件没有表达的约束和行为：跳过 CTE；只响应 hint 或 `tidb_opt_enable_sem_join_rewrite`；拒绝 Apply、LeftOuterSemiJoin、left/other conditions；把 right conditions 物化为 Aggregation 下方的 Selection；为每个分组键建立 `first_row` 聚合描述；维护 session context、query block、输出名、key info 和 hint 警告。

当前生产 Rust 的 `optimizer_runtime.rs::semi_join_rewrite_descendants` 更接近上述 Go 完整语义，并额外尝试把可物化的 `OtherConditions` 提升为等值条件、规范化左右连接键顺序，同时给新 Inner Join 设置 `FromSemiJoinRewrite = true`。因此，修改本文件不会自动改变生产 SQL 优化行为；若目标是行为对齐，必须判断改动属于简化测试模型、生产 `LogicalPlanRef` 路径，还是两者都需要同步。

## 扩展指南

- 增加适用条件或拒绝条件时，优先修改 `recursivePlan` 当前 Join 分支，并在独立的 `rule_semi_join_rewrite_test.rs` 增加回归；不要把测试内嵌进生产 `.rs`。
- 若增加新 `JoinNode` 容器变体，应在本函数补充对子节点的递归，否则深层 Semi Join 可能被跳过；同时核对 `rule_join_reorder.rs` 中该变体的 schema 约定。
- 若改变分组键生成，必须验证多等值键、重复键、空键、左右列方向和内外 schema 重叠。当前代码直接使用 `JoinEdge::right_column`，任何“连接键可能反向”的支持都需要显式规范化。
- 若改变输出结构，必须保持半连接基数与 schema：内表重复行不能放大结果，Projection 的列及顺序必须等于原外表 schema，并检查 `id`、`row_count`、`preferred_method` 是否仍按约定传播。
- 若希望此类型进入生产优化器，不能只注册名称；需处理 `JoinPlan` 与 `LogicalPlanRef` 两套模型的关系，并避免与 `optimizer_runtime.rs::semi_join_rewrite_descendants` 形成双重改写。更现实的做法是先确定唯一实现边界，再同步 Go 的 hint、条件分类、CTE/Apply、错误和统计语义。
- 兼容性风险集中在 NULL/重复行语义、非列等值表达式、LeftOuter/Anti Semi 类型、hint 警告和 schema 列序；性能风险集中在新增 Aggregation 的代价与错误的行数估计。相关 SQL 行为测试可参考 Go 的 `casetest/physicalplantest/physical_plan_test.go` 与 `casetest/rule/rule_outer_to_semi_join_test.go`，但本文件的直接回归应放在对应独立 Rust 测试文件中。

## 验证依据

- 目标源码：`pkg/planner/core/rule_semi_join_rewrite.rs`，RustCodeGraph `node --file` 核对了 153 行完整实现及其两个使用文件。
- 类型与模块：`pkg/planner/core/rule_join_reorder.rs` 的 `Result`、`JoinEdge`、`JoinNode`、`JoinPlan`；`pkg/planner/core/task.rs` 的 `JoinType`、`Expression`；`pkg/planner/core/lib.rs` 的公开模块及显式测试模块声明。
- crate 边界：`pkg/planner/core/Cargo.toml` 的 package、lib、feature、dependency、dev-dependency 和 `go-package = "pkg/planner/core"` 声明。
- 直接 Rust 测试：`pkg/planner/core/rule_semi_join_rewrite_test.rs::semi_join_rewrite_matches_go_plan_shape_and_change_contract`、`inapplicable_parent_still_recurses_into_children`；`pkg/planner/core/casetest/rule/rule_outer_to_semi_join_test.rs::semi_join_rewrite_builds_grouped_inner_join`。
- Go 对照：`pkg/planner/core/rule_semi_join_rewrite.go::{Optimize, Name, recursivePlan}` 与 `pkg/planner/core/operator/logicalop/logical_join.go::LogicalJoin.SemiJoinRewrite`。
- 生产 Rust 对照：`pkg/planner/core/optimizer_runtime.rs::{LOGICAL_RULES, logical_optimize_in_place, semi_join_rewrite_descendants}`；它证明生产规则顺序和真实分派，也证明本文件当前不是该分派的调用目标。
- 结构验证采用任务指定命令，要求文档存在且固定二级章节计数恰为 11；本任务是纯文档分析，按计划不运行 Cargo。
