# `pkg/planner/cascades/old/enforcer_rules.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-old` crate，负责旧版 Cascades 优化器的物理属性强制（enforcer）阶段。crate 入口 `pkg/planner/cascades/old/lib.rs` 将 `enforcer_rules` 声明为私有模块后公开重导出其符号；`pkg/planner/cascades/old/Cargo.toml` 通过 `../../../expression`、`../../core/base`、`../../core/operator/physicalop`、`../../implementation`、`../../memo`、`../../property` 和 `../../util` 等路径依赖接入表达式、物理计划、Memo、属性与代价实现。

它位于常规实现规则枚举之后的兜底路径：`pkg/planner/cascades/old/optimize.rs::Optimizer::implGroup` 先寻找自然满足所需物理属性的实现，再调用本文件的 `GetEnforcerRules`，必要时放宽孩子属性并在孩子实现上包一层 `PhysicalSort`。当前规则集只处理 TiDB 引擎上的非空排序需求，不处理 TiKV、TiFlash/MPP、分区或其他物理属性。

## 核心职责

- `Enforcer` trait 统一描述强制规则的三个阶段：生成孩子需要满足的放宽属性、把强制算子挂到孩子实现之上、估算强制算子自身代价。
- `GetEnforcerRules` 根据 `Group::EngineType` 和 `PhysicalProperty::SortItems` 选择候选规则；当前最多返回全局单例 `ORDER_ENFORCER`。
- `OrderEnforcer` 把父节点要求的排序列转换为 `PhysicalSort::ByItems`，保留孩子计划的上下文、统计、查询块偏移和 Schema，并产生新的 `SortImpl`。
- `SortPlan` 适配 `PhysicalSort` 与 `astersql-planner_implementation::{PlanAccess, SortCostPlan}`，使通用排序 Implementation 能读取计划、计算自代价，并在挂接孩子时克隆 Sort 后设置实际物理孩子。

## 主要符号

- `pub trait Enforcer`：公开规则接口。`NewProperty(&PhysicalProperty)` 返回递归实现孩子时使用的属性；`OnEnforce(&PhysicalProperty, ImplementationRef)` 组装强制后的实现；`GetEnforceCost(&Group)` 返回强制算子的估算代价。命名保留 Go 风格。
- `pub fn GetEnforcerRules(...) -> Vec<&'static dyn Enforcer>`：公开候选选择入口。`group.EngineType != EngineTiDB` 或 `property.IsSortItemEmpty()` 时返回空列表，否则返回唯一的 `&ORDER_ENFORCER`。
- `pub struct OrderEnforcer` 与私有静态量 `ORDER_ENFORCER`：无字段、无逐次分配状态的排序强制规则及其进程期单例。
- 私有 `struct SortPlan { plan: PhysicalSort }`：`PhysicalSort` 的适配包装，不作为 crate API 导出。
- `PlanAccess for SortPlan`：`Plan`/`PlanMut` 分别把包装内的 Sort 暴露为不可变/可变 `dyn PhysicalPlan`。
- `SortCostPlan for SortPlan`：`ExpectedCount` 从 Sort 的第 0 个孩子需求属性读取 `ExpectedCnt`；`SelfCost` 委托 `PhysicalSort::GetCost`；`InjectProjectionBelowSort` 克隆 Sort、设置传入孩子并返回完整物理子树。
- `Enforcer for OrderEnforcer`：实现排序属性放宽、Sort 构造和基于 Group 统计的代价估算。

## 执行流程

`Optimizer::implGroup` 中的强制路径按以下顺序运行：

