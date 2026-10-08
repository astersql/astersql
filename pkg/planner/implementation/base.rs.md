# `pkg/planner/implementation/base.rs`

## 文件定位

本文件是 `astersql-planner-implementation` crate 的公共基础层，源码由 [`lib.rs`](lib.rs) 的私有 `base` 模块加载并整体再导出。它位于 Cascades/Memo 物理优化链的中间：上游 [`pkg/planner/cascades/old/implementation_rules.rs`](../cascades/old/implementation_rules.rs) 把具体 `PhysicalPlan` 包装成这里定义的访问与代价 trait，下游 [`datasource.rs`](datasource.rs)、[`join.rs`](join.rs)、[`simple_plans.rs`](simple_plans.rs) 和 [`sort.rs`](sort.rs) 用这些 trait 构造实现候选，并通过 `impl_implementation!` 接入 Memo 的 `Implementation` 接口。

该文件不是独立的优化器入口，也不决定候选规则集合；候选规则及物理算子适配位于 `implementation_rules.rs`。它负责统一“缓存代价、读取计划、挂接子计划、折算子节点代价上限”这组基础协议。crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，包名为 `astersql-planner-implementation`，移植元数据指向 Go 包 `pkg/planner/implementation`。

## 核心职责

1. `BaseImpl` 用 `Cell<f64>` 保存 Implementation 的已计算代价，并提供默认的子树代价求和与剩余代价上限计算。
2. `PlanAccess` 将具体物理计划统一暴露为只读/可变 `dyn PhysicalPlan`；其余十一个 `*CostPlan` trait 在此基础上描述各算子代价公式所需的最小参数，而不把具体物理算子类型耦合进本 crate 的公共计算逻辑。
3. `ClonePlan`、`CloneChildren` 和 `AttachChildren` 把 Memo 中共享的子 Implementation 转成拥有所有权的物理计划子树；`ChildCost`、`ChildRows` 为具体代价公式提供按序读取辅助。
4. `impl_implementation!` 把每个具体 `*Impl` 的内部方法与 `base: BaseImpl` 字段映射到 `astersql_planner_memo::Implementation`。该宏消除重复接线，但不生成具体代价公式。

这些职责共同保证 Memo 可以用统一接口枚举和比较物理候选，同时让代价参数仍由真实物理节点及会话配置提供。

## 主要符号

- `BaseImpl { cost: Cell<f64> }`：唯一的模块级结构体。`CalcCost(out_count, children)` 忽略默认实现不需要的 `out_count`，累加所有孩子的 `GetCost()` 后写回缓存；`SetCost`/`GetCost` 直接读写缓存；`ScaleCostLimit` 原样返回上限；`GetCostLimit` 返回总上限减去已有孩子代价之和。`Default` 的初始代价为 `0.0`。
- `PlanAccess`：要求实现 `Plan() -> &dyn PhysicalPlan` 与 `PlanMut() -> &mut dyn PhysicalPlan`。`PlanAdapter<T>` 在 `implementation_rules.rs` 中为任意 `T: PhysicalPlan` 实现此接口。
- `ReaderCostPlan`：提供表相关网络因子、基于 `HistColl` 与子计划的平均行宽、Coprocessor worker 数；由表/索引 Reader 使用。
- `TableScanCostPlan`：提供投影列行宽、正序/逆序扫描因子及方向；`IndexScanCostPlan` 进一步提供 Seek 因子与 range 数量。
- `BinaryJoinCostPlan`、`ProjectionCostPlan`：分别以左右输入行数、单个输入行数计算算子自身代价。
- `SelectionCostPlan`：根据是否为 coprocessor 侧返回 CPU 因子。`HashAggCostPlan`、`TopNCostPlan` 通过 `root` 区分 TiDB 根侧与下推侧。
- `UnionAllCostPlan`：提供并发因子。`ApplyCostPlan` 同时接收左右行数与左右子代价，并暴露是否存在左侧过滤条件。
- `SortCostPlan`：提供期望行数、自身排序代价，以及在 Sort 下方注入 Projection 后返回新物理树的操作。
- `ClonePlan`：调用 `PhysicalPlan::clone_physical`，沿用原计划的 `s_ctx().clone()`；克隆失败会触发带固定信息的 `expect`。
- `CloneChildren`/`AttachChildren`：逐个借用 `ImplementationRef`、读取 `GetPlan` 并克隆，随后用 `PhysicalPlan::set_children` 一次性替换目标计划的 children。
- `ChildCost`/`ChildRows`：按索引分别读取缓存代价和 `stats_info().RowCount`；索引有效性由调用方保证。
- `impl_implementation!`：要求目标类型具有 `base: BaseImpl`，以及 `calc_cost`、`plan`、`attach_children`、`cost_limit` 四个内部方法；生成的 `SetCost` 委托 `BaseImpl`，其他方法转发到对应内部方法并让 `AttachChildren` 返回 `self`。

