# `pkg/planner/core/telemetry.rs`

## 文件定位

`telemetry.rs` 属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），提供规划器侧的 TiFlash/MPP 使用情况判定。模块在 `pkg/planner/core/lib.rs` 中以私有 `mod telemetry` 挂载，再通过 `pub use telemetry::*` 将唯一公开函数 `IsTiFlashContained` 暴露到 crate 根。

这个文件只读取一棵轻量 `PlanNode` 计划树并返回两个布尔值，不直接修改遥测计数器。Go 应用中的完整链路是 `pkg/session/session.go:isInternal` 所在的语句收尾逻辑调用 `pkg/planner/core/telemetry.go:IsTiFlashContained`，再更新 `CurrentTiFlashPushDownCount` 和 `CurrentTiFlashExchangePushDownCount`。仓库内尚未找到 Rust 生产代码对 Rust 版函数的调用；当前 Rust 文件是已公开且有独立单元测试的移植实现，而不是已验证接入 Rust 会话主链的实现。

## 核心职责

- `IsTiFlashContained(&PlanNode) -> (bool, bool)` 判断计划中是否出现 `StoreType::TiFlash` 的 `PlanKind::TableReader`。
- 返回值第一项表示找到了 TiFlash `TableReader`；第二项只在该 reader 的表计划根（Rust 表示为第一个子节点）是 `PlanKind::ExchangeSender` 时为真，用来区分一般 TiFlash 下推与 MPP exchange 下推。
- 遍历只覆盖物理计划：遇到用 `PlanKind::DataSource` 或 `PlanKind::Join` 表示的逻辑节点立即停止该分支。
- 找到首个 TiFlash reader 后短路返回，避免继续扫描其余兄弟子树。该短路与 Go 实现的 `if tiFlashPushDown { return }` 一致。

该函数不负责判断任意 TiFlash 算子、普通 `TableScan`，也不递归搜索 `TableReader` 内部的任意层级 `ExchangeSender`；这些限制是遥测口径的一部分，而不是遗漏。

## 主要符号

- `pub fn IsTiFlashContained(plan: &PlanNode) -> (bool, bool)`：文件内唯一函数和唯一公开 API。输入是不可变借用，因此调用方保有计划树所有权；输出依次为“TiFlash reader 存在”和“该 reader 的表计划根是 ExchangeSender”。
- `crate::PlanNode`：定义于 `pkg/planner/core/common_plans.rs`，关键字段是 `kind: PlanKind`、`children: Vec<PlanNode>` 和 `store_type: StoreType`。本函数不读取节点的代价、行数或运行时统计字段。
- `crate::PlanKind`：同样定义于 `common_plans.rs`。本文件显式识别 `DataSource`、`Join`、`TableReader`、`ExchangeSender` 四类，其余变体只作为可继续遍历的中间物理节点。
- `crate::StoreType`：`Root`、`TiKV`、`TiFlash` 三值枚举；只有 `StoreType::TiFlash` 能令第一返回值为真。

文件没有模块级常量、结构体、trait、`impl`、条件编译项或可变静态状态。

## 执行流程

1. 入口先检查当前节点是否为 `PlanKind::DataSource` 或 `PlanKind::Join`。二者代表当前轻量模型中的逻辑节点，对应 Go 中无法断言为 `base.PhysicalPlan` 的情况；命中即返回 `(false, false)`，其后代不会被观察。
2. 若当前节点是 `PlanKind::TableReader`，用 `plan.store_type == StoreType::TiFlash` 计算第一标志。
3. 只有第一标志为真时，才读取 `children.first()`；若第一个子节点存在且其 `kind` 是 `PlanKind::ExchangeSender { .. }`，第二标志为真。没有子节点不会报错，而是得到 `false`。
4. `TableReader` 分支无论是否为 TiFlash 都立即返回，不把 reader 的内部 table plan 当作普通子树继续递归。因此，TiFlash reader 下 `Projection -> ExchangeSender` 返回 `(true, false)`；TiKV reader 即使子节点含 TiFlash reader 也不会继续向内搜索。
5. 对其他节点，按 `children` 顺序递归调用 `IsTiFlashContained`。某个子树返回第一标志为真时立即原样返回该二元组；如果所有子树均未命中，返回 `(false, false)`。

由步骤 5 可知，结果依赖深度优先、从左到右的首个 TiFlash reader。由于任何 `exchange == true` 都蕴含 `tiflash == true`，短路不会产生 `(false, true)`。

