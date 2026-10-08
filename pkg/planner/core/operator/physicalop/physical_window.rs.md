# `pkg/planner/core/operator/physicalop/physical_window.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate（见同目录 `Cargo.toml`），实现逻辑窗口向物理窗口的枚举、窗口算子的公共物理计划行为、TiFlash protobuf 下推，以及 TiDB Root 窗口并行化所需的 Shuffle 边界。模块由 `lib.rs` 的 `mod physical_window` 装配并通过 `pub use physical_window::*` 导出；`lib.rs` 中的 `ConcretePhysicalOperator` 实现再把 `ExplainInfo`、`ResolveIndices`、内存统计、相关列抽取、成本计算和 `ToPB` 接入统一 `PhysicalPlan` 接口。

它位于 SQL 规划主链的逻辑/物理分界处：上游是 `logicalop::LogicalWindow` 与父节点要求的 `property::PhysicalProperty`，核心枚举入口是 `ExhaustPhysicalPlans4LogicalWindow`；下游要么形成 TiDB Root `PhysicalWindow`，要么形成 `StoreTp == TiFlash` 的 MPP 窗口并由物理计划任务挂接及 protobuf 构造继续处理。Root 计划的后优化还可能由 `pkg/planner/core/optimizer_runtime.rs::optimize_by_shuffle_for_window` 包装成 `PhysicalShuffle`。

## 核心职责

1. `PhysicalWindow` 保存窗口函数描述、`PARTITION BY`、`ORDER BY`、窗框及执行存储类型，并实现构造、克隆、Explain、列下标解析、相关列抽取、内存估算、成本委托、任务挂接和 TiFlash PB 序列化。
2. `ExhaustPhysicalPlans4LogicalWindow` 根据会话 MPP 开关、TiFlash 可用性、表达式限制、排序/分区属性和父任务类型，枚举 TiFlash MPP 或 TiDB Root 窗口候选。
3. `PhysicalShuffle` 表示 Root 窗口并行执行的主线程边界，保存 worker 数、实际数据源、每个源的分区键及 splitter 类型；`PhysicalShuffleReceiverStub` 表示 worker 侧接收占位节点。
4. `frame_bound_to_pb` 和 `contains_virtual_window_expression` 分别承担窗框 PB 转换、TiFlash 下推前的虚拟列/相关列拒绝检查。

本文件只描述和构造计划，不执行窗口函数。实际窗口执行器在 `pkg/executor/windows/`；因此 `WindowFuncDescs` 是规划期元数据，而不是运行期累积状态。

## 主要符号

- `PhysicalWindow`：输出 Schema 由 `PhysicalSchemaProducer` 管理；`WindowFuncDescs` 是窗口函数及参数；`PartitionBy`/`OrderBy` 是有序属性；`Frame` 可为空；`StoreTp` 默认为 `kv::StoreType::TiDB`，MPP 候选显式改为 `TiFlash`。
- `PhysicalWindow::New`：分配 `TypeWindow` 计划 ID 并建立空窗口。`Init` 只补充上下文、统计、查询块偏移和唯一子属性要求，不重新分配 ID；独立测试 `window_and_shuffle_initialization_preserve_allocated_ids` 固化这一不变量。
- `PhysicalWindow::Clone`：克隆公共计划状态、Schema、函数描述、排序项、窗框和存储类型；表达式错误通过 `Result` 向上传播。
- `PhysicalWindow::ExplainInfo` / `format_bound`：输出函数到结果列的映射以及 `OVER(partition/order/frame)`；普通数值窗框服从日志脱敏模式，细粒度 Shuffle 开启时附加 `stream_count`。
- `PhysicalWindow::ResolveIndices`：以唯一 child Schema 为基准解析透传输出列、分区/排序列、窗口参数与起止窗框计算表达式。窗口函数新增的尾部结果列不应当解析为 child 列。
- `PhysicalWindow::ToPB`：重建并验证窗口函数描述，将函数、排序项、窗框和 child 转换成 `tipb::Window`，并写入 executor ID、细粒度 stream count 与 batch size。
- `PhysicalShuffle`：`Init` 继承 child Schema/输出名并挂接单 child；`ResolveIndices` 要求 `DataSources.len() == ByItemArrays.len()`，再逐源解析分区键；`Clone` 深克隆实际数据源和表达式。
- `PhysicalShuffleReceiverStub`：保存可选的真实数据源，构造时继承其 Schema；成本计算仍委托公共基类。
- `ExhaustPhysicalPlans4LogicalWindow`：公开的逻辑到物理窗口候选枚举函数；`base_physical_plan.rs` 的逻辑算子分派表引用它。

