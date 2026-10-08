# `pkg/planner/memo/implementation.rs`

## 文件定位

[`implementation.rs`](implementation.rs) 位于 `astersql-planner-memo` crate，是旧版 Cascades 优化器中“物理实现候选”的抽象边界。`pkg/planner/memo/lib.rs` 将本模块声明为私有模块后通过 `pub use implementation::*` 公开其 API，因此其他 crate 使用的是 `astersql_planner_memo::{Implementation, ImplementationRef}`。

这个文件不实现任何具体物理算子，也不执行搜索；它只定义对象安全的 `Implementation` trait 和统一引用类型 `ImplementationRef`。逻辑等价表达式由 `Group` 保存，而满足某个 `PhysicalProperty` 的最佳物理候选由 `Group::ImplMap` 以 `ImplementationRef` 缓存（`pkg/planner/memo/group.rs` 的 `Group::GetImpl`、`Group::InsertImpl`）。实际代价公式和物理计划持有者位于 `pkg/planner/implementation/*.rs`，搜索编排位于 `pkg/planner/cascades/old/optimize.rs::implGroup`。

`pkg/planner/memo/Cargo.toml` 将该目录定义为包 `astersql-planner-memo`，库入口为 `lib.rs`。本文件自身唯一的工作区依赖是 `astersql-planner-core-base::PhysicalPlan`；`Rc`、`RefCell` 来自标准库。

## 核心职责

本文件把 Cascades 物理搜索需要的六项能力收束为一个动态分发接口：

1. `CalcCost` 根据输出行数和已经选出的子实现计算候选总代价。
2. `SetCost`、`GetCost` 写入和读取候选的缓存代价。
3. `GetPlan` 暴露候选承载的 `PhysicalPlan`，供读取子属性、统计信息及最终克隆计划树。
4. `AttachChildren` 把选中的子实现对应的物理计划挂到当前候选上。
5. `GetCostLimit` 在实现下一个子 Group 前计算剩余预算，以便提前剪枝。
6. `ImplementationRef` 为异构实现提供共享所有权和运行时可变借用，使候选能够进入 Group 缓存、规则返回值和递归搜索过程。

这些方法只规定协议，不规定成本公式、子节点数量、缓存更新时机或错误类型。具体语义由各实现负责；例如 `pkg/planner/implementation/base.rs::BaseImpl` 默认把子代价求和，而 reader、join、sort 和 simple-plan 实现会叠加各自的算子代价。

## 主要符号

- `pub type ImplementationRef = Rc<RefCell<dyn Implementation>>`：单线程共享的 trait object。`Rc` 允许规则、Group 缓存和搜索过程持有同一候选；`RefCell` 允许在运行时执行 `SetCost` 与 `AttachChildren` 所需的可变借用。
- `pub trait Implementation`：物理候选的对象安全接口，没有关联类型、泛型方法、默认实现或 supertrait。
- `CalcCost(&self, out_count: f64, children: &[ImplementationRef]) -> f64`：计算候选代价。接口使用不可变 `self`，具体实现可以像 `BaseImpl` 一样通过 `Cell<f64>` 缓存结果；不能据此假设所有实现都会回写缓存。
- `SetCost(&mut self, cost: f64)` / `GetCost(&self) -> f64`：显式修改或读取已缓存的代价。搜索失败时，`implGroup` 用 `SetCost(f64::MAX)` 标记候选不可用。
- `GetPlan(&self) -> &dyn PhysicalPlan`：借用底层物理计划。搜索过程用它取得子节点所需属性，最终入口用它克隆胜出计划。
- `AttachChildren(&mut self, children: &[ImplementationRef]) -> &mut dyn Implementation`：挂接胜出子树并返回同一个实现对象，支持链式使用。`pkg/planner/implementation/base.rs::impl_implementation!` 的适配实现先调用具体类型的 `attach_children`，再返回 `self`。
- `GetCostLimit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64`：给尚未实现的下一个子 Group 计算预算。默认基座的公式是总预算减去已选子实现代价之和，但具体实现可以缩放或重写。

文件没有模块级常量、结构体、自由函数、`impl` 块或条件编译项；两个公开符号均经 crate 根再导出。

## 执行流程

主流程可由 `pkg/planner/cascades/old/optimize.rs::implGroup` 验证：