## 数据与状态

函数只消费 `PlanNode` 的不可变视图，不持有引用、不缓存结果，也不写入计划节点或全局遥测状态。唯一中间状态是当前栈帧中的 `(bool, bool)` 返回值以及 `tiflash`、`exchange` 两个局部布尔值。

重要不变量如下：

- 第二返回值为真时，第一返回值必为真；该约束由 `let exchange = tiflash && ...` 保证。
- `children.first()` 在 `TableReader` 上被解释为 Go `PhysicalTableReader.GetTablePlan()` 的根，而不是普通子计划集合中的任意命中。
- 遍历顺序稳定地跟随 `Vec<PlanNode>` 顺序，并在首个 TiFlash reader 处结束。
- 空树不能以 `&PlanNode` 表示；Rust API 因此没有 Go 顶层 `nil plan` 分支。空子节点列表是合法的，并安全返回未命中或仅 TiFlash 命中。

## 依赖与调用关系

直接依赖仅有 crate 内的 `PlanNode` 与 `StoreType`，以及通过全限定路径使用的 `PlanKind`；`Cargo.toml` 没有为本文件引入专属外部依赖或 feature。`nextgen` feature 也不改变本模块的编译或行为。

模块接线为 `pkg/planner/core/lib.rs: mod telemetry` → `pub use telemetry::*` → crate 根公开 `IsTiFlashContained`。RustCodeGraph 能识别 Rust 和 Go 两个同名定义，但其文件限定 `callers` 把 Go `pkg/session/session.go:isInternal` 返回为调用者，反映的是跨语言同名聚合而非已确认的 Rust 调用边；原始 Rust 搜索仅找到函数自身的递归调用和 `pkg/planner/core/telemetry_test.rs` 的测试调用。因此，不能据此宣称 Rust 会话层已经接线。

Go 的实际上游位于 `pkg/session/session.go`：仅对非内部会话且全局 `EnableTelemetry` 开启、语句成功结束的场景调用 Go 版判定函数。下游不是服务或 I/O，而是 `PlanNode.children` 的递归遍历、`PlanKind` 模式匹配和 `StoreType` 比较。

## 错误处理与边界

函数返回纯布尔结果，没有 `Result`、错误码、日志或 panic 约定。通过 `children.first().is_some_and(...)` 安全处理无表计划根的 `TableReader`。在正常构造的树上，未发现 TiFlash、逻辑节点、空 children 和非 TiFlash reader 都属于普通阴性结果，不是错误。

边界和已知表示差异包括：

- Rust 接口不接受空计划；Go 的 `plan == nil` 返回两个假值。
- Go 会先将 `*Explain` 解包到 `TargetPlan`，目标为空时返回；当前 `PlanKind` 没有 `Explain` 变体，Rust 函数也没有等价解包逻辑。若 Rust 调用链未来用其他类型表示 Explain，必须在调用前归一化或扩展此处模型。
- Rust 以 `DataSource` 和 `Join` 两个变体近似 Go 的“不是 `base.PhysicalPlan`”边界。若未来增加新的逻辑专用 `PlanKind`，必须同步更新物理性判定，否则函数会错误进入其子树。
- 递归深度与计划树深度一致；本文件没有显式深度限制。异常深的手工构造计划可能消耗较多栈空间，但正常计划的深度由上游规划过程约束。

## 并发与资源生命周期

`IsTiFlashContained` 不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。它只借用调用方传入的计划树，借用在函数返回时结束；递归期间也只创建更短的子节点共享借用。

