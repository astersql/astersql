# `pkg/planner/implementation/join.rs`

源文件：[join.rs](join.rs)；同路径 Go 对照：[join.go](join.go)。

## 文件定位

本文件属于 Cargo crate `astersql-planner-implementation`（见同目录 [Cargo.toml](Cargo.toml) 的 `[package]` 与 `[lib] path = "lib.rs"`）。[lib.rs](lib.rs) 以私有模块 `mod join` 装入本文件，并通过 `pub use join::*` 对 crate 使用者公开其类型和构造函数。它位于旧版 Cascades/Memo 优化链的“物理计划候选包装”层：上游 [implementation_rules.rs](../cascades/old/implementation_rules.rs) 先从逻辑 Join 构造 `PhysicalHashJoin` 或 `PhysicalMergeJoin`，再用本文件的构造函数包装成 `ImplementationRef`；[optimize.rs](../cascades/old/optimize.rs) 的 `implGroup` 对候选递归求子计划、计算总代价、剪枝并挂接最终孩子。

本文件不负责选择连接类型、生成连接键、设置孩子所需物理属性，也不执行 Join。它只把已经构造好的二元 Join 物理节点适配到 `astersql_planner_memo::Implementation` 接口，并统一完成代价汇总、计划访问、孩子挂接和剩余代价上限计算。

## 核心职责

1. `binary_join_implementation!` 为二元 Join 生成相同形状的 Implementation 包装，避免 HashJoin 与 MergeJoin 重复实现 Memo 接口。
2. `calc_cost` 从两个孩子的物理计划统计中读取左右行数，调用底层 `BinaryJoinCostPlan::SelfCost`，再加上左右子树已缓存的代价，并把总和写回 `BaseImpl`。
3. `attach_children` 在某候选成为当前最优实现后，克隆两个孩子的物理计划并挂到所包装的 Join 物理节点上。
4. `cost_limit` 把父层允许的总代价减去已找到孩子的代价，为递归搜索下一个孩子提供剪枝上限。
5. 宏的两次调用分别生成 `HashJoinImpl`/`NewHashJoinImpl` 与 `MergeJoinImpl`/`NewMergeJoinImpl`；二者的差异留在实现 `BinaryJoinCostPlan` 的适配器中，而不是本文件中分支判断。

## 主要符号

- `binary_join_implementation!($name, $constructor)`：文件内私有宏。每次展开生成一个公开结构体、一个公开构造函数、四个私有适配方法，以及由 `impl_implementation!` 生成的 `memo::Implementation` trait 实现。
- `HashJoinImpl`：公开的 HashJoin 候选包装。字段 `base: BaseImpl` 在 crate 内可见，用于缓存总代价；`plan_node: Box<dyn BinaryJoinCostPlan>` 是私有 trait object，持有具体物理计划适配器。
- `NewHashJoinImpl(plan: Box<dyn BinaryJoinCostPlan>) -> HashJoinImpl`：公开构造函数。当前直接调用者是 `implementation_rules.rs::getImplForHashJoin`，传入包裹 `PhysicalHashJoin` 的 `PlanAdapter`。
- `MergeJoinImpl`：公开的 MergeJoin 候选包装，字段和行为与 `HashJoinImpl` 相同。
- `NewMergeJoinImpl(plan: Box<dyn BinaryJoinCostPlan>) -> MergeJoinImpl`：公开构造函数。当前直接调用位置是 `implementation_rules.rs::ImplMergeJoin::OnImplement`。
- `calc_cost(&self, _out_count, children)`：要求 `children[0]`、`children[1]` 分别为左、右孩子；忽略父层传入的输出行数，使用孩子 `stats_info().RowCount` 计算 Join 自身代价。
- `plan(&self)`：通过 `BinaryJoinCostPlan` 继承的 `PlanAccess::Plan` 返回底层 `&dyn PhysicalPlan`。
- `attach_children(&mut self, children)`：调用 `base.rs::AttachChildren`，后者克隆每个孩子的物理计划后调用父节点 `set_children`。
- `cost_limit(&self, cost_limit, children)`：委托 `BaseImpl::GetCostLimit`，返回 `cost_limit - sum(children.GetCost())`。
- `impl_implementation!`：定义在 `base.rs`，把上述私有方法映射为 `Implementation::{CalcCost, GetPlan, AttachChildren, GetCostLimit}`，并把 `SetCost`/`GetCost` 映射到 `base`。

文件没有模块级常量、显式 trait 定义、条件编译项或异步入口。

## 执行流程