1. 优化器以 `(Group, required PhysicalProperty, cost_limit)` 进入递归搜索，先调用 `Group::GetImpl` 查找该属性哈希对应的缓存实现；命中且 `GetCost() <= cost_limit` 时直接复用。
2. 对 Group 中每个逻辑等价表达式，`implGroupExpr` 运行实现规则。规则在 `pkg/planner/cascades/old/implementation_rules.rs` 中把具体实现包装为 `Rc<RefCell<_>>`，形成 `ImplementationRef` 候选。
3. 对候选的每个子 Group，优化器通过 `GetPlan().get_child_req_props(index)` 取得子物理属性，再把当前已经选出的子实现传给 `GetCostLimit`，得到下一次递归调用的预算。
4. 子 Group 无可行实现时，候选被 `SetCost(f64::MAX)` 标记并跳过；否则把子实现依次加入 `child_implementations`。
5. 子节点齐备后，优化器调用 `CalcCost(out_count, &child_implementations)`。超过当前上限的候选被剪枝；比当前最佳候选便宜的候选才调用 `AttachChildren` 组装物理树，并收紧后续 `cost_limit`。
6. 排序等物理属性无法自然满足时，`pkg/planner/cascades/old/enforcer_rules.rs::OrderEnforcer::OnEnforce` 构造 `SortImpl`，调用 `AttachChildren` 包住原实现，再由搜索过程 `SetCost` 写入“强制算子代价 + 子代价”。
7. 最佳候选由 `Group::InsertImpl` 按完整物理属性哈希缓存。顶层 `findBestPlan` 通过 `GetCost` 返回代价，并通过 `GetPlan().clone_physical(...)` 产出独立的最终物理计划。

因此 `AttachChildren` 只发生在候选已经胜过当前最优值之后，而成本预算可在每个子 Group 递归前逐步扣减；两者共同避免为明显劣势候选构造完整物理树。

## 数据与状态

`Implementation` 的可观察状态由实现对象持有，接口只约定访问方式。当前通用实现 `BaseImpl` 使用 `Cell<f64>` 保存 cost：默认值为 `0.0`，`CalcCost` 把全部子实现的 `GetCost` 求和并缓存，`SetCost` 覆盖缓存，`GetCost` 返回缓存。某些具体实现有不同细节，例如零成本算子的 `CalcCost` 返回 `0.0` 但不清空此前显式设置的缓存；该行为由 `pkg/planner/implementation/base_test.rs::zero_cost_calc_does_not_reset_cached_cost` 固定。

`children: &[ImplementationRef]` 是借用切片，本接口不取得切片所有权。具体挂接辅助函数 `pkg/planner/implementation/base.rs::AttachChildren` 会读取每个子实现的 `GetPlan`，通过 `ClonePlan` 克隆物理计划，然后把克隆结果写入父物理计划；因此常规实现不会把 `ImplementationRef` 本身嵌入最终物理树。

Group 侧的 `ImplMap: HashMap<Vec<u8>, ImplementationRef>` 以 `PhysicalProperty::HashCode()` 的字节向量为键。`pkg/planner/memo/memo_aster_unit_test.rs::implementation_cache_keys_by_full_physical_property_hash` 验证了相同属性返回同一 `Rc`，而仅 `ExpectedCnt` 不同也不会误命中。

代价使用 `f64`，协议没有禁止负数、`NaN` 或无穷值。当前搜索明确把 `f64::MAX` 当作不可用哨兵，并用普通浮点比较筛选候选；具体实现和调用方必须维护所需的数值不变量。测试还确认 reader worker 数为零时按 Go 浮点语义产生正无穷，以及 sort 的 `NaN` 会继续传播，而不是由本 trait 纠正。

## 依赖与调用关系

上游主要调用者如下：

- `pkg/planner/cascades/old/optimize.rs::implGroup`：消费全部六个 trait 方法，负责递归枚举、代价剪枝、组树与缓存。
- `pkg/planner/cascades/old/implementation_rules.rs::implementation_ref`：把各规则产生的具体实现擦除为 `ImplementationRef`。
- `pkg/planner/cascades/old/enforcer_rules.rs::OrderEnforcer::OnEnforce`：读取子计划信息、创建强制排序实现并挂接孩子。
- `pkg/planner/memo/group.rs::{GetImpl, InsertImpl}`：按物理属性缓存和返回共享实现。
- `pkg/planner/implementation/base.rs::impl_implementation!`：为 datasource、join、simple plans、sort 等具体类型生成 trait 适配代码。

下游依赖只有 `astersql_planner_core_base::PhysicalPlan`：`GetPlan` 将具体物理计划统一暴露为 trait object。实际方法执行随后会进入 `pkg/planner/implementation/base.rs`、`datasource.rs`、`join.rs`、`simple_plans.rs` 或 `sort.rs` 中的具体公式和计划挂接逻辑。