函数本身没有共享可变状态，因此对不同不可变计划树可并发调用。它不负责 Go 会话层遥测计数器的并发安全与生命周期；计数器递增发生在 `pkg/session/session.go`，不在本 Rust 文件中。时间复杂度最坏为 `O(n)`，其中 `n` 是被物理边界允许访问的节点数；首个 TiFlash reader 可提前结束。额外堆分配为零，递归栈空间为 `O(h)`，`h` 是访问到的最大树深度。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/telemetry.go`。两版共同语义是：只将 TiFlash `PhysicalTableReader` 计为命中；只检查其 table plan 根是否为 `ExchangeSender`；遇到非物理计划停止；找到首个 TiFlash reader 后停止遍历。

表示映射如下：Go `base.Plan` 对应 Rust `&PlanNode`；Go `base.PhysicalPlan` 类型断言由 Rust 对 `DataSource | Join` 的排除近似；Go `*physicalop.PhysicalTableReader` 对应 `PlanKind::TableReader`；Go `tableReader.StoreType == kv.TiFlash` 对应 `StoreType::TiFlash`；Go `GetTablePlan().TP() == plancodec.TypeExchangeSender` 对应第一个 child 的 `PlanKind::ExchangeSender`。

两版并非完全等价。Go 明确支持 `nil` 与 `*Explain` 解包，Rust 类型目前无法直接表达顶层空值且没有 Explain 分支。Rust 对“物理计划”的判断也只排除了两个已知逻辑变体，并非 Go interface 断言的完整结构保证。这些差异应在未来接入 Rust 主链前通过模型或适配层解决，而不应在文档中视作已支持。

Go 目录中没有找到 `IsTiFlashContained` 的专门 `*_test.go` 调用。Rust 的直接回归覆盖位于独立文件 `pkg/planner/core/telemetry_test.rs`，符合测试与源文件分离要求。

## 扩展指南

- 新增遥测判断条件时，优先修改 `IsTiFlashContained` 的明确分支，并保持返回二元组的不变量；不要把任意 TiFlash `TableScan` 或嵌套 `ExchangeSender` 自动扩大为当前指标命中，除非 Go 指标口径同时改变。
- 新增逻辑 `PlanKind` 时，应同时审查 `pkg/planner/core/common_plans.rs:PlanNode::IsPhysical` 与本函数的逻辑节点终止集合，避免穿透逻辑计划。更稳健的后续重构方向是复用统一的物理性判定，但必须先验证其语义与 Go `base.PhysicalPlan` 一致。
- 改变 `TableReader` 的 table plan 表示时，应把 `children.first()` 替换为新的显式根访问接口，并验证缺失根、非 Exchange 根和嵌套 Exchange 三种边界。
- 接入 Rust 生产链路时，需要在 Rust 会话语句成功收尾处复现 Go 的非内部会话、全局 telemetry 开关和计数器递增条件；本文件不应自行承担计数器副作用。
- 行为修改必须同步扩展独立测试 `pkg/planner/core/telemetry_test.rs`。现有四项覆盖普通 TiFlash `TableScan` 不计数、直接 Exchange 根计数、嵌套 Exchange 不计数和逻辑节点阻断；建议补充 TiKV `TableReader`、兄弟子树短路、空 children，以及模型支持后补充 Explain/空目标场景。
- 若要与 Go 口径对齐，先以 `pkg/planner/core/telemetry.go` 和其真实上游 `pkg/session/session.go` 为依据；性能上保持单次遍历和无堆分配，兼容性上避免改变两个布尔值的顺序或含义。

## 验证依据

- 源实现：`pkg/planner/core/telemetry.rs`，确认唯一公开函数、三段分支与递归短路。
- 类型与 crate 接线：`pkg/planner/core/common_plans.rs` 中的 `StoreType`、`PlanKind`、`PlanNode`、`PlanNode::IsPhysical`；`pkg/planner/core/lib.rs` 中的 `mod telemetry`、测试模块和 `pub use telemetry::*`；`pkg/planner/core/Cargo.toml` 中的 crate、feature 与依赖声明。
- Rust 独立测试：`pkg/planner/core/telemetry_test.rs` 的 `only_tiflash_table_readers_are_reported`、`exchange_flag_comes_from_tiflash_table_plan_root`、`nested_exchange_is_not_a_table_plan_root`、`logical_nodes_do_not_expose_physical_descendants`。
- Go 对照与应用入口：`pkg/planner/core/telemetry.go:IsTiFlashContained`；`pkg/session/session.go` 中成功语句完成后的 telemetry 计数逻辑。搜索未发现 Go 专门测试调用该函数。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query IsTiFlashContained --kind function` 定位 Rust/Go 两个同名实现；`node IsTiFlashContained` 展示两版源码；`query`/`node` 定位 `common_plans.rs` 的 `PlanNode`、`PlanKind`、`StoreType`；文件限定 `callers` 的跨语言结果经 Rust 原始引用搜索复核后，未作为 Rust 生产接线证据；文件限定 `callees` 未给出静态下游边，因此递归与字段访问以源码为准。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求文档存在且恰好包含本页这 11 个固定二级标题；事实复核同时确认没有把未接线 Rust API、Go 的 Explain/nil 行为或任意 TiFlash 算子写成已支持事实。