## 执行流程

`ExhaustPhysicalPlans4LogicalWindow` 的主流程如下：

1. 从 `LogicalWindow::SCtx` 取得计划上下文；不存在上下文时返回空候选。
2. 若会话允许 MPP 且应检查 TiFlash 下推，则继续验证比较语义、虚拟/相关表达式、父排序项是否全用于分区、任务类型是否为 Root/MPP，以及父分区要求不是 Broadcast。
3. 构造 MPP child 属性：无限期望行数、允许 enforcer、排序键为分区键后接窗口排序键、任务类型为 MPP，并继承 CTE producer 状态。
4. 只有父属性是该 child 属性的前缀时才继续。按 `LogicalWindow::GetPartitionKeys` 构造带 collation ID 的 MPP 分区列；父要求 Hash 时选择匹配键，无分区键时要求 SinglePartition。无法满足父分区要求则不生成候选。
5. 合法时复制逻辑窗口字段、缩放统计、复制 Schema，将 `StoreTp` 设为 TiFlash，并加入候选。
6. 父只接受 MPP，或强制 MPP 且已有合法候选时立即返回；否则构造 Root child 属性，仍要求输入按 `PartitionBy + OrderBy` 排序，并继承 CTE/禁止 Cop 下推标志。
7. 父属性不是 Root child 属性前缀时返回空；否则生成默认 TiDB `PhysicalWindow` 候选。

候选进入后续任务挂接时，`base_physical_plan.rs` 会为 TiFlash 窗口检查 MPP 分区是否满足；必要时插入/整理 Exchange，再克隆窗口并挂上输入。Root 后优化的 `optimize_by_shuffle_for_window` 仅在并发度大于 1、外部属性为空、窗口有分区键、直接 child 是 Sort 且数据源估计行数大于 1 时插入 `PhysicalShuffle`，并将分区列作为 Hash splitter 键。

PB 路径中，`ToPB` 先要求 `BuildPBContext` 有 client，然后逐一校验并编码函数、分区键、排序键和窗框，递归编码唯一 child，最后形成 `ExecType::TypeWindow` executor。

## 数据与状态

`PhysicalWindow` 的持久规划状态包括公共 Schema/统计/child/child required property、窗口描述、排序属性、窗框和存储位置。其关键不变量是：输入必须满足 `PartitionBy` 后接 `OrderBy` 的排序；输出 Schema 前缀来自 child，尾部列与 `WindowFuncDescs` 一一对应；下推 PB 时必须有且只有规划所需的首个 child。

MPP 分区状态存在 `PhysicalProperty` 中：有分区键时优先为 Hash，没有分区键时为 SinglePartition；每个 `MPPPartitionColumn` 同时保存列和 collation ID。父要求 Hash 时，通过 `IsSubsetOf`/`ChoosePartitionKeys` 缩小到父可接受的键集合。

`PhysicalShuffle` 同时保存展示用 `DataSourceExplainIDs` 与可操作的 `DataSources`。后者不是普通 flat children，而是 splitter 的输入来源；`ByItemArrays[i]` 必须对应 `DataSources[i]`。`Concurrency` 在后优化时取配置并发度与数据源估计行数的较小值。`PhysicalShuffleReceiverStub::DataSource` 同样不通过普通 child 表达。

本文件没有全局可变状态。计划 ID、会话变量、表达式上下文和 PB client 由 `ContextRef`/`BuildPBContext` 注入。

## 依赖与调用关系

上游依赖包括 `base::{Plan, PhysicalPlan, Task}`、`logicalop::LogicalWindow`、`property::{PhysicalProperty, SortItem, MPPPartitionColumn}` 和会话 `PlanContext`。RustCodeGraph 显示 `ExhaustPhysicalPlans4LogicalWindow` 由 `base_physical_plan.rs` 的逻辑到物理分派表接入；级联优化的 `pkg/planner/cascades/old/implementation_rules.rs` 也直接构造 `PhysicalWindow`。

