# `pkg/planner/core/operator/physicalop/nominal_sort.rs`

## 文件定位

本文件定义物理规划阶段的 `NominalSort`。它不是无条件执行排序的算子，而是把 `ORDER BY` 推导出的排序要求交给子计划：当排序键全是列时，已有物理属性可以满足要求，节点在挂接任务时直接消失；当排序键包含可接受的标量函数时，节点保留排序项和输出 Schema，供 Root 任务及后续投影改写处理。直接入口是 `physical_sort.rs::ExhaustPhysicalPlans4LogicalSort`，crate 根通过 `lib.rs` 的 `mod nominal_sort`、`pub use nominal_sort::*` 和 `impl_concrete_physical_plan!(NominalSort)` 将它接入 `base::PhysicalPlan`。

该文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`（`pkg/planner/core/operator/physicalop/Cargo.toml`，库入口为 `lib.rs`）。直接使用的内部依赖包括 `base`、`costusage`、`logicalop`、`property`、`planner_util`、`expression` 与 `plancodec`；Cargo 元数据把 Go 对照包声明为 `pkg/planner/core/operator/physicalop`。

## 核心职责

1. `FromLogical` 把 `logicalop::LogicalSort` 转为可行的名义排序候选，并区分 Root 与 MPP 的属性保留规则。
2. `Init` 保存统计信息、查询块偏移和唯一的子节点所需属性；`ByItems` 保存原始排序表达式，`OnlyColumn` 记录是否全为列引用。
3. `ResolveIndices` 先解析通用物理节点，再按第一个子节点的 Schema 重写每个排序表达式的列下标。
4. `Attach2Task` 在 `OnlyColumn` 为真时透传唯一子任务；否则克隆子计划和当前节点，把当前节点包装为新的 `RootTask`。
5. `Clone`、`MemoryUsage` 和两个代价接口提供物理计划框架需要的复制、记账和成本委托能力。

本文件不执行排序算法，也不编码 executor protobuf；真实 `PhysicalSort` 在 `physical_sort.rs`。对含标量函数的名义排序，最终“抽出表达式、保持原输出”的两层 Projection 语义由 `pkg/planner/core/rule_inject_extra_projection.rs::TurnNominalSortIntoProj` 描述，而不是在本文件内完成。

## 主要符号

- `pub struct NominalSort`：包含 `PhysicalSchemaProducer`、`Vec<planner_util::ByItems>` 形式的 `ByItems` 和布尔值 `OnlyColumn`。三个字段均公开，便于枚举器、克隆器和规则接线读取。
- `NominalSort::New(ctx)`：以 `plancodec::TypeSort` 和查询块偏移 `0` 构造空节点；此时排序项为空且 `OnlyColumn == false`。
- `NominalSort::Init(ctx, stats, offset, props)`：重建 `BasePhysicalPlan`，写入统计信息与子属性。调用方应传入与该一元算子相符的一个属性。
- `NominalSort::FromLogical(logical, required, mpp_only_columns) -> Option<Self>`：本文件的主要构造入口。不可从排序项推导属性、缺少上下文，或 MPP 路径出现非纯列排序时返回 `None`。
- `NominalSort::Clone(new_ctx) -> Result<Self, expression::Error>`：换用新上下文克隆基础计划，同时深克隆 Schema 与排序项。
- `NominalSort::ResolveIndices() -> Result<(), expression::Error>`：解析基础计划与排序表达式下标。
- `NominalSort::MemoryUsage() -> i64`：合计生产者、`Vec` 头部、按 capacity 计算的指针槽位、各 `ByItems` 和布尔字段。
- `NominalSort::Attach2Task(tasks) -> Box<dyn Task>`：按 `OnlyColumn` 选择透传或 Root 包装路径。
- `GetPlanCostVer1` / `GetPlanCostVer2`：原样委托 `BasePhysicalPlan`，本文件没有单独的名义排序成本公式。

文件内没有模块级常量、trait 定义、条件编译项或异步入口；trait 实现在相邻 `lib.rs` 中生成。

## 执行流程

Root 枚举路径始于 `physical_sort.rs::ExhaustPhysicalPlans4LogicalSort`。当所需属性与逻辑排序项匹配时，函数先生成一个真实 `PhysicalSort`，再调用 `NominalSort::FromLogical(logical, required, false)` 追加名义候选。`FromLogical` 调用 `GetPropByOrderByItemsContainScalarFunc`：列直接成为 `SortItem`；支持的单列标量函数被还原成对应列和方向；其他表达式使候选生成失败。Root 路径把 `required.ExpectedCnt` 和 `required.NoCopPushDown` 写入子属性。

MPP 路径调用 `FromLogical(logical, required, true)`，只接受 `only_columns == true`。它先克隆 `required` 的必要字段，再仅替换 `SortItems`，因此保留 MPP 任务类型等本质属性。两条路径都按 `required.ExpectedCnt` 缩放逻辑统计信息，复制逻辑 Schema、排序项、查询块偏移，并把推导出的属性作为唯一子属性交给 `Init`。

计划完成后，`lib.rs` 中 `ConcretePhysicalOperator::resolve_operator` 转发到 `ResolveIndices`。该函数先递归解析 `PhysicalSchemaProducer`；若不存在第一个子节点则安静返回成功，否则逐项调用 `item.Expr.ResolveIndices(child_schema)`，遇到首个表达式错误立即返回。

挂接阶段由 `ConcretePhysicalOperator::attach_operator_to_task` 转发到 `Attach2Task`。纯列路径取出第一个子任务并直接返回；非纯列路径克隆所有传入任务的计划，克隆当前 `NominalSort` 到相同上下文，安装这些孩子并创建 `RootTask`。随后规划器的额外投影规则可把名义排序消去：`OnlyColumn` 为真时返回孩子；否则在孩子之上增加一个物化标量排序键的底部 Projection，并在顶层 Projection 剪掉额外列（`rule_inject_extra_projection.rs::TurnNominalSortIntoProj`）。

## 数据与状态

`PhysicalSchemaProducer` 是主要继承式状态，持有 `BasePhysicalPlan`、输出 Schema、孩子、统计信息、上下文和子属性。`Init` 会替换其中的基础计划，因此调用顺序应是先准备 `ByItems`/`OnlyColumn`/Schema，再以最终统计信息和子属性完成初始化；`FromLogical` 正是按此顺序操作。

`ByItems` 保留原逻辑排序表达式及升降序方向。它与子属性中的 `SortItems` 角色不同：前者供索引解析、克隆和投影物化使用，后者告诉优化器子节点必须提供什么顺序。`OnlyColumn` 是关键不变量：它必须与 `GetPropByOrderByItemsContainScalarFunc` 的第二个结果一致，否则 `Attach2Task` 会错误地消去或保留节点。

统计信息不会在本文件重新推导，只从逻辑节点取得后按期望行数缩放。成本状态也全部位于 `BasePhysicalPlan`。`Clone` 会复制 Schema 和每个 `ByItems`，不共享可变排序项；上下文则使用调用方传入的 `ContextRef`。

## 依赖与调用关系

上游直接调用边为：

- `physical_sort.rs::ExhaustPhysicalPlans4LogicalSort -> NominalSort::FromLogical`：Root 与 MPP 新优化器枚举入口。
- `cascades/old/implementation_rules.rs::ImplSort::OnImplement -> NominalSort::New/Init`：旧 Cascades 在排序项可转为属性时构造名义排序，并交给 `implementation::NewNominalSortImpl`。
- `lib.rs::ConcretePhysicalOperator for NominalSort -> ResolveIndices/MemoryUsage/Attach2Task/GetPlanCostVer1/GetPlanCostVer2`：统一的动态物理计划接口接线。

主要下游边为：

- `FromLogical -> GetPropByOrderByItemsContainScalarFunc`、`LogicalPlan::{SCtx, StatsInfo, Schema, QueryBlockOffset}`、`StatsInfo::ScaleByExpectCnt` 和 `PhysicalProperty::CloneEssentialFields`。
- `ResolveIndices -> PhysicalSchemaProducer::ResolveIndices -> ByItems.Expr::ResolveIndices`。
- `Attach2Task -> Task::plan -> PhysicalPlan::clone_physical -> NominalSort::Clone -> PhysicalPlan::set_children -> RootTask::New`。
- 成本接口直接转发给 `BasePhysicalPlan::{GetPlanCostVer1, GetPlanCostVer2}`。

另有一条简化 `PlanNode` 路径位于 `pkg/planner/core/task.rs`：`attach2Task` 对 `PlanKind::NominalSort` 分派到 `attach2Task4NominalSort`，它依据 `partial_order` 决定转换为 `Sort` 或透传。该路径使用不同的数据模型，不能当作本文件 `NominalSort::Attach2Task` 的逐行实现，但共同表达“条件满足时消去名义节点”的语义。

## 错误处理与边界

`FromLogical` 用 `Option` 表示“不生成候选”，覆盖缺少 `SCtx`、排序项无法转换，以及 MPP 中含标量函数三种正常拒绝情形；它们不是运行错误。若逻辑统计缺失，使用默认 `StatsInfo`，不会中止枚举。

`Clone`、`ResolveIndices` 和成本接口传播 `expression::Error`。`ResolveIndices` 在没有孩子时返回 `Ok(())`，因此它不负责强制一元结构；有孩子时仅使用第一个孩子的 Schema。`Attach2Task` 的纯列路径在没有任务时以 `expect("NominalSort requires one child task")` 触发 panic；非纯列路径对孩子计划克隆和自身克隆也使用 `expect`。因此调用框架必须保证节点已正确初始化、至少有一个有效子任务且克隆不会失败。

`Attach2Task` 的纯列路径若收到多个任务，只返回第一个；非纯列路径则克隆全部传入任务为孩子。按算子语义它应始终是一元节点，当前方法没有显式检查“恰好一个”。扩展或新调用者不应依赖这些宽松/崩溃行为来校验拓扑。

内存计算与 Go 保持“slice 容量而非长度”的记账方式，但 Rust 实现没有 Go 的 nil receiver 情形。`MemoryUsage` 使用指针大小估算 `Vec<ByItems>` 的容量槽位，这是对 Go `[]*ByItems` 存储模型的兼容记账，不等同于 Rust `ByItems` 实际内联元素大小。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。`ContextRef` 与 trait object 的所有权由规划器管理；`New`/`Init`/`Clone` 只克隆引用计数上下文或拥有的数据。

`Attach2Task` 消耗传入的任务向量。纯列分支把第一个 `Box<dyn Task>` 的所有权移交给调用者；非纯列分支只借用每个任务来克隆其计划，随后原任务随向量离开作用域而释放，新 `RootTask` 拥有克隆后的计划树。`ResolveIndices` 和成本方法需要可变访问，因此同一节点上的调用顺序与并发同步由上层保证；本文件没有内部同步。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/planner/core/operator/physicalop/nominal_sort.go`。字段语义一致：两者都保存基础物理计划、`ByItems` 与 `OnlyColumn`；`Init` 都安装 TypeSort、统计信息、偏移和子属性；`MemoryUsage` 都计算基础计划、slice 头部、容量指针槽、元素和布尔值。