1. `ImplHashJoinBuildLeft`、`ImplHashJoinBuildRight` 或 `ImplMergeJoin` 在 `implementation_rules.rs` 中匹配逻辑 Join 与父物理属性。
2. HashJoin 路径经 `getImplForHashJoin` 设置 inner/build 侧、并发度、等值条件、统计、schema 和孩子所需属性，然后调用 `NewHashJoinImpl(Box::new(PlanAdapter { plan: physical }))`。MergeJoin 路径生成连接键排序属性、升降序、统计和 schema 后，调用 `NewMergeJoinImpl`。
3. `optimize.rs::implGroup` 读取候选底层计划的 `get_child_req_props(index)`，并在递归寻找每个孩子前调用候选的 `GetCostLimit`。对二元 Join，这会逐步扣除已经选定孩子的缓存代价。
4. 两个孩子均找到后，优化器调用 `CalcCost(out_count, children)`。宏展开的方法读取 `ChildRows(children, 0/1)`，调用 `plan_node.SelfCost(left_rows, right_rows)`，加上 `ChildCost(children, 0/1)`，缓存并返回总成本。`out_count` 对这两类实现没有参与公式。
5. 若总代价超过当前上限，候选被跳过；若优于已有候选，优化器调用 `AttachChildren`。此时孩子计划被克隆并正式装入 Join 物理计划，候选成为当前最优方案。
6. 最终 `onPhaseImplementation` 从胜出 Implementation 读取缓存代价与物理计划，再克隆物理计划作为优化结果。

底层自身代价由 `implementation_rules.rs::PlanAdapter` 转发：HashJoin 调用 `PhysicalHashJoin::GetCost(left, right, false, 0)`，当前 Rust 公式为 `(max(left, 0) + max(right, 0)) / max(concurrency, 1)`；MergeJoin 调用 `PhysicalMergeJoin::GetCost(left, right, 0)`，当前公式为 `max(left, 0) + max(right, 0)`。因此本文件汇总的是“Join 自身估价 + 两个完整子树代价”。

## 数据与状态

- `BaseImpl` 内部用 `Cell<f64>` 保存代价，所以 `calc_cost(&self, ...)` 可在不可变借用下更新缓存。默认值为 `0.0`；优化器在子计划不可得时也可经 trait 的 `SetCost` 将其改为 `f64::MAX` 作为淘汰标记。
- `plan_node` 使用 `Box<dyn BinaryJoinCostPlan>` 擦除具体 Join 类型。该 trait 继承 `PlanAccess`，保证调用方既能取得 `PhysicalPlan`，也能在挂接孩子时取得可变计划。
- `children` 使用 `ImplementationRef`，即 Memo 层共享的引用包装；读取代价或计划时通过 `borrow()` 访问。`AttachChildren` 不把这些共享引用保存进父节点，而是用 `ClonePlan` 复制其物理计划。
- 左右次序是硬约束：索引 `0` 永远作为 left，索引 `1` 作为 right。该次序同时影响行数、自身代价和最终物理树的孩子排列。
- 本文件不拥有统计数据；`ChildRows` 每次从孩子物理计划的 `stats_info().RowCount` 读取。统计的建立与缩放发生在实现规则和其上游 Memo Group 中。

## 依赖与调用关系

上游调用关系：

- `implementation_rules.rs::getImplForHashJoin -> NewHashJoinImpl`；该辅助函数由 `ImplHashJoinBuildLeft::OnImplement` 与 `ImplHashJoinBuildRight::OnImplement` 调用。
- `implementation_rules.rs::ImplMergeJoin::OnImplement -> NewMergeJoinImpl`。
- `optimize.rs::implGroup -> Implementation::{GetCostLimit, CalcCost, AttachChildren}`；具体动态分派到本文件宏生成的实现。

下游调用关系：

- `calc_cost -> ChildRows -> child.GetPlan().stats_info().RowCount`。
- `calc_cost -> BinaryJoinCostPlan::SelfCost`；当前适配器分别转发到 `PhysicalHashJoin::GetCost` 和 `PhysicalMergeJoin::GetCost`。
- `calc_cost -> ChildCost -> child.GetCost`，随后 `BaseImpl::SetCost` 缓存总成本。
- `attach_children -> AttachChildren -> CloneChildren -> ClonePlan -> PhysicalPlan::set_children`。
- `cost_limit -> BaseImpl::GetCostLimit -> child.GetCost`。