文件没有模块级常量、枚举、条件编译项或异步函数。除 `impl_implementation!` 仅以 `pub(crate)` 暴露外，上述结构体、trait 和 helper 均经 `lib.rs` 再导出为 crate 公共 API。

## 执行流程

典型候选的执行链如下：

1. `implementation_rules.rs` 中的某条 `ImplementationRule::OnImplement` 持有具体物理算子，并用 `PlanAdapter<T>` 实现本文件的 `PlanAccess` 及相应 `*CostPlan`。
2. `New*Impl` 构造器把适配器装入 `datasource.rs`、`join.rs`、`simple_plans.rs` 或 `sort.rs` 的具体 Implementation，并初始化 `BaseImpl`。
3. Memo 通过 `Implementation::CalcCost` 调用由 `impl_implementation!` 生成的转发方法；具体实现读取 `ChildRows`/`ChildCost`，或委托 `BaseImpl::CalcCost`，再用 `BaseImpl::SetCost` 缓存结果。
4. 枚举下一子 Group 前，Memo 调用 `GetCostLimit`。大多数实现委托 `BaseImpl::GetCostLimit` 扣除已选孩子代价；Reader、UnionAll、Apply 等会在各自文件中按并发或执行次数重写折算方式。
5. 选中候选后，`Implementation::AttachChildren` 转发到具体 `attach_children`。多数实现调用本文件的 `AttachChildren`，将子计划克隆后交给 `set_children`；Sort 因需注入 Projection，直接调用 `ClonePlan` 并保存改写后的树。
6. 上层通过 `GetPlan` 取得已经挂接的物理计划，继续组装最终计划树。

具体调用证据包括：`join.rs` 用 `ChildRows` 和 `ChildCost` 组合二元 Join 代价；`simple_plans.rs` 的 Projection、Selection、HashAgg、TopN、Apply 等使用同一组 helper；`datasource.rs` 的各实现使用 `AttachChildren` 和扫描相关 trait；`sort.rs` 使用 `ClonePlan` 处理真实与名义 Sort。

## 数据与状态

`BaseImpl.cost` 是本文件持有的唯一状态。`Cell<f64>` 提供内部可变性，因此 `CalcCost(&self, ...)` 和 `SetCost(&self, ...)` 可在不可变引用下更新代价；宏生成的 Memo `SetCost(&mut self, ...)` 则满足外部 trait 的可变签名后再委托给它。该字段没有“尚未计算”的独立标志，默认值 `0.0` 同时可能表示真实零代价或尚未写入，调用顺序由 Memo 协议保证。

`ImplementationRef` 在 Memo crate 中定义为 `Rc<RefCell<dyn Implementation>>`。本文件仅在局部借用期间读取子代价和子计划，不保存借用；克隆后的 `Box<dyn PhysicalPlan>` 拥有独立物理树节点。`CloneChildren` 保持输入顺序，因而 `ChildRows(children, 0/1)` 等位置语义与 Join/Apply 的左右孩子约定一致。

本文件不持有 `HistColl`、`TableInfo`、`Schema` 或 `Column`；这些类型只出现在代价 trait 参数中，由具体 Implementation 持有并传入。`out_count` 在默认 `BaseImpl::CalcCost` 中不参与计算，但仍保留在接口中供具体算子公式使用。