1. `GetEnforcerRules(&group.borrow(), required)` 先检查执行引擎。只有 `EngineTiDB` 且 `required.SortItems` 非空时才产生 `OrderEnforcer`。
2. `OrderEnforcer::NewProperty` 创建默认 `PhysicalProperty`，把 `ExpectedCnt` 设为 `f64::MAX`。因此递归孩子不再承担父排序要求，也不受父级行数期望截断；默认值中的其他属性同样不会从输入属性复制。
3. `OrderEnforcer::GetEnforceCost` 从 Group 首个等价表达式取得 planner context，从 `Group::Prop` 取得统计和 Schema，构造一个仅用于估价的 `PhysicalSort`，以 `stats.RowCount` 调用 `PhysicalSort::GetCost`。该代价会先从 `cost_limit` 中扣除，再递归调用 `implGroup(group, &relaxed, ...)`。
4. 若递归没有得到孩子实现，当前 enforcer 被跳过。成功时 `OnEnforce` 从孩子计划复制 context、统计、query block offset 和 Schema，随后把每个 `required.SortItems` 映射为 `ByItems { Expr: item.Col.Clone(), Desc: item.Desc }`。
5. 新建 Sort 的第 0 个孩子需求属性仍是 `ExpectedCnt = f64::MAX`；设置 Schema 后调用 `Init`，再经 `NewSortImpl(Box::new(SortPlan { ... }))` 转为通用 Implementation，并用 `AttachChildren(&[child])` 挂接孩子。
6. `SortImpl::attach_children` 会克隆孩子物理计划并调用 `SortPlan::InjectProjectionBelowSort`；后者再克隆 Sort、设置孩子。因此最终 Implementation 持有可独立组树的 `PhysicalSort`。优化器把 `enforce_cost + child_cost` 写入结果，并与当前最佳实现比较。

需要区分两个代价入口：本文件 `GetEnforceCost` 用 Group 总行数预估 enforcer 并参与上限剪枝；`SortCostPlan::SelfCost` 则供通用 `SortImpl::calc_cost` 使用，后者以孩子统计行数和 `ExpectedCount` 的较小值计算 Sort 自代价。不过当前 enforcer 给 Sort 的孩子属性设置 `f64::MAX`，所以这里通常取孩子统计行数。

## 数据与状态

文件自身没有可变全局状态。`ORDER_ENFORCER` 是零大小、只读的 `'static` 单例；`GetEnforcerRules` 返回对它的 trait object 引用，避免为每次属性检查分配规则对象。

`OnEnforce` 的输入输出使用 `ImplementationRef = Rc<RefCell<dyn Implementation>>`。函数先短暂不可变借用孩子，克隆后续组装需要的 planner context、统计、query block offset 和 Schema，再显式 `drop(child_plan)`，之后才把原 `ImplementationRef` 移入 `AttachChildren`，避免 `RefCell` 借用跨越组装阶段。

排序键不会与原属性共享可变容器：`ByItems` 的 `Expr` 由每个 `SortItem.Col.Clone()` 构造，升降序位 `Desc` 按值复制。孩子属性也是新的默认对象，只保留显式设置的最大期望行数。最终 Sort 的 Schema 使用孩子 Schema 的 `Clone()`，统计和上下文也以各自的克隆语义传入。

## 依赖与调用关系

唯一直接生产调用者是同 crate 的 `pkg/planner/cascades/old/optimize.rs::Optimizer::implGroup`：它调用 `GetEnforcerRules`，依次调用 trait 的三个方法，递归实现放宽属性下的同一 Group，并把强制结果加入最佳实现竞争。`pkg/planner/cascades/old/lib.rs` 虽然公开重导出本文件符号，但精确 Rust 引用搜索未发现另一条生产调用链。

主要下游关系如下：

- `astersql_planner_cascades_pattern::EngineTiDB`：限制规则只在 TiDB 引擎 Group 上生效。
- `PhysicalProperty::IsSortItemEmpty`：其实现只检查 `SortItems.is_empty()`；`PartialOrderInfo` 等保持顺序信息不会单独触发本规则。
- `PhysicalSort::{New, Init, GetCost, Clone}` 与 `Plan::set_children`：分别负责创建、初始化、估价、复制和挂接实际 Sort 物理节点。
- `NewSortImpl`、`PlanAccess`、`SortCostPlan`：把具体 Sort 适配为通用、可计价的 Memo Implementation。相邻 `pkg/planner/implementation/sort.rs` 表明 `SortImpl` 挂孩子时可能在 Sort 下方注入 Projection，并在计算代价时加上孩子代价。
- `Group::{EngineType, Equivalents, Prop}`：提供候选筛选、planner context、统计和 Schema；`ImplementationRef` 提供孩子计划与共享可变 Implementation。
- `ByItems`：承载每个排序表达式和升降序方向。