crate 依赖边界由 `pkg/planner/implementation/Cargo.toml` 给出：本文件直接使用 `astersql-planner-core-base` 和 `astersql-planner-memo`；`BinaryJoinCostPlan`、`BaseImpl` 与辅助函数来自同 crate。具体 `PhysicalHashJoin`/`PhysicalMergeJoin` 类型并未直接进入本文件，而是在依赖 `astersql-planner-core-operator-physicalop` 的 Cascades 实现规则中通过 `PlanAdapter` 接入。

## 错误处理与边界

- 本文件所有接口均不返回 `Result`，没有可恢复错误路径。构造阶段的逻辑类型、统计、schema 或上下文错误由 `implementation_rules.rs` 在进入本文件前返回 `PlannerError`。
- `calc_cost` 和当前优化器流程都假定恰有两个孩子。少于两个孩子时，`ChildRows`/`ChildCost` 的切片索引会 panic；多出的孩子不会参与成本公式，但 `attach_children` 会克隆并挂接整个切片。因此安全调用的结构不变量是“二元 Join 必须传入且只传入两个按左右排序的孩子”。
- `ChildRows` 依赖每个孩子计划已具有统计信息；其具体 accessor 是否对缺失统计报错或 panic 属于 `PhysicalPlan` 实现边界，本文件不做校验。
- `cost_limit` 不截断负数。当已选孩子成本超过父上限时，负的剩余上限会继续传给递归搜索，由优化器据此找不到可接受实现并淘汰候选。
- 浮点值没有在本文件中检查 `NaN` 或无穷大。底层当前 Join 公式用 `max(0.0)` 处理负行数，但缓存与比较仍遵循 Rust `f64` 语义；异常浮点输入可能影响候选比较。
- `ClonePlan` 在物理计划不可克隆时使用 `expect("memo implementation child plan must be cloneable")` panic。因此可克隆性是 Memo Implementation 挂接孩子的全局不变量。
- HashJoin/MergeJoin 自身成本只由左右行数和底层适配器决定；传入的 `_out_count` 被明确忽略。若未来成本模型需要输出行数，必须同步修改宏生成方法、Go 对照判断及测试。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、事务或外部 I/O。`BaseImpl::Cell<f64>` 提供的是单线程内部可变性，不是线程同步原语；`ImplementationRef` 的使用模式从调用处的 `Rc<RefCell<_>>` 可见，同样限定为单线程优化器对象图，不能据此宣称 `Send`/`Sync`。