## 依赖与调用关系

- `astersql-planner-memo`：提供 `Implementation` 和 `ImplementationRef`，是本文件对 Memo 的核心上游协议。
- `astersql-planner-core-base`：提供 `PhysicalPlan`，支撑计划访问、统计行数、克隆和 children 挂接。
- `astersql-expression`：提供 `Column`、`Schema`，分别用于表扫描行宽和 Sort 自身代价接口。
- `astersql-meta-model`：提供 `TableInfo`，用于 Reader 网络因子。
- `astersql-statistics`：提供 `HistColl`，用于 Reader/Scan 行宽估算。

`Cargo.toml` 还声明了 implementation crate 的其他运行时依赖；就本文件的直接 `use` 而言仅涉及以上五个 crate。直接使用本文件的生产文件由 RustCodeGraph 识别为 `datasource.rs`、`join.rs`、`simple_plans.rs`、`sort.rs` 和 `pkg/planner/cascades/old/implementation_rules.rs`，另有独立测试 `base_test.rs`。其中 `implementation_rules.rs` 是 trait 的主要生产实现方，四个同 crate 文件是 helper、`BaseImpl` 与宏的主要消费方。

## 错误处理与边界

- `ClonePlan` 将不可克隆的物理计划视为内部不变量破坏，使用 `expect("memo implementation child plan must be cloneable")` 直接 panic，而不是返回 `Result`。安全扩展时必须保证进入 Memo Implementation 的计划实现了可成功执行的 `clone_physical`。
- `ChildCost`、`ChildRows`、Reader/Join/Sort 等调用点直接用下标访问 children；孩子数量或左右顺序不正确会 panic。arity 由匹配规则和具体物理算子协议保证，本层不重复校验。
- `RefCell::borrow()` 可能在存在冲突可变借用时 panic。本文件的读取都限制在表达式或迭代闭包内，调用者不应在同一 `ImplementationRef` 的可变借用存活期间调用这些 helper。
- 浮点值没有归一化或有限性检查：`GetCostLimit` 可以产生负数；NaN/无穷大按 IEEE-754 传播。`base_test.rs` 进一步确认 Reader worker 为零时可出现正无穷，以及 Sort 的 NaN 期望行数必须传播；这些属于与 Go 行为保持一致的边界，而非本文件主动纠正的数据。
- `AttachChildren` 替换而非追加目标计划的 children，并始终使用克隆；调用方若需要特殊树改写，应像 `SortImpl` 一样覆盖自己的挂接逻辑。

## 并发与资源生命周期

本模块没有线程、异步任务、通道、锁、事务或外部 I/O。`ImplementationRef` 使用单线程的 `Rc<RefCell<_>>`，`BaseImpl` 使用非线程安全的 `Cell<f64>`，因此这些类型不表达跨线程共享能力；物理优化过程应在其所属线程内使用它们。

子 Implementation 的 `RefCell` 借用只持续到一次代价/计划读取结束。`ClonePlan` 以克隆的 `ContextRef` 创建新的物理计划所有权，`CloneChildren` 返回的 `Vec<Box<dyn PhysicalPlan>>` 随后交给父计划；父计划负责这些 Box 子树的生命周期。`BaseImpl` 无需显式清理，其缓存随具体 `*Impl` 一同释放。

## 与 Go 版本的对应关系

直接对照文件是 [`base.go`](base.go)，测试对照是 [`base_test.go`](base_test.go)。语义对应如下：

