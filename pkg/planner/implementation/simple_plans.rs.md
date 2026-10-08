# `pkg/planner/implementation/simple_plans.rs`

源码：[`simple_plans.rs`](./simple_plans.rs)

## 文件定位

本文件属于 `astersql-planner-implementation` crate（见 `pkg/planner/implementation/Cargo.toml`），由 `lib.rs` 的私有 `simple_plans` 模块加载并通过 `pub use simple_plans::*` 对外再导出。它位于旧 Cascades 优化器从逻辑表达式生成物理候选的下游：`pkg/planner/cascades/old/implementation_rules.rs` 构造具体 `Physical*` 节点，再调用本文件的 `New*Impl` 将物理节点包装成 `astersql_planner_memo::Implementation`，供 Memo 计算代价、剪枝和挂接子计划。

RustCodeGraph 的文件节点显示，本文件当前被 `pkg/planner/cascades/old/implementation_rules.rs` 使用。文件只负责 Show、Limit、Projection、Selection、HashAgg、TopN、UnionAll、Apply、MaxOneRow 和 Window 这些“简单计划”的 Implementation 包装与代价策略；逻辑到物理节点的属性匹配、统计缩放和节点字段填充不在这里，而在上述 implementation rules 中。

## 核心职责

1. 保存“物理计划节点 + 已缓存代价”：每个实现都含 `BaseImpl` 和一个装箱的计划访问 trait object；`BaseImpl` 内部以 `Cell<f64>` 保存代价。
2. 为各算子实现 Memo 的统一 `Implementation` 协议。`impl_implementation!`（定义于 `base.rs`）把本文件内部的 `calc_cost`、`plan`、`attach_children`、`cost_limit` 转接为公开 trait 方法。
3. 保持 Go `pkg/planner/implementation/simple_plans.go` 的代价模型：Projection、Selection、HashAgg、TopN、UnionAll、Apply 各有专用公式；Show/Limit 使用默认子代价求和；MaxOneRow/Window 透传唯一孩子代价。
4. 区分根侧与存储侧成本。Selection 用 `coprocessor` 选择 TiDB CPU factor 或 TiKV Cop CPU factor；HashAgg、TopN 用 `root` 把执行位置传给物理算子的成本函数。
5. 在父实现最终落树时，通过 `AttachChildren` 克隆各子 Implementation 的物理计划并设置为当前计划的 children。

## 主要符号

- `base_cost_implementation!`：生成 `ShowImpl`、`LimitImpl` 及其构造器。生成类型持有 `Box<dyn PlanAccess>`，成本直接委托 `BaseImpl::CalcCost`，即所有已给孩子成本之和。
- `ProjectionImpl` / `NewProjectionImpl`：持有 `Box<dyn ProjectionCostPlan>`；成本为 `SelfCost(ChildRows(children, 0)) + ChildCost(children, 0)`。
- `SelectionImpl`：Rust 将 Go 的 `TiDBSelectionImpl` 与 `TiKVSelectionImpl` 合并为一个结构，以私有布尔值 `coprocessor` 区分执行端；公开构造器 `NewTiDBSelectionImpl`、`NewTiKVSelectionImpl` 固定该标志，并提供两个同名语义的类型别名。
- `HashAggImpl`：以 `root` 区分 TiDB/TiKV；`NewTiDBHashAggImpl` 传 `true`，`NewTiKVHashAggImpl` 传 `false`。
- `TopNImpl`：同样以 `root` 区分两端，并由对应的两个公开构造器封装。
- `UnionAllImpl` / `NewUnionAllImpl`：按并行分支模型取最大孩子成本，而不是求和；其 `cost_limit` 原样返回父上限。
- `ApplyImpl` / `NewApplyImpl`：二元实现，把左右行数及左右成本交给 `ApplyCostPlan::SelfCost`；对右孩子计算专用成本上限。
- `passthrough_implementation!`：生成 `MaxOneRowImpl` 与 `WindowImpl`，成本等于第一个孩子成本。
- 本文件没有模块级常量、enum、条件编译项或独立 trait 定义。所有生成类型均通过 `impl_implementation!` 实现 `Implementation`；内部辅助构造器 `newSelectionImpl`、`newHashAggImpl`、`newTopNImpl` 不对 crate 外公开。