Go `ResolveIndices` 委托 `resolveIndicesForSort(&p.BasePhysicalPlan)`；Rust 展开为先解析 `PhysicalSchemaProducer`，再逐个按首个孩子 Schema 解析 `ByItems.Expr`。Go `Attach2Task` 通过 `utilfuncp.Attach2Task4NominalSort` 跨包回调，Rust 则在本类型上直接实现任务挂接。Rust 还把 Go `physical_sort.go` 中 `getNominalSort` 与 `getNominalSortSimple` 的共同逻辑收敛进 `FromLogical(mpp_only_columns)`：Root 复制 `ExpectedCnt`/`NoCopPushDown`，MPP 克隆必要字段且拒绝非纯列排序，与 Go 两函数的分支意图一致。

需要注意当前 Rust 文件的非纯列 `Attach2Task` 会把克隆的 `NominalSort` 包装进 `RootTask`，并未在该方法内把类型改成 `PhysicalSort`；标量表达式物化的可观察结构应结合后续 `TurnNominalSortIntoProj` 检查。文档因此不把文件头注释中的“真实排序计划”扩大解释为此方法已经构造 `PhysicalSort`。

## 扩展指南

新增排序键类型时，首先修改 `physical_sort.rs::GetPropByOrderByItemsContainScalarFunc` 的可转换规则，再同步审查 `FromLogical` 的 Root/MPP 接受条件和 `OnlyColumn` 不变量；不要只在 `NominalSort` 中保存新表达式，否则子属性与投影物化可能不一致。新增 MPP 字段时，应确认 `PhysicalProperty::CloneEssentialFields` 会保留该字段。

