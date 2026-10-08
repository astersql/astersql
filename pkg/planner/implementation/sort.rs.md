# `pkg/planner/implementation/sort.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-implementation`，由同目录 `lib.rs` 的私有模块 `mod sort` 纳入，并通过 `pub use sort::*` 将公开符号重新导出。这个包位于旧版 Cascades 优化器与 Memo 物理实现层之间：上游实现规则创建具体物理计划，下游 Memo 通过 `astersql_planner_memo::Implementation` 的统一接口计算代价、挂接孩子并取得最终物理计划。

文件提供两种一元排序 Implementation：`SortImpl` 包装需要真实执行排序的计划，`NominalSortImpl` 表示孩子已经满足顺序属性、无需保留真实排序节点的候选。生产入口可见于 `pkg/planner/cascades/old/implementation_rules.rs` 的 `ImplSort::OnImplement`；真实排序还会由 `pkg/planner/cascades/old/enforcer_rules.rs` 的 `OrderEnforcer::OnEnforce` 强制插入。

直接 crate 边界来自 `pkg/planner/implementation/Cargo.toml`：本文件使用 `astersql-planner-core-base` 的 `PhysicalPlan`、`astersql-planner-memo` 的 `ImplementationRef`，并使用当前 crate 在 `base.rs` 定义的公共基座、访问 trait 与辅助函数。该 manifest 没有为本模块声明条件 feature；本文件本身也没有条件编译项。

## 核心职责

- `SortImpl` 计算“排序自身代价 + 孩子累计代价”，排序行数是孩子统计行数和物理计划期望行数的较小值；结果写入 `BaseImpl` 的缓存。
- `SortImpl` 挂接时克隆 Memo 孩子的物理计划，再把克隆交给 `SortCostPlan::InjectProjectionBelowSort`，缓存适配器返回的最终计划树。
- `NominalSortImpl` 在候选尚未挂接时仍能暴露原始 `NominalSort` 计划；挂接后改为暴露孩子计划的克隆，并把自身缓存代价设为孩子代价，因此在最终计划树中消除名义排序层。
- 两种结构都通过 `impl_implementation!` 接入 Memo 的 `Implementation` trait；本文件只实现四个内部钩子，公共 `CalcCost`、`GetPlan`、`AttachChildren`、`GetCostLimit` 等方法由 `base.rs` 的宏生成。

这里不负责决定应选真实排序还是名义排序，也不直接实现排序算法。前者由 `ImplSort` 根据 `GetPropByOrderByItems` 的结果决定，后者由 `PhysicalSort` 及执行器负责。本文件承担的是候选包装、代价桥接和孩子计划树组装。

## 主要符号

- `pub struct SortImpl`：真实排序候选。`base: BaseImpl` 保存已计算代价；`plan_node: Box<dyn SortCostPlan>` 保存可访问物理计划、读取期望行数、计算自身代价并执行挂接改写的适配器；`attached_plan: Option<Box<dyn PhysicalPlan>>` 保存挂接孩子后的最终树。只有 `base` 为 crate 内可见，其余字段私有。
- `pub fn NewSortImpl(plan: Box<dyn SortCostPlan>) -> SortImpl`：构造真实排序候选，使用默认零代价基座并令 `attached_plan` 为 `None`。函数名保留 Go 风格，因此由 crate 级 `allow(non_snake_case)` 接受。
- `SortImpl::calc_cost`：读取 `children[0]` 的 `StatsInfo.RowCount`、schema 与缓存代价，读取 `SortCostPlan::ExpectedCount`，用 Go `math.Min` 兼容语义确定排序行数，调用 `SelfCost` 后加子代价并缓存。
- `SortImpl::plan`：挂接后返回 `attached_plan`，否则返回 `plan_node.Plan()`；因此 `GetPlan` 的身份会在 `AttachChildren` 前后变化。
- `SortImpl::attach_children`：克隆第一个孩子计划，将其交给 `InjectProjectionBelowSort`，并缓存返回的计划树。
- `pub struct NominalSortImpl`：名义排序候选。字段布局与 `SortImpl` 相似，但适配器只要求 `PlanAccess`，不需要排序代价接口。
- `pub fn NewNominalSortImpl(plan: Box<dyn PlanAccess>) -> NominalSortImpl`：构造尚未挂接的名义排序候选。
- `NominalSortImpl::calc_cost`：委托 `BaseImpl::CalcCost` 累加传入的全部孩子代价。
- `NominalSortImpl::attach_children`：只克隆 `children[0]` 作为最终计划，同时用 `ChildCost(children, 0)` 覆盖缓存代价。
- 两个 `cost_limit`：均转发 `BaseImpl::GetCostLimit`，语义是从父代价上限中减去已经提供的孩子代价。
- `impl_implementation!(SortImpl)` 与 `impl_implementation!(NominalSortImpl)`：生成 `astersql_planner_memo::Implementation` 实现；宏定义位于 `pkg/planner/implementation/base.rs`。