下游主要依赖：

- `expression`：相关列/虚拟列检查、下标解析、Explain、函数和排序项 PB 转换、错误类型。
- `aggregation`：重建 `WindowFuncDesc` 并生成 PB 表达式。
- `tipb`：`Window`、`WindowFrame`、边界枚举和最终 `Executor`。
- `planner_util::ShouldCheckTiFlashPushDown`：决定是否尝试 TiFlash 候选。
- `BasePhysicalPlan`/`PhysicalSchemaProducer`：统一 child、Schema、统计、成本和任务挂接。

下游消费者包括 `pkg/planner/core/task.rs`/`base_physical_plan.rs` 的任务挂接、`optimizer_runtime.rs` 的窗口 Shuffle 与细粒度 Shuffle 标记、`pkg/executor/statement_ru_plan_walk.rs` 的 RU 表达式计数，以及执行器 builder 对计划的运行期物化。`Cargo.toml` 证明本 crate 对 `base`、`costusage`、`expression`、`aggregation`、`kv`、`logicalop`、`property`、`planner_util`、`plancodec`、`tipb` 等使用显式依赖，且没有为本文件声明条件 feature。

## 错误处理与边界

- 候选枚举对缺少上下文、不兼容排序前缀、Broadcast、无法匹配 Hash 分区键、SinglePartition 冲突、虚拟/相关表达式或不适合 MPP 的任务类型采用“返回空/跳过 MPP 候选”，而不是制造不可执行计划。
- `ResolveIndices` 没有 child 时返回 `Ok(())`；有 child 时任何列或表达式解析失败都会立即传播。Shuffle 的源与键数组数量不同则返回明确错误 `shuffle data sources and partition keys differ`。
- `ToPB` 对缺少 PB client、无效窗口函数、函数/分区/排序项不可下推、缺少 child，以及表达式/PB 转换错误均返回 `expression::Error`；它不会发送 KV 请求。
- `format_bound` 对 `CurrentRow`、无界、带计算函数和普通数值分别处理。当前 Rust 分支对带计算函数的展示主要取首个标量函数的第二个参数；Go 版本还按 `DateAdd/DateSub` 与 `Plus/Minus` 区分格式，因此扩展 Explain 时要同步核对两端输出。
- `MemoryUsage` 是估算值：窗口侧统计描述名称/参数与排序项，Shuffle 侧统计 ID、数据源和分区表达式；不要把它视为 allocator 的精确实时占用。

## 并发与资源生命周期

本文件没有启动线程、异步任务、锁或通道。`ContextRef` 及 boxed trait object 负责共享上下文和计划对象所有权；克隆计划时通过 `clone_physical`/`CloneExpr` 创建独立计划树和表达式，测试还确认 Shuffle 克隆后的数据源不是原 trait object 的同一地址。

并发语义由计划元数据表达：`PhysicalShuffle::Concurrency` 决定 Root 窗口 worker 数，`SplitterType` 与 `ByItemArrays` 决定数据分发；`TiFlashFineGrainedShuffleStreamCount` 和 PB context 的 batch size 传给 TiFlash。`PhysicalShuffleReceiverStub` 将实际数据源留在普通 `Children()` 之外，保持 Go 的 worker-side receiver 拓扑语义。执行期资源分配、chunk 生命周期和关闭行为属于 `pkg/executor/windows/`，不由本文件管理。

## 与 Go 版本的对应关系

Go 主对照是同目录 `physical_window.go`；Shuffle 对照是 `physical_shuffle.go`，插入入口是 `pkg/planner/core/plan.go::optimizeByShuffle4Window`。字段与主要方法保持一一对应：`PhysicalWindow`、`Init`、`Clone`、`ExtractCorrelatedCols`、`MemoryUsage`、`ExplainInfo`、`ResolveIndices`、`Attach2Task`、`ToPB` 和 `ExhaustPhysicalPlans4LogicalWindow` 均有对应实现；Rust 将 Go 指针/切片改为 `Option`/`Vec`/trait object，并用 `Result` 显式传播错误。