若改变挂接语义，修改点是 `NominalSort::Attach2Task` 及 `lib.rs` 的 `ConcretePhysicalOperator` 接线，并同步核对 `rule_inject_extra_projection.rs::TurnNominalSortIntoProj` 和简化模型 `task.rs::attach2Task4NominalSort`。应增加独立测试文件中的用例，而不是把测试嵌入本源文件：

- `nominal_sort_test.rs`：补充空/单/多任务、`OnlyColumn` 两分支、克隆错误与索引解析边界。
- `physical_sort_test.rs`：补充 Root/MPP、列/标量函数/不可转换表达式的候选枚举和属性保留。
- `rule_inject_extra_projection_test.rs`：补充标量键物化、输出 Schema 保持和多排序项顺序。

兼容风险集中在 Go 语义漂移、MPP 属性丢失及错误地消去排序；正确性测试应验证输出顺序和 Schema，而非只验证候选类型。性能风险主要是非纯列路径克隆所有孩子计划、Projection 物化额外表达式，以及 `ExpectedCnt` 改变造成的成本选择变化。

## 验证依据

- RustCodeGraph：`status` 报告本仓库索引含 11,467 个文件；`node --file pkg/planner/core/operator/physicalop/nominal_sort.rs --offset 1 --limit 400` 返回目标文件完整 209 行；`query NominalSort`、`query FromLogical`、`query Attach2Task` 定位 Rust/Go 对照符号、`TurnNominalSortIntoProj`、`PlanKind::NominalSort` 与 `NewNominalSortImpl`。精确哈希 ID 的 `callers/callees` 查询产生全库同名歧义，因此未把其噪声结果作为调用边证据，未覆盖边改由下列源码搜索核实。
- 目标实现：`pkg/planner/core/operator/physicalop/nominal_sort.rs` 的 `NominalSort::{New, Init, FromLogical, Clone, ResolveIndices, MemoryUsage, Attach2Task, GetPlanCostVer1, GetPlanCostVer2}`。
- crate 与 trait 接线：`pkg/planner/core/operator/physicalop/Cargo.toml`；`pkg/planner/core/operator/physicalop/lib.rs` 的模块声明、再导出、`ConcretePhysicalOperator for NominalSort` 和 `impl_concrete_physical_plan!`。
- 上下游：`physical_sort.rs::ExhaustPhysicalPlans4LogicalSort`、`GetPropByOrderByItemsContainScalarFunc`；`cascades/old/implementation_rules.rs::ImplSort`；`core/rule_inject_extra_projection.rs::TurnNominalSortIntoProj`；`core/task.rs::attach2Task4NominalSort`。
- Go 对照：`pkg/planner/core/operator/physicalop/nominal_sort.go` 与 `physical_sort.go::{getNominalSort, getNominalSortSimple, GetPropByOrderByItemsContainScalarFunc}`。
- 独立 Rust 测试：`nominal_sort_test.rs::memory_usage_counts_go_slice_storage_for_by_items` 验证容量记账；`physical_sort_test.rs::root_sort_enumerates_physical_and_nominal_candidates_like_go` 验证 Root 枚举两个候选；`core/rule_inject_extra_projection_test.rs::nominal_sort_matches_go_passthrough_and_two_projection_paths` 验证透传和双 Projection 路径。现有测试未直接覆盖本文件 `Attach2Task`、`ResolveIndices`、`Clone` 或 MPP 拒绝分支，属于后续扩展时应补的验证缺口。