crate 边界上，`astersql-planner-memo` 依赖 `astersql-planner-core-base`；反向消费方 `astersql-planner-implementation` 和 `pkg/planner/cascades/old` 各自在其 `Cargo.toml` 中依赖 `astersql-planner-memo`。这避免 memo 抽象直接依赖具体实现 crate，从而没有形成 Rust crate 循环依赖。

## 错误处理与边界

trait 本身所有方法都返回普通值或引用，没有 `Result`/`Option`，所以本层不表达可恢复错误。失败和边界由调用者及具体实现处理：

- 找不到满足条件的子实现时，`implGroup` 用 `f64::MAX` 标记候选并停止继续组树；找不到顶层计划最终转换为 planner error。
- `Rc<RefCell<_>>` 的借用规则在运行时检查；重叠的可变借用或可变/不可变借用会 panic。现有调用链会在递归或重新借用前结束局部 borrow，例如 `OrderEnforcer::OnEnforce` 显式 `drop(child_plan)` 后再移动 child。
- `GetPlan` 返回借用引用，生命周期受实现对象的 `RefCell` borrow guard 约束，不能在 guard 释放后继续使用。
- `children` 的长度与顺序没有由 trait 校验。具体算子直接访问 `children[index]` 时，数量不足会越界 panic；调用者必须与物理计划的 child required properties 保持一致。
- 默认 `GetCostLimit` 可以返回负数；接口不截断为零。递归搜索以得到的值作为上限，使不可行分支自然不能通过成本比较。
- `AttachChildren` 的常规辅助路径要求物理计划可克隆；`ClonePlan` 在 clone 失败时使用 `expect("memo implementation child plan must be cloneable")` panic。这是实现层不变量，不是本 trait 提供的错误通道。
- 测试桩可以合法地让 `GetPlan` panic，只要测试路径不访问计划；这表明接口无法在类型层强制“每个实现的每个方法都可用”，生产实现必须自行满足完整协议。

## 并发与资源生命周期

`ImplementationRef` 使用 `Rc<RefCell<_>>`，明确限定为单线程所有权模型：它既不是 `Arc`，trait 也没有 `Send + Sync` 约束。因此候选、Group 缓存和递归搜索应在同一线程内使用，不能直接跨线程发送或共享。

生命周期从实现规则创建候选开始。候选在当前搜索过程中由局部变量持有；胜出后同一个 `Rc` 被写入 `Group::ImplMap`，缓存与调用者可共同持有。`Rc` 最后一个强引用释放时，实现对象及其物理计划状态被销毁。本文件没有线程、异步任务、锁、通道、文件句柄或显式清理逻辑。

`RefCell` 只提供动态借用检查，不提供线程同步。修改 cost 或挂接 children 必须经 `borrow_mut()`；只读代价和计划访问经 `borrow()`。物理子计划通常是克隆后挂接，因此父实现的计划树与子实现的内部计划在对象生命周期上解耦，不依赖子 `ImplementationRef` 长期存活。

## 与 Go 版本的对应关系

直接原型是 `pkg/planner/memo/implementation.go::Implementation`。Rust 保留了相同的六个方法名和总体语义：计算/读写代价、取得计划、挂接孩子、推导下一子 Group 的代价上限。参数中的 Go variadic `children ...Implementation` 对应 Rust 借用切片 `&[ImplementationRef]`。

主要表示差异如下：

- Go interface 值本身具有共享引用语义；Rust 显式使用 `Rc<RefCell<dyn Implementation>>` 完成共享、动态分发和内部可变性。
- Go `GetPlan() PhysicalPlan` 返回 interface 值；Rust 返回 `&dyn PhysicalPlan` 借用，避免转移或克隆计划所有权。
- Go `AttachChildren(...) Implementation` 返回 interface；Rust 返回 `&mut dyn Implementation`，表示仍是同一个借用中的对象，不能借此创建新的共享所有权。
- Go `SetCost` 的接收者由具体类型决定；Rust trait 明确要求 `&mut self`。当前 `BaseImpl` 内部虽用 `Cell`，宏仍通过可变 trait 方法对外暴露写操作。
- Rust 的引用模型是单线程 `Rc<RefCell>`；Go interface 没有对应的编译期线程限制。移植新路径时不能假设 Rust 候选可跨线程使用。