## 执行流程

真实排序候选有两条主要创建路径。

1. `ImplSort::OnImplement` 处理 `LogicalSort`。当排序项不能全部转成孩子物理顺序属性时，它创建 `PhysicalSort`，复制 `ByItems`，设置缩放后的统计、查询块偏移及一个 `ExpectedCnt = f64::MAX` 的孩子属性，然后以 `PlanAdapter<PhysicalSort>` 调用 `NewSortImpl`。
2. `OrderEnforcer::OnEnforce` 在 TiDB 引擎需要有序输出、但孩子不能自然满足时创建 `PhysicalSort`；排序项来自父 `PhysicalProperty.SortItems`，随后调用 `NewSortImpl` 并立即 `AttachChildren`。
3. Memo 调用 `CalcCost` 时，`SortImpl::calc_cost` 先借用第一个孩子，计算 `count = min(child.RowCount, ExpectedCount)`。为保持 Go `math.Min` 行为，只要任一输入为 NaN 就显式令结果为 NaN；否则才调用 Rust 的 `min`。
4. 适配器的 `SelfCost(count, child.schema())` 进入 `PhysicalSort::GetCost`。该物理算子按 `n log n` 估算 CPU，并计入内存；满足 OOM 临时存储条件时还计入 spill 磁盘代价。本文件再加上孩子已缓存代价，并将总值写入 `BaseImpl`。
5. Memo 挂接孩子时，`SortImpl` 通过 `ClonePlan` 克隆孩子计划，再调用适配器的 `InjectProjectionBelowSort`。此后 `GetPlan` 返回适配器产生的树，不再返回构造时的 `plan_node.Plan()`。

名义排序路径如下。

1. `ImplSort::OnImplement` 在 `GetPropByOrderByItems` 能把全部排序项表达为列顺序属性时，把父 `ExpectedCnt` 传给孩子属性，创建 `NominalSort`，再经 `PlanAdapter<NominalSort>` 调用 `NewNominalSortImpl`。
2. 代价计算默认累加孩子代价；按当前一元调用约定即等于唯一孩子代价。
3. 挂接时克隆第一个孩子作为 `attached_plan`，并把缓存代价明确设置成第一个孩子代价。之后 `GetPlan` 直接返回该孩子克隆，名义节点不进入最终物理计划树。

## 数据与状态

两个 Implementation 的状态都分为“候选计划适配器”“已挂接计划”和“缓存代价”三部分。

- `plan_node` 是拥有所有权的 trait object。`SortImpl` 使用较强的 `SortCostPlan: PlanAccess` 契约；`NominalSortImpl` 只要求 `PlanAccess`。
- `attached_plan` 构造时为空，`AttachChildren` 后变为 `Some`。重复挂接会用新克隆替换旧值，不在本文件累积孩子。
- `BaseImpl` 内部用 `Cell<f64>` 保存代价，所以 `calc_cost(&self, ...)` 虽只拿共享引用仍能更新缓存。
- `ImplementationRef` 是 `Rc<RefCell<dyn Implementation>>`。本文件短暂借用孩子，读取其计划、统计和代价；产出的物理孩子通过 `ClonePlan` 拥有独立所有权，不保存对 Memo `RefCell` 的借用。
- `SortImpl::calc_cost` 的 `_out_count` 未使用；排序规模取自孩子统计和 `ExpectedCount`。`NominalSortImpl::calc_cost` 把 `out_count` 传给基座，但基座当前同样不使用它。