RustCodeGraph 已索引目标文件及上述依赖，并能解析 `GetEnforcerRules`、`NewSortImpl`、`IsSortItemEmpty`、`PhysicalSort::GetCost` 等符号；其 `callers/callees` 对本文件的同名 Go/Rust 函数和 trait 方法未返回可用边，因此调用关系另以 `optimize.rs` 的精确符号引用核验。

## 错误处理与边界

本文件 API 不返回 `Result`，失败前置条件主要以 `expect` 或 `RefCell` 运行时借用检查体现：

- `GetEnforceCost` 要求 `group.Equivalents` 至少有一个表达式，否则以 `a memo group must contain at least one expression` panic；首表达式还必须保留 `SCtx`，否则以 `memo expression must retain planner context` panic。
- Group 必须有 Schema，否则 `memo group must retain schema` panic。统计缺失不会 panic，而是 `unwrap_or_default()` 后用默认统计估价；这可能得到与完整统计不同的成本。
- `InjectProjectionBelowSort` 假设 enforcer 构造的 Sort 可克隆；`Clone` 失败会以 `enforcer sort must remain cloneable` panic。
- `ExpectedCount` 无条件读取第 0 个孩子需求属性。本文件的 `OnEnforce` 总是传入恰好一个该属性，因而满足前置条件；若未来绕过此构造路径创建 `SortPlan`，空孩子属性会越界。
- `OnEnforce` 假设孩子当前没有冲突的可变 `RefCell` 借用，否则 `child.borrow()` 会 panic。它不验证 `required.SortItems` 非空，因为正常入口已由 `GetEnforcerRules` 过滤；直接调用仍可构造空排序项 Sort。
- `GetEnforcerRules` 只接受精确的 `EngineTiDB`。即使其他引擎能够执行排序，本文件也不会为其返回 enforcer。
- `PhysicalSort::GetCost` 至少按两行估价，并读取会话 CPU、内存、磁盘因子和内存配额；启用临时存储且估算内存超限时会包含 spill 磁盘代价。因此结果依赖会话配置与统计质量，而不是固定常数。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部 I/O。`Rc<RefCell<...>>` 明确限定在单线程所有权模型中；规则及优化器调用方必须串行访问共享 Implementation，不能把这些引用直接跨线程发送。

临时 `PhysicalSort` 在 `GetEnforceCost` 返回后立即释放，只用于估价。`OnEnforce` 创建的 Sort 被 `SortPlan` 独占，之后由 `SortImpl` 管理；挂接孩子时通过克隆物理孩子和 Sort 形成独立计划树，而 `ImplementationRef` 仍保存 Memo 层孩子以参与代价与生命周期管理。局部的孩子借用在组装前显式释放，不会被保存到返回对象中。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/old/enforcer_rules.go`，生产调用对照是 `pkg/planner/cascades/old/optimize.go::implGroup`。两边的核心语义一致：只为 TiDB Group 的非空排序属性返回一个 `OrderEnforcer`；放宽属性时清空排序并设置最大期望行数；先估算 Sort 代价并扣减成本上限；递归取得孩子后在上方加入 `PhysicalSort`；最终用孩子代价加 enforcer 代价参与最优实现选择。

Rust 移植存在以下可核验差异：

- Go 的 `orderEnforcer` 是 `*OrderEnforcer`，Rust 使用零大小静态值并返回 `&'static dyn Enforcer`。
- Go `OnEnforce` 直接把 `SortItem.Col` 放入 `ByItems`；Rust 显式 `Clone()` 列表达式，并额外从孩子复制 Schema 后调用 `SetSchema`。
- Go 的 `NewSortImpl(sort).AttachChildren(child)` 返回接口；Rust 先构造 `SortImpl`，再通过 `Rc<RefCell<dyn Implementation>>` 包装。为满足 Rust trait object 边界，本文件增加私有 `SortPlan` 适配器。
- Go `GetEnforceCost` 直接使用 `g.Prop.Stats` 和 `g.Prop.Schema`；Rust 在统计缺失时采用默认值，但 Schema 缺失会明确 panic。两边都依赖首个等价表达式提供会话上下文。
- Rust 的 Sort 挂接路径会通过 `SortCostPlan::InjectProjectionBelowSort` 克隆计划并设置孩子；这是 Rust `SortImpl` 抽象所需的适配细节，不改变“在孩子之上增加实际排序”的目标语义。