## 执行流程

1. `pkg/planner/cascades/old/implementation_rules.rs` 的 `ImplProjection`、`ImplShow`、`ImplSelection`、`ImplHashAgg`、`ImplLimit`、`ImplTopN`、`ImplTopNAsLimit`、`ImplUnionAll`、`ImplApply`、`ImplMaxOneRow`、`ImplWindow` 先检查所需物理属性并构造具体 `Physical*` 节点。
2. 规则把物理节点放进 `PlanAdapter<T>`。该适配器实现 `PlanAccess`，并针对具体节点实现 `ProjectionCostPlan`、`SelectionCostPlan`、`HashAggCostPlan`、`TopNCostPlan`、`UnionAllCostPlan` 或 `ApplyCostPlan`，从会话变量和物理算子提取成本参数。
3. 规则调用相应 `New*Impl`，再用 `implementation_ref` 转为 Memo 使用的 `Rc<RefCell<dyn Implementation>>`。Selection、HashAgg、TopN 根据 Group 的 `EngineType` 选择 TiDB 或 TiKV 构造器；不支持的引擎在规则层返回 `PlannerError`。
4. Memo 调用 `Implementation::CalcCost`。宏把调用转发到具体 `calc_cost`：
   - Show/Limit：所有已有孩子成本之和；
   - Projection：投影自身成本加第一个孩子成本；
   - Selection：第一个孩子行数乘执行端 CPU factor，再加孩子成本；
   - HashAgg/TopN：物理节点按输入行数和 `root` 计算自身成本，再加孩子成本；
   - UnionAll：`(1 + 分支数) * ConcurrencyFactor + max(分支成本)`；
   - Apply：由计划节点综合左右行数、左右成本计算；
   - MaxOneRow/Window：透传第一个孩子成本。
5. 除 Show/Limit 的默认路径外，算出的成本通过 `BaseImpl::SetCost` 写入缓存，之后由 `GetCost` 参与上层候选比较。
6. Memo 逐步实现孩子时调用 `GetCostLimit`。一般实现用父上限减去已完成孩子成本；UnionAll 不缩减；Apply 在已有左孩子时把剩余上限除以有效左行数，得到右侧可用上限。
7. 选中候选后，`AttachChildren` 克隆孩子的物理计划并写入当前物理节点，形成最终物理计划树。

## 数据与状态

- 每个实现拥有自己的 `plan_node: Box<dyn ...CostPlan>`；trait object 同时暴露底层 `PhysicalPlan` 和该算子成本计算所需的最小接口，避免本文件依赖具体 `Physical*` 类型。
- `BaseImpl` 的 `Cell<f64>` 是单线程内部可变缓存，使 `CalcCost(&self, ...)` 能更新成本。新对象的成本默认为 `0.0`。
- `children: &[ImplementationRef]` 中的 `ImplementationRef` 是 `Rc<RefCell<dyn Implementation>>`。`ChildRows` 从孩子计划的 `stats_info().RowCount` 读取基数，`ChildCost` 从孩子的成本缓存读取数值。
- `coprocessor` 和 `root` 是构造后不变的执行位置标志；对外构造器保证它们与 TiDB/TiKV 名称一致。
- `UnionAllImpl` 的最大孩子成本表达“各分支并行执行，整体等待最慢分支”的假设；额外的 `(1 + n)` 并发因子计入调度成本。
- `ApplyImpl::cost_limit` 在存在左过滤条件时，以 `SelectionFactor` 缩小左行数，保持 Go 版本对右侧重复执行次数的估计。

## 依赖与调用关系

上游调用者是 `pkg/planner/cascades/old/implementation_rules.rs`。其中 `PlanAdapter<PhysicalProjection>::SelfCost` 使用 CPU factor、投影并发度和并发因子；Selection 读取 CPU/CopCPU factor；HashAgg、TopN 和 Apply 委托各自物理节点的 `GetCost`；UnionAll 读取 `TiDBOptConcurrencyFactor`。因此本文件不直接解释会话变量，而是消费适配器提供的稳定成本接口。

本文件直接依赖：