关键不变量是两种排序都按一元算子使用。源码直接索引 `children[0]`，而不是接受空列表或选择性孩子；`NominalSortImpl::calc_cost` 虽会累加所有传入孩子，但挂接只保留第一个孩子，因此调用者必须保持恰好一个孩子。

## 依赖与调用关系

上游调用关系：

- `pkg/planner/cascades/old/implementation_rules.rs::ImplSort::OnImplement` 是两种构造函数的直接生产调用者。它在默认实现表中登记于 `Operand::Sort`。
- `pkg/planner/cascades/old/enforcer_rules.rs::OrderEnforcer::OnEnforce` 直接调用 `NewSortImpl`，用于强制满足非空排序属性，并立即挂接一个孩子。
- Memo 优化流程通过 `Implementation` trait 调用宏生成的 `CalcCost`、`GetPlan`、`AttachChildren` 与 `GetCostLimit`；宏再分派到本文件的同名内部钩子。

下游依赖关系：

- `PhysicalPlan::stats_info` 与 `schema` 提供排序计数和代价所需 schema。
- `SortCostPlan::{ExpectedCount, SelfCost, InjectProjectionBelowSort}` 隔离具体 `PhysicalSort` 类型。`implementation_rules.rs` 的 `PlanAdapter<PhysicalSort>` 和 `enforcer_rules.rs` 的 `SortPlan` 是两个生产适配器。
- `BaseImpl::{CalcCost, SetCost, GetCostLimit}` 管理代价；`ChildCost` 读取指定孩子代价；`ClonePlan` 调用 `clone_physical` 并取得独立计划树。
- `PhysicalSort::GetCost` 是生产适配器 `SelfCost` 的最终计算实现；会读取会话 CPU、内存、磁盘因子和内存配额。

RustCodeGraph 的文件节点把 `sort.rs` 的直接使用文件列为 `pkg/planner/cascades/old/implementation_rules.rs` 和 `pkg/planner/implementation/base_test.rs`。文本搜索补充确认了 `enforcer_rules.rs` 对重新导出的 `NewSortImpl` 的调用；当前索引的文件级反向边没有列出该路径，因此这里不把索引列表视作穷尽调用者集合。

## 错误处理与边界

本文件 API 不返回 `Result`，失败边界主要表现为 panic 或 IEEE-754 特殊值。

- 两个实现都直接访问 `children[0]`；空孩子会因越界而 panic。该约束依赖上游把 Sort 始终建模为一元算子。
- `ClonePlan` 在 `clone_physical` 返回错误时以 `expect("memo implementation child plan must be cloneable")` panic。因此孩子必须支持在其现有计划上下文中克隆。
- 两个生产 `SortCostPlan` 适配器在克隆 Sort 失败时也使用 `expect`；错误不会从本文件向上传播。
- `RefCell` 的重叠可变借用会在运行时 panic。本文件仅做短时不可变借用并立即克隆/读取，但调用者仍须遵守 Memo 的借用纪律。
- `SortImpl::calc_cost` 显式传播孩子行数或期望行数中的 NaN，使最终 `SelfCost` 与总代价保持 NaN；`base_test.rs::sort_count_min_propagates_nan_like_go` 固定了这一兼容行为。
- 正常有限值不在本文件检查负数、无穷大或代价溢出；这些值会传给 `PhysicalSort::GetCost` 或按浮点运算传播。
- `GetCostLimit` 可以返回负数，表示已有孩子代价已经超过上限；本文件不截断为零。