`pkg/planner/cascades/old/enforcer_rules_test.go` 与独立 Rust 测试 `enforcer_rules_test.rs` 都覆盖空排序属性不返回规则、非空排序属性恰好返回一个规则，以及 `NewProperty` 清空排序项。Go 通过类型断言确认 `OrderEnforcer`；Rust 因 `Enforcer` 未继承 `Any`，改用唯一规则的可观察行为确认。当前两边测试都没有直接覆盖非 TiDB 引擎、`OnEnforce` 组树或 `GetEnforceCost` 的前置条件与成本。

## 扩展指南

- 新增 enforcer 时先扩展 `GetEnforcerRules` 的引擎和属性判定，再实现完整的 `NewProperty`、`OnEnforce`、`GetEnforceCost` 三件套；放宽后的属性必须恰好移除由新算子负责满足的部分，不能无意丢弃仍应由孩子满足的任务类型、分区或其他约束。
- 修改排序强制时应同步检查 `OrderEnforcer::{NewProperty, OnEnforce, GetEnforceCost}`、私有 `SortPlan` 适配器、`Optimizer::implGroup` 的成本上限顺序，以及 `pkg/planner/implementation/sort.rs` 的实际挂接与代价逻辑，避免预估成本和最终 Implementation 成本模型分叉。
- 若支持 TiFlash/MPP 或其他引擎，不能只放宽 `EngineType` 判断；还需验证目标引擎可执行的 Sort 类型、属性传播、任务类型和成本因子，并与 Go 行为保持一致或记录有意差异。
- 若让 trait 可向下转型或新增状态，需重新评估静态单例的 `Sync` 要求及 `Rc<RefCell>` 单线程约束；当前无状态设计不应被误解为整个返回计划可跨线程共享。
- 测试必须继续放在独立的 `pkg/planner/cascades/old/enforcer_rules_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] mod enforcer_rules_test;` 装配。建议补充 EngineTiDB/非 TiDB 分支、排序列与 `Desc` 映射、孩子 context/statistics/offset/Schema 保留、Sort 孩子属性、总成本相加，以及空 Equivalents、缺 Schema、默认统计等边界；对应 Go 测试也应保持相同意图。
- 性能风险主要在排序本身的 `n log n` CPU、内存及可能的磁盘 spill，以及 `OnEnforce` 挂接时对孩子计划和 Sort 的克隆。新增强制候选还会增加 `implGroup` 的递归搜索分支，应确保成本上限剪枝仍在递归前扣除 enforcer 代价。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/cascades/old/enforcer_rules.rs` 确认目标文件已索引并包含 16 个符号。
- 目标源码：`pkg/planner/cascades/old/enforcer_rules.rs` 的 `Enforcer`、`GetEnforcerRules`、`OrderEnforcer`、`SortPlan` 及其三个 trait impl。
- crate 与模块边界：`pkg/planner/cascades/old/Cargo.toml`、`pkg/planner/cascades/old/lib.rs`。
- Rust 上游调用：`pkg/planner/cascades/old/optimize.rs::Optimizer::implGroup` 中连续调用 `GetEnforcerRules`、`NewProperty`、`GetEnforceCost`、递归 `implGroup` 和 `OnEnforce` 的分支。
- Rust 下游语义：`pkg/planner/property/physical_property.rs::IsSortItemEmpty`、`pkg/planner/implementation/sort.rs::{NewSortImpl, SortImpl::calc_cost, SortImpl::attach_children}`、`pkg/planner/core/operator/physicalop/physical_sort.rs::GetCost`、`pkg/planner/memo/implementation.rs::{Implementation, ImplementationRef}`。
- Go 对照与调用：`pkg/planner/cascades/old/enforcer_rules.go`、`pkg/planner/cascades/old/optimize.go::implGroup`。
- 独立测试：`pkg/planner/cascades/old/enforcer_rules_test.rs` 与 `pkg/planner/cascades/old/enforcer_rules_test.go`。它们验证规则选择和属性放宽，但不覆盖组树、代价及 panic 前置条件；文档未把这些未测试分支描述成已由测试保障。
- 本任务只新增说明文档，按计划未运行 Cargo。结构验证检查目标文件存在且恰好包含规定的十一个二级标题；人工复核覆盖了文件存在原因、执行链、状态与生命周期、Go 对照、边界和安全扩展入口。