行为一致性的直接测试包括：`pkg/planner/implementation/base_test.rs::TestBaseImplementation` 对应 Go `base_test.go::TestBaseImplementation`；`base_implementation_children_follow_go` 验证子代价求和、剩余上限和计划挂接；`pkg/planner/memo/memo_aster_unit_test.rs::implementation_cache_keys_by_full_physical_property_hash` 与 Go `group_test.go::TestGetInsertGroupImpl` 共同验证按物理属性缓存实现。Rust memo 目录没有同名 `implementation_test.rs`，相关测试按职责放在独立的 `memo_aster_unit_test.rs` 和 implementation crate 的 `base_test.rs`，符合源文件与测试分离要求。

## 扩展指南

新增具体物理实现时，优先在 `pkg/planner/implementation/` 的对应独立源文件中复用 `BaseImpl` 和 `impl_implementation!`，并完成以下约束：

1. `GetPlan` 始终返回与候选一致的物理算子，且该计划支持现有挂接路径所需的克隆。
2. 明确 `CalcCost` 是否包含全部子树代价、是否回写缓存，并使 `GetCostLimit` 与同一公式一致；否则分支剪枝可能错误排除最优计划。
3. 让 child required properties、`children` 的个数和顺序完全一致，并在自定义 `AttachChildren` 中保持该顺序。
4. 在实现规则中将新类型包装为 `ImplementationRef`，并在 `implGroupExpr` 可达的规则表中接线；不要让 memo crate 反向依赖具体实现 crate。
5. 若需要跨线程优化，不能只把 `Rc` 改成 `Arc`：还必须审计 `RefCell`、trait 的 `Send + Sync`、具体 `PhysicalPlan` 和全部实现状态。这属于接口级兼容变更。
6. 如果为 trait 增加必需方法，所有具体类型、`impl_implementation!` 宏和测试桩都必须同步修改；优先考虑有可靠默认语义的默认方法，以减小破坏面。

测试应继续放在独立文件。通用协议或成本基座行为应扩展 `pkg/planner/implementation/base_test.rs`；Group 属性缓存行为应扩展 `pkg/planner/memo/memo_aster_unit_test.rs`；具体算子的公式应在 implementation crate 的相应独立测试文件中覆盖。至少验证正常成本、超限剪枝、子节点数量/顺序、特殊浮点值和计划挂接后的树形结构。兼容风险集中在 Go 浮点语义、缓存写入时机和 `AttachChildren` 返回同一对象；性能风险集中在重复克隆物理子树与 `RefCell` 动态借用。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/memo/implementation.rs`，确认仅含 `ImplementationRef` 与 `Implementation` 六方法协议。
- crate/模块边界：`pkg/planner/memo/Cargo.toml`、`pkg/planner/memo/lib.rs`；具体消费 crate 依赖见 `pkg/planner/implementation/Cargo.toml` 与 `pkg/planner/cascades/old/Cargo.toml`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/memo` 定位 Rust/Go 对照及独立测试；`explore "pkg/planner/memo/implementation.rs Implementation ImplementationRef CalcCost AttachChildren GetCostLimit"` 找到 `BaseImpl`、实现规则、Group 缓存和测试调用；`node` 核对了目标文件、`group.rs`、`old/optimize.rs`、`implementation/base.rs`、`enforcer_rules.rs`、`base_test.rs` 与 `memo_aster_unit_test.rs` 的相关源码。对 trait 本身执行 `callers`/`callees` 没有给出方法级边，故又以精确符号引用搜索补齐动态分发调用证据。
- Go 对照：`pkg/planner/memo/implementation.go::Implementation`；缓存测试为 `pkg/planner/memo/group_test.go::TestGetInsertGroupImpl`，基础成本测试为 `pkg/planner/implementation/base_test.go::TestBaseImplementation`。
- Rust 测试：`pkg/planner/memo/memo_aster_unit_test.rs::{TestImplementation, implementation_cache_keys_by_full_physical_property_hash}` 与 `pkg/planner/implementation/base_test.rs::{TestBaseImplementation, base_implementation_children_follow_go, zero_cost_calc_does_not_reset_cached_cost, reader_zero_workers_follow_go_float_semantics, readers_use_the_same_row_size_source_as_go, sort_count_min_propagates_nan_like_go}`。
- 应用主链：`pkg/planner/cascades/old/optimize.rs::{findBestPlan, implGroup, implGroupExpr}`，规则构造见 `implementation_rules.rs::implementation_ref`，强制排序见 `enforcer_rules.rs::OrderEnforcer::OnEnforce`。

本任务是纯文档分析，按计划不运行 Cargo 或代码测试。结构验证要求本文恰好包含约定的十一个二级章节；事实复核限于当前工作区源码与 RustCodeGraph 索引，没有验证运行时性能或执行完整 SQL 集成流程。