还有一个需要明确的当前迁移边界：Go `SortImpl.AttachChildren` 调用 `plannercore.InjectProjBelowSort(sort, sort.ByItems)`，用于排序键含标量函数时在 Sort 周围补 Projection。Rust trait 保留了 `InjectProjectionBelowSort` 这一语义名称，但目前 `implementation_rules.rs` 与 `enforcer_rules.rs` 的生产适配器都只是克隆 `PhysicalSort` 并挂接原孩子，没有调用 Rust 侧 `pkg/planner/core/rule_inject_extra_projection.rs::InjectProjBelowSort`。因此不能把“当前 Rust 已完成与 Go 等价的 Projection 注入”写成已支持事实。

## 并发与资源生命周期

该实现是优化期的同步、单线程所有权模型：`ImplementationRef` 使用 `Rc<RefCell<_>>`，`BaseImpl` 使用 `Cell<f64>`，都不提供跨线程共享语义。源码没有线程、异步任务、通道、锁、事务、I/O 或后台资源。

`New*` 构造函数取得 `Box<dyn ...>` 的所有权；`AttachChildren` 克隆孩子物理计划，并把新 `Box<dyn PhysicalPlan>` 放入 `attached_plan`。替换或销毁 Implementation 时，旧的适配器和已挂接计划按 Rust 所有权自动释放。Memo 孩子的 `Rc` 生命周期不被挂接计划延长，因为这里只保存克隆，不保存 `ImplementationRef`。