构造函数取得 `Box<dyn BinaryJoinCostPlan>` 的所有权，包装对象与其中的物理计划共同存活。求成本阶段只临时借用孩子，不保存孩子引用；候选胜出后，`AttachChildren` 克隆孩子物理计划，父计划拥有这些克隆。最终优化结果再次克隆胜出计划，因此 Memo 候选与返回给后续阶段的物理树不共享同一个可变计划所有权。HashJoin 的运行时 worker 并发度在上游构造 `PhysicalHashJoin` 时设定，只影响底层 `GetCost`，不由本文件调度。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/implementation/join.go`：

- Go 分别手写 `HashJoinImpl` 和 `MergeJoinImpl`；Rust 用 `binary_join_implementation!` 合并相同结构与控制流。
- 两版均按“自身 Join 代价 + 左子代价 + 右子代价”计算并缓存总代价，也都忽略 `CalcCost` 的输出行数参数。
- Go 构造函数接受具体的 `*physicalop.PhysicalHashJoin`/`*PhysicalMergeJoin`，并依赖运行时类型断言；Rust 构造函数接受 `Box<dyn BinaryJoinCostPlan>`，由 `PlanAdapter<T>` 在上游做静态类型适配。
- Go 在 `CalcCost` 调用底层 `GetCost` 前先把两个孩子直接 `SetChildren` 到 Join 上，之后 `AttachChildren` 又设置一次。Rust 的当前底层 `GetCost(left, right, ...)` 只使用显式行数（HashJoin 还读取自身并发度），所以 `calc_cost` 不提前挂接孩子，而只在候选胜出后的 `AttachChildren` 中克隆并挂接。这是实现时序差异；若底层成本函数将来读取 `children`，必须重新评估并恢复计算前挂接。
- Go 的 `AttachChildren` 复用孩子计划指针；Rust 通过 `CloneChildren` 克隆计划以满足 Rust 所有权和 trait object 生命周期要求，最终树的逻辑顺序保持一致。
- Go 两种 Join 调用不同的 `GetCost` 参数签名；Rust 将差异封装在 `implementation_rules.rs` 的两个 `BinaryJoinCostPlan for PlanAdapter<...>` 实现中。

当前 Rust 独立测试没有直接构造 `HashJoinImpl` 或 `MergeJoinImpl` 来断言成本与挂接行为。`pkg/planner/cascades/old/implementation_rules_test.rs::build_right_rejects_full_outer_join` 仅验证 FullOuterJoin 在 HashJoin build-right 规则中不产生候选；它是上游规则边界证据，不是本文件宏行为的直接覆盖。

## 扩展指南

- 新增另一种遵循相同二元成本模型的 Join：为物理计划适配 `PlanAccess + BinaryJoinCostPlan`，再调用 `binary_join_implementation!(TypeName, ConstructorName)`；同时在实现规则中构造并包装该候选。若公式或孩子数不同，不应勉强复用此宏。
- 修改 Join 成本公式：优先修改具体 `BinaryJoinCostPlan::SelfCost` 适配器或底层物理 Join 的 `GetCost`；只有 Hash/Merge 共有的汇总规则变化时才修改 `calc_cost`。需要检查代价剪枝是否仍能用 `BaseImpl::GetCostLimit` 正确估算剩余预算。
- 让成本依赖输出行数：把当前 `_out_count` 改为实际使用，并为 HashJoin、MergeJoin 分别证明其语义；同步 Go 行为或明确记录有意差异。
- 修改孩子挂接策略：保持左/右顺序、计划可克隆性以及“只有胜出候选进入最终物理树”的优化器流程。若在 `calc_cost` 前挂接，应验证多候选共享/克隆不会污染 Memo 状态。
- 补测试时遵守仓库要求，不把测试内嵌到 `join.rs`。最合适的位置是新增同目录独立 `join_test.rs` 并由 `lib.rs` 的 `#[cfg(test)]` 装入，或扩展现有独立测试装配；至少覆盖两种 Join 的自身代价转发、子树成本相加、缓存写回、左右行数顺序、两个孩子挂接、成本上限扣减，以及少于两个孩子这一结构前置条件。
- 上游规则变化应同步 `pkg/planner/cascades/old/implementation_rules_test.rs`，尤其是 JoinType、build/inner 侧、排序属性和无连接键时不生成 MergeJoin 的边界。性能风险主要来自成本公式失真导致错误选型，以及不必要的计划克隆；兼容风险主要来自偏离 Go 的候选集合或左右侧语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录清单确认 `join.rs`、`base.rs`、`lib.rs`、Go 对照和独立测试文件均在索引中。
- RustCodeGraph `node --file pkg/planner/implementation/join.rs`：核对 73 行完整源码、宏定义、两次展开调用及导入。
- RustCodeGraph `explore "NewHashJoinImpl NewMergeJoinImpl HashJoinImpl MergeJoinImpl"`：确认 `NewHashJoinImpl` 的调用者包含 Go/Rust `getImplForHashJoin`，`NewMergeJoinImpl` 的 Go 调用入口为 `ImplMergeJoin::OnImplement`，并读取 Go 对照全貌。宏生成的 Rust 符号未被 `query` 单独展开，因此调用位置另以索引文件源码和精确 `rg` 复核。
- RustCodeGraph `node` 读取 `pkg/planner/implementation/base.rs`、`lib.rs`、`pkg/planner/cascades/old/implementation_rules.rs`（尤其 996–1199 行）、`optimize.rs`（280–389 行）及 `implementation_rules_test.rs`：核对 trait 桥接、孩子克隆、候选构造、剪枝/计价/挂接顺序和现有测试范围。
- `pkg/planner/implementation/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go package 移植元数据及 planner core/memo/physicalop 依赖边界；该 manifest 没有 feature 声明。
- `pkg/planner/implementation/join.go`：核对两个 Go Implementation 的公式、构造器和挂接时序。
- `pkg/planner/core/operator/physicalop/physical_hash_join.rs::GetCost` 与 `physical_merge_join.rs::GetCost`：核对当前 Rust 自身成本公式确实只使用显式输入行数（HashJoin 另用并发度），不读取已挂接孩子。
- `rg` 对 `pkg/planner/**/*_test.rs`/`*_test.go` 的符号检索只发现 `implementation_rules_test.rs` 对 `ImplHashJoinBuildRight` 的覆盖，未发现直接引用 `NewHashJoinImpl`、`NewMergeJoinImpl`、`HashJoinImpl` 或 `MergeJoinImpl` 的独立测试。

本任务只生成文档，未运行 Cargo。最终结构验证要求目标文件存在，且上述十一个固定二级标题各出现一次。