- `astersql-planner-core-base::PhysicalPlan`：返回和挂接底层物理计划；
- `astersql-planner-core-cost::factors_thresholds::SelectionFactor`：Apply 左过滤条件的选择率；
- `astersql-planner-memo::ImplementationRef`：读取孩子成本和统计；
- 同 crate 的 `BaseImpl`、`ChildRows`、`ChildCost`、`AttachChildren`、各 `*CostPlan` trait 及 `impl_implementation!`。

crate 边界由 `pkg/planner/implementation/Cargo.toml` 声明：本文件使用的 core-base、core-cost、memo 均是工作区路径依赖；Cargo metadata 将对应 Go package 指向 `pkg/planner/implementation`。`lib.rs` 的公开再导出使旧 Cascades crate 可直接导入这些构造器。

## 错误处理与边界

- 本文件的构造器和成本方法不返回 `Result`；引擎不支持、缺少上下文或物理节点构造失败等可恢复错误由上游 implementation rules 处理。
- Projection、Selection、HashAgg、TopN、MaxOneRow、Window 直接访问 `children[0]`，Apply 在 `calc_cost` 中访问 `children[0]` 和 `children[1]`。调用者必须满足算子元数；孩子不足会因切片越界而 panic。只有 `ApplyImpl::cost_limit` 显式允许空 children，并原样返回上限。
- `ApplyImpl::cost_limit` 没有对 `left_count == 0`、负的剩余预算或非有限浮点数做特殊处理；它沿用 Go 浮点除法语义，可能得到正负无穷或 NaN，调用方必须按成本模型处理。
- `UnionAllImpl` 对空 children 的最大孩子成本为 `0.0`，自身成本仍为一个并发因子；这是公式的直接结果。
- `AttachChildren` 经 `ClonePlan` 调用 `clone_physical(...).expect(...)`；若某个物理计划不可克隆，会 panic。可克隆性是 Memo 组树的前置不变量。
- 本文件不验证统计是否存在或行数是否有效；`ChildRows` 直接读取计划统计。统计准备属于上游 Group/PhysicalPlan 初始化职责。

## 并发与资源生命周期

这里没有线程、异步任务、锁、通道、事务或 I/O。`Rc<RefCell<...>>` 与 `Cell<f64>` 明确限定为单线程 Memo 优化期共享与内部可变性，不能跨线程安全共享。

`Box<dyn ...CostPlan>` 独占适配后的物理节点；实现对象销毁时节点随之释放。挂接孩子时不是保存孩子的 `Rc`，而是克隆其 `PhysicalPlan` 后写入父节点，因此最终计划树与 Memo 中孩子 Implementation 的可变借用生命周期解耦。成本缓存从构造时的零值开始，每次 `CalcCost` 覆盖；调用者不应在尚未计算成本时把默认零值当成已验证成本。