Rust 把 Go `tryToGetMppWindows` 的核心分支内联到 `ExhaustPhysicalPlans4LogicalWindow`，并通过 `contains_virtual_window_expression` 明确拒绝窗口参数、排序项和窗框中的虚拟列/相关列。Go 当前实现还会逐函数检查 `CanPushDownToTiFlash`、表达式下推黑名单及 RANGE frame 表达式 PB/下推能力，并在强制 MPP 时产生警告；Rust 枚举处可见的检查集合不同，部分错误推迟到 `ToPB`。因此新增下推能力或拒绝规则时，必须同时核对 Go 的预检查、Rust 枚举条件和 Rust `ToPB`，不能只以“最终序列化会失败”替代候选阶段语义。

Rust 的 `PhysicalShuffle`/receiver 与窗口实现同文件，而 Go 独立放在 `physical_shuffle.go`。Rust 还保存 typed `DataSources` 和 `ByItemArrays`，供列解析和 statement-RU splitter 公式使用；仅 ExplainID 不足以恢复这些信息。

## 扩展指南

- 新增窗口函数或 TiFlash 下推规则：优先检查 `ExhaustPhysicalPlans4LogicalWindow` 的候选门槛、`contains_virtual_window_expression`、`ToPB` 的描述重建/编码及 Go `tryToGetMppWindows`；同步扩展独立 Rust 测试，覆盖“生成候选”和“拒绝候选”两面。
- 新增窗框类型/边界：同步修改 `format_bound`、`frame_bound_to_pb`、`ExtractCorrelatedCols`、`ResolveIndices` 和虚拟/相关表达式检查，并核对 `logicalop::WindowFrame` 与 tipb 枚举兼容性。
- 改变输出 Schema：保持“child 透传列在前、窗口结果列在后”的约定，并扩展 `resolve_indices_updates_passthrough_schema_columns_like_go`，防止错误解析新产生的窗口列。
- 改变 Shuffle 拓扑：维护 `DataSources` 与 `ByItemArrays` 等长不变量，核对 `optimizer_runtime.rs::optimize_by_shuffle_for_window`、RU 遍历和 Clone 深拷贝；相关测试应放在独立的 `physical_window_test.rs`、规划器集成测试或 RU 测试中，不要内嵌到生产文件。
- 改变 Explain 或日志值输出：覆盖关闭、标记、完全脱敏三种模式，并与 Go golden 输出兼容；特别注意 interval RANGE 边界格式。
- 性能风险集中在额外排序/Exchange、错误分区导致的数据倾斜、过高 Shuffle 并发和重复深克隆；兼容风险集中在 Explain 文本、PB 字段、collation ID 和 MPP 强制模式的候选选择。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file ...physical_window.rs` 阅读完整 892 行；`query PhysicalWindow`、`query ExhaustPhysicalPlans4LogicalWindow`、`node`/`explore` 确认符号、分派入口及 `base_physical_plan.rs`、`optimizer_runtime.rs` 等调用关系。
- Rust 源与装配：`pkg/planner/core/operator/physicalop/physical_window.rs`、`lib.rs`、`base_physical_plan.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/task.rs`、`pkg/executor/statement_ru_plan_walk.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`，包名为 `astersql-planner-core-operator-physicalop`，`lib.rs` 为库入口，`autotests = false`，测试由模块显式装配。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_window.go`、`physical_shuffle.go` 和 `pkg/planner/core/plan.go`。
- 独立 Rust 测试：`physical_window_test.rs` 覆盖 ID 保持、TiFlash PB executor ID、透传 Schema 下标解析、Shuffle 深克隆/分区键解析/长度错误；更高层 MPP 与窗口计划覆盖见 `pkg/planner/core/enforce_mpp_test.rs`、`integration_test.rs`、`plan_test.rs` 和 `pkg/executor/statement_ru_plan_walk_test.rs`。Go 相关回归入口包括 `pkg/planner/core/plan_test.go`、`explain_ru_test.go` 与 `casetest/windows/window_push_down_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务指定的 11 章节结构命令，并人工核对本文可回答文件定位、运行路径、状态不变量、失败边界和安全扩展入口。