成本缓存具有内部可变性，但不是并发缓存：连续调用 `CalcCost` 或 `AttachChildren` 可以覆盖已有值；调用次序由优化器负责。尤其 `NominalSortImpl::AttachChildren` 会把此前可能由 `CalcCost` 累加得到的值改写为第一个孩子代价。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/implementation/sort.go`。

- Go `SortImpl`、`NewSortImpl`、`CalcCost` 与 Rust 同名符号一一对应。两边都以 `min(孩子 RowCount, 第一个孩子 Required Property 的 ExpectedCnt)` 作为排序行数，并加上孩子代价。
- Rust 通过 `SortCostPlan` 从具体物理计划抽取 `ExpectedCount`、`SelfCost` 和挂接改写，使 implementation crate 不必在结构字段中固定 `PhysicalSort`；Go 则在 `baseImpl.plan` 中保存并向下断言具体类型。
- Go 的 `math.Min` 遇到 NaN 会返回 NaN。Rust 没有直接使用 `f64::min`，而是显式检测 NaN，避免 Rust `min` 选择另一操作数；独立 Rust 回归测试覆盖了这点。
- Go `SortImpl.AttachChildren` 原地设置孩子并调用 `plannercore.InjectProjBelowSort`，返回自身；Rust 为避免把 Memo 孩子计划直接移入候选，会先克隆孩子，并把适配器返回的树缓存在 `attached_plan`。如上一节所述，当前生产适配器尚未实现 Go 的标量排序键 Projection 注入。
- Go `NominalSortImpl.AttachChildren` 直接返回 `children[0]`。Rust 的 `Implementation::AttachChildren` 签名只能返回 `&mut dyn Implementation`，所以改为缓存孩子物理计划的克隆，并让后续 `GetPlan` 返回该克隆；可观察的最终计划仍绕过名义排序节点。
- Go `NominalSortImpl` 未覆盖 `CalcCost`，继承 `baseImpl` 的孩子代价求和；Rust 明确转发 `BaseImpl::CalcCost`。Rust 挂接时额外把缓存成本设置为第一个孩子成本，以保持透传候选的代价。

相关 Go 规则 `pkg/planner/cascades/old/implementation_rules.go::ImplSort` 同样在排序项全为列时生成 `NominalSort`，否则生成 `PhysicalSort`。Rust `implementation_rules.rs::ImplSort` 保持这一分支意图。

## 扩展指南

- 修改真实排序代价的组合方式时，优先改 `SortImpl::calc_cost`；若变化属于物理 Sort 自身算法，应改 `PhysicalSort::GetCost` 或 `SortCostPlan::SelfCost` 的适配，而不是在两层重复计算。同步覆盖有限值、NaN、无穷大和期望行数裁剪。
- 补齐 Go 的标量排序键 Projection 注入时，应集中实现 `SortCostPlan::InjectProjectionBelowSort` 的生产适配器，并验证返回树的上/下 Projection、schema、排序表达式索引和孩子所有权。不能只修改 trait 名称或本文件的调用点。
- 改变名义排序消除方式时，要同时验证 `CalcCost`、`AttachChildren` 后的 `GetPlan`、计划类型和代价缓存，防止名义节点误进入执行树或丢失孩子代价。
- 若计划支持零孩子或多孩子，必须先调整本文件所有 `children[0]`、`ChildCost(..., 0)` 和上游 Sort 的一元契约；仅让某个入口容忍空切片会留下不一致状态。
- 新增状态字段时，要明确它在 `AttachChildren` 重入时是替换、累计还是禁止，并确认 trait object 与物理计划克隆的所有权。
- Rust 测试逻辑应继续放在独立测试文件，不嵌入 `sort.rs`。最接近的现有测试是 `pkg/planner/implementation/base_test.rs`；规则分支测试应放在 `pkg/planner/cascades/old/implementation_rules_test.rs`，强制排序路径测试应放在 `pkg/planner/cascades/old/enforcer_rules_test.rs`。Go 对照测试和行为也应同步核对。
- 兼容风险主要是 Go/Rust Projection 改写差异和 NaN 语义；正确性风险是孩子数量与挂接前后计划身份；性能风险集中在排序计数裁剪、`PhysicalSort::GetCost` 的 CPU/内存/spill 参数，以及不必要的计划克隆。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/planner/implementation/sort.rs` 的 `SortImpl`、`NewSortImpl`、`NominalSortImpl`、`NewNominalSortImpl` 及两个 `impl_implementation!` 调用。
- 公共契约：`pkg/planner/implementation/base.rs` 的 `BaseImpl`、`PlanAccess`、`SortCostPlan`、`ClonePlan`、`ChildCost` 与 `impl_implementation!`。
- crate 装配：`pkg/planner/implementation/lib.rs` 和 `pkg/planner/implementation/Cargo.toml`。
- 生产入口：`pkg/planner/cascades/old/implementation_rules.rs` 的 `PlanAdapter<PhysicalSort>`、`defaultImplementationMap`、`ImplSort::OnImplement`；`pkg/planner/cascades/old/enforcer_rules.rs` 的 `SortPlan` 与 `OrderEnforcer::OnEnforce`。
- 物理行为：`pkg/planner/core/operator/physicalop/physical_sort.rs::PhysicalSort::GetCost` 与 `pkg/planner/core/operator/physicalop/nominal_sort.rs::NominalSort::Attach2Task`。
- Go 对照：`pkg/planner/implementation/sort.go` 以及 `pkg/planner/cascades/old/implementation_rules.go::ImplSort`。
- 独立测试：`pkg/planner/implementation/base_test.rs::sort_count_min_propagates_nan_like_go`；`pkg/planner/cascades/old/enforcer_rules_test.rs` 只覆盖 enforcer 选择和新孩子属性清空，不覆盖 `SortImpl` 挂接；`implementation_rules_test.rs` 中未检索到 `ImplSort`/`NominalSort` 专项测试。
- RustCodeGraph：`status` 显示索引包含目标文件；`node --file pkg/planner/implementation/sort.rs` 给出完整源码并报告直接使用文件；`query` 同时定位 Rust/Go 的 `SortImpl`、`NewSortImpl`、`NominalSortImpl`、`NewNominalSortImpl`；对限定 Rust 符号执行的 `callers/callees` 查询在本次限定时间内未返回结果，因此调用边又用上述索引文件节点与精确文本搜索交叉核验。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构验收应确认本文恰好包含计划要求的十一个固定二级标题；人工复核重点是：真实/名义排序的分流、挂接前后 `GetPlan` 的变化、一元孩子不变量、NaN 兼容行为，以及 Projection 注入尚未与 Go 对齐的事实均有源码依据。