UnionAll 的“并行”仅体现在成本公式取最大分支成本，本文件本身不启动并行执行；实际执行并发属于执行器层。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/implementation/simple_plans.go`，总体公式和上游构造器一一对应：

- Rust 的 Projection、Show、Limit、UnionAll、Apply、MaxOneRow、Window 对应 Go 同名结构和构造器。
- Go 为 TiDB/TiKV Selection、HashAgg、TopN 分别声明结构；Rust 合并成一个结构，通过布尔标志与公开类型别名保留调用端语义。该合并没有删减两端成本分支。
- Go Projection 调用 `utilfuncp.GetCost4PhysicalProjection`；Rust 把等价计算放入上游 `PlanAdapter<PhysicalProjection>::SelfCost`，本文件只组合自身成本与孩子成本。
- Go HashAgg/TopN/Apply 直接向具体物理节点调用 `GetCost`；Rust 通过相应 `*CostPlan` trait 转发，隔离具体类型但保留参数顺序和 root 标志。
- Go UnionAll 明确说明孩子并行，取最大孩子成本；Rust 的 `fold(0.0, f64::max)` 与同一公式一致。两版均不缩减 UnionAll 的成本上限。
- Go Apply 在左条件非空时乘 `cost.SelectionFactor`；Rust 由 `HasLeftConditions` 和 `SelectionFactor` 实现相同分支。两版在还没有孩子时均原样返回成本上限。
- Go `baseImpl.AttachChildren` 直接挂接孩子计划接口；Rust 为满足所有权模型先克隆孩子计划，再挂接。逻辑树结构一致，但 Rust 额外要求 `clone_physical` 成功。

当前 Rust 文件已经具有 AsterSQL 处理标记和保留的 PingCAP Apache License。没有发现本文件独有而 Go 版本缺失的运行时功能，也没有条件编译造成的版本分叉。

## 扩展指南

- 新增简单算子时，先判断能否复用 `base_cost_implementation!` 或 `passthrough_implementation!`；若有自身成本，新增最小 `*CostPlan` trait（通常位于 `base.rs`）及具体实现结构，并用 `impl_implementation!` 接入 Memo。
- 新增或调整成本公式时，应同步检查 `pkg/planner/cascades/old/implementation_rules.rs` 中的 `PlanAdapter`，确保会话变量、统计行数、root/coprocessor 标志和具体物理节点参数仍与 Go `simple_plans.go` 一致。
- 改动孩子元数或属性传递时，必须同步对应的 `Impl*::OnImplement`：尤其是 Apply 的两个孩子、MaxOneRow 的 `ExpectedCnt = 2`、TopN/Limit 的孩子期望行数以及 Window 的排序属性。
- 不要把单元测试嵌入本生产文件。应在同目录独立测试文件中扩展；现有 `base_test.rs` 直接覆盖 `LimitImpl` 的计划类型、默认成本和缓存读写，但 Projection、Selection、HashAgg、TopN、UnionAll、Apply、MaxOneRow、Window 尚无同目录直接成本公式测试。新增公式测试宜新建或扩展独立 `*_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] #[path = "..."]` 声明加载。
- 建议覆盖的边界包括：TiDB/TiKV 因子选择、UnionAll 空/多分支和最大成本、Apply 空孩子/左条件/零左行数、缺少必需孩子时的契约，以及 AttachChildren 的克隆行为。
- 兼容风险主要是与 Go 成本排序发生偏差；性能风险主要是错误的基数或并发因子改变候选选择，以及不必要的物理计划克隆。修改后应优先做精确代价单测和旧 Cascades 规则测试，而不是用“可编译”代替行为验证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/planner/implementation/simple_plans.rs` 与文件节点均确认目标已索引；`node --file ... --offset 1/261` 读取了 389 行全文件，并报告唯一使用文件为 `pkg/planner/cascades/old/implementation_rules.rs`。精确 `node --file ... NewApplyImpl` 核对了构造器定义；一次精确 `callers` 查询未返回结果，因此调用点另以精确源码引用搜索核验。
- 生产源码：完整读取 `pkg/planner/implementation/simple_plans.rs`；读取 `base.rs` 的 `BaseImpl`、各成本 trait、`ChildRows`/`ChildCost`、`AttachChildren` 和 `impl_implementation!`；读取 `lib.rs` 的模块与再导出；读取 `pkg/planner/cascades/old/implementation_rules.rs` 的 `PlanAdapter` 成本实现和所有相关 `Impl*` 规则。
- crate 配置：读取 `pkg/planner/implementation/Cargo.toml`，确认 crate 名、`lib.rs` 入口、Go package metadata、工作区依赖和测试依赖。
- Go 对照：完整读取 `pkg/planner/implementation/simple_plans.go`，并读取 `base.go` 核对默认成本、成本上限和孩子挂接语义；精确搜索确认 Go/Rust implementation rules 均调用对应构造器。
- 测试证据：读取 `pkg/planner/implementation/base_test.rs`（直接覆盖 `NewLimitImpl`、`CalcCost`、`SetCost`/`GetCost`、默认成本上限和挂接辅助）、`main_test.rs`；读取 `pkg/planner/cascades/old/implementation_rules_test.rs`，确认其当前只覆盖 HashJoin FullOuterJoin 拒绝分支，并未直接覆盖本文件各简单算子的代价公式。Go 的 `base_test.go` 提供默认实现对照；同目录不存在 `simple_plans_test.rs` 或 `simple_plans_test.go`。
- 本任务是纯文档分析，按任务约束未运行 Cargo。最终结构检查要求目标文件存在且恰好包含本页列出的十一个固定二级标题。