- Go `baseImpl.cost` 对应 Rust `BaseImpl.cost`；空 children 的 `CalcCost` 返回并缓存 `0.0`，非空时累加所有子代价；`SetCost`、`GetCost`、`ScaleCostLimit` 和“总上限减孩子代价”的 `GetCostLimit` 保持一致。
- Go `baseImpl` 同时持有 `plan PhysicalPlan`，Rust 将计划字段下沉到每个具体 `*Impl`，并用 `PlanAccess`/`*CostPlan` trait 解耦具体节点；因此 Rust `BaseImpl` 只负责代价状态。
- Go `AttachChildren` 直接收集孩子的 `GetPlan()` 并挂接；Rust 为满足 trait object 所有权要求，通过 `clone_physical` 克隆后再挂接。最终保持孩子顺序和整组替换语义，但 Rust 多了“必须可克隆”的 panic 边界。
- Go 主要依靠具体物理计划方法直接取代价参数；Rust 用多个 `*CostPlan` trait 显式列出依赖，并由 `PlanAdapter<T>` 调回物理计划、统计模块和会话变量。
- Go 通过嵌入 `baseImpl` 复用 Memo 接口；Rust 通过 `impl_implementation!` 为每个具体类型生成 `Implementation` 实现。

Rust 独立测试 `base_test.rs` 保留 Go `TestBaseImplementation` 的计划访问、空子代价求和和代价读写意图，并补充非空 children、上限、克隆/挂接、零代价算子不覆盖缓存、Reader 行宽来源与零 worker 浮点行为、Sort NaN 传播等移植边界。测试逻辑位于独立文件，未内嵌到生产源码。

## 扩展指南

- 新增普通物理算子时，先判断现有 `PlanAccess` 或 `*CostPlan` 是否已表达其全部代价输入。若足够，在具体 implementation 文件中复用它并调用 `impl_implementation!`；若不够，应在本文件新增范围最小的 trait 方法，并在 `implementation_rules.rs` 的相应 `PlanAdapter<PhysicalX>` 上实现。
- 新代价公式应明确区分“算子自身代价”和“子树已缓存代价”，决定是否使用 `BaseImpl::CalcCost`、`ChildCost`/`ChildRows`，并在返回前通过 `SetCost` 缓存。还需同步定义代价上限如何反推到下一子节点，不能机械套用默认减法。
- 新 arity 或可选孩子结构若调用 `ChildCost`/`ChildRows`，必须在规则匹配处保证索引存在与顺序稳定；如无法保证，应先在具体实现中显式处理空/缺失孩子。
- 需要特殊树改写的算子不应直接使用通用 `AttachChildren`。可仿照 `SortImpl` 克隆子树并保存改写结果，但必须验证 `clone_physical` 的支持情况和上下文复制语义。
- 修改公共协议时同步更新独立测试 [`base_test.rs`](base_test.rs)，并核对 Go 的 `base.go`/`base_test.go`；算子专属公式还应更新其同目录独立测试。兼容风险主要是宏要求的字段/方法名、trait object API 变化和 children 顺序；性能风险主要来自不必要的整树克隆、重复统计访问，以及错误的并发因子或代价上限折算。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；本次查询时索引可用。
- RustCodeGraph `node --file pkg/planner/implementation/base.rs`：核对 256 行完整源码、全部公开符号、宏和 5 个直接使用文件。
- RustCodeGraph `query`：核对 `BaseImpl`、`ClonePlan`、`CloneChildren`、`AttachChildren`、`ChildCost`、`ChildRows`、`PlanAccess`、`ReaderCostPlan`、`SortCostPlan` 的定义与候选使用点。
- RustCodeGraph `callees CloneChildren`：确认其调用 `ClonePlan`、`Implementation::GetPlan` 及共享引用借用；索引未返回若干 helper 的 caller 列表，因此再以目标文件的直接使用文件和精确符号搜索补齐证据。
- RustCodeGraph 文件读取：核对 `datasource.rs`、`join.rs`、`simple_plans.rs`、`sort.rs`、`pkg/planner/cascades/old/implementation_rules.rs` 与 `pkg/planner/memo/implementation.rs` 中的实际调用、trait 实现和 Memo 接口。
- 直接读取未由图覆盖的 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`base.go`](base.go)、[`base_test.go`](base_test.go) 和 [`base_test.rs`](base_test.rs)，核对 crate 依赖、模块再导出、Go 对照及测试边界。`pkg/planner/implementation` 目录不存在 `doc.go`，因此没有更近的包级 Go 契约可读取。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构验证要求目标文件存在且恰有上述十一个固定二级章节。
