# `pkg/planner/core/operator/physicalop/physical_plan_misc.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate，由 [`lib.rs`](lib.rs) 的 `mod physical_plan_misc` 纳入并通过 `pub use physical_plan_misc::*` 公开。它不是单一算子，而是物理计划层共享的辅助数据与 Runtime Filter 协议：下推 Limit、虚拟列谓词拆分、分区剪枝元数据，以及 HashJoin/TableScan 之间的运行时过滤描述和 `tipb` 编码。crate 边界及其对 `base`、`expression`、`kv`、`parser_ast`、`types`、`tipb` 的依赖由 [`Cargo.toml`](Cargo.toml) 直接确认。

当前 Rust 接线并不均匀：`PushedDownLimit` 和 `PhysPlanPartInfo` 已被多个 reader、缓存快照及内存统计路径使用；`RuntimeFilterListToPB` 已接入 `PhysicalHashJoin::to_pb`；但 `NewRuntimeFilter` 没有 Rust 生产调用者，仓库内也没有 `RuntimeFilterTargetNode` 的实现，所以 Runtime Filter 的“生成并绑定目标扫描”尚未形成与 Go 一致的完整生产链路。

## 核心职责

1. `PushedDownLimit` 保存已压入 reader 的 `Offset`/`Count`，提供值复制和固定结构尺寸核算；Rust 使用点包括 [`physical_indexlookup_reader.rs`](physical_indexlookup_reader.rs)、[`physical_indexmerge_reader.rs`](physical_indexmerge_reader.rs) 与 `base_physical_plan.rs`。
2. `SplitSelCondsWithVirtualColumn` 按 `expression::ContainVirtualColumn` 将条件拆成可继续下推和含虚拟列的两组，并对每个 `ExprBox` 调用 `CloneExpr`，不转移调用方持有的表达式。
3. `PhysPlanPartInfo` 汇集动态分区剪枝所需的谓词、显式分区名、列和列名，提供读取、两种克隆语义及内存核算；`TableScanAndPartitionInfo` 再将该信息与任意 `dyn PhysicalPlan` 扫描计划捆绑。
4. `RuntimeFilterType`、`RuntimeFilterMode`、两个节点适配 trait、`RuntimeFilterIDGenerator` 与 `RuntimeFilter` 定义运行时过滤的创建、节点登记、 EXPLAIN/调试展示、克隆和 protobuf 序列化协议。

## 主要符号

- `PushedDownLimit { Offset, Count }`：两个 `u64` 边界；`Clone() -> Box<Self>` 是装箱值复制，`MemoryUsage()` 返回 `pushedDownLimitSize`。
- `SplitSelCondsWithVirtualColumn(&[ExprBox]) -> (Vec<ExprBox>, Vec<ExprBox>)`：结果顺序与输入顺序一致；同一条件只进入一组。
- `PhysPlanPartInfo`：四个公开字段分别为 `PruningConds`、`PartitionNames`、`Columns`、`ColumnNames`。`GetColumnNames` 与 `CloneForPlanCache` 对 `NameSlice` 做浅复制；普通 `Clone` 为每个非空 `FieldName` 创建新的 `Arc` 内容。
- `TableScanAndPartitionInfo`：`TableScan` 可为空，`PhysPlanPartInfo` 必须存在；`MemoryUsage` 无条件统计分区信息，再按需统计扫描计划。
- `RuntimeFilterType::{In, MinMax}` 与 `RuntimeFilterMode::{Off, Local, Global}`：默认值分别是 `In` 和 `Off`；`Display` 输出协议字符串 `IN`/`MIN_MAX` 与 `OFF`/`LOCAL`/`GLOBAL`。
- `RuntimeFilterBuildNode` / `RuntimeFilterTargetNode`：隔离本文件与具体 HashJoin/TableScan 类型的适配接口。`PhysicalHashJoin` 已实现 build trait；未找到 target trait 的 Rust 实现。
- `RuntimeFilterIDGenerator::New/GetNextID`：返回当前 `i32` 后以 `saturating_add(1)` 前进；达到 `i32::MAX` 后会重复最大值，因而“查询内唯一”依赖正常计划规模不触及饱和边界。
- `NewRuntimeFilter`：从列等值条件中提取左右列，按 build 侧选择源列和目标 `UniqueID`，为 build 节点配置的每种过滤类型分配一条初始为 `Off`、尚未绑定目标的过滤器。
- `RuntimeFilter::{ID, Assign, ExplainInfo, String, Clone, ToPB}`：完成绑定、展示、复制和传输编码；`RuntimeFilterListToPB` 以输入顺序批量编码并在首个错误处返回。

## 执行流程

下推与分区信息路径相互独立。条件拆分时逐项检查虚拟列并克隆到对应列表；规划器可将不含虚拟列的一侧继续交给存储层，把另一侧留在 root。分区路径由规划阶段构造 `PhysPlanPartInfo`，reader 持有它，动态分区展示/访问与计划缓存快照读取这些字段，内存统计递归累计字段内容。

Runtime Filter 的设计流程是：调用方持有一个 `RuntimeFilterIDGenerator`；对 HashJoin 的每个列等值条件调用 `NewRuntimeFilter`；函数依据 `right_is_build_side()` 决定源列和 probe 列唯一 ID，并按 `runtime_filter_types()` 展开为多条过滤器。找到目标扫描后，`Assign` 首次将目标最大等待时间设为 10 秒，写入 build/target 节点 ID 和目标列，并通知两端登记过滤器 ID。EXPLAIN 分别从 build 侧显示 `id[type] <- source`、从目标侧显示 `id[type] -> target`。生成执行 protobuf 时，`PhysicalHashJoin::to_pb` 调用 `RuntimeFilterListToPB`，每条过滤器通过表达式转换器编码源/目标列、执行器 ID、类型和模式。

实际 Rust 主链的边界必须单独理解：`PhysicalHashJoin` 已保存 `RuntimeFilterList`、在 EXPLAIN 中读取它并在 `to_pb` 中编码它，也实现了 `RuntimeFilterBuildNode`；但是其 `register_runtime_filter` 目前不插入列表，且不存在 target trait 实现或 `NewRuntimeFilter` 的生产调用。因此上述生成/分配流程是本文件提供的接口语义，不等于当前 Rust 规划器已经自动走通该流程。`pkg/planner/core/runtime_filter_generator.rs` 另有基于简化 `PlanNode` 的同名模型，不能与本文件的物理算子过滤器类型混为一谈。

## 数据与状态

这些结构均不拥有后台任务或外部句柄。`ExprBox`、`Column`、`CIStr` 和 `NameSlice` 的克隆深度是关键状态约束：`SplitSelCondsWithVirtualColumn` 克隆表达式；`PhysPlanPartInfo::Clone` 深复制列名对象，而 `CloneForPlanCache` 保留列名的共享 `Arc`。相关表达式和列当前通过各自的 `Clone` 实现复制，是否包含更深层共享状态由对应类型定义。

`RuntimeFilter` 的 ID、源/目标表达式、类型、模式与节点 ID 是 protobuf 和 EXPLAIN 的完整本地状态。新建过滤器已有 build 节点 ID，但 `target_node_id` 为 `None`、目标表达式为空、模式为 `Off`；`Assign` 才补齐目标状态。`Clone` 复制表达式容器与所有标量字段，不保留对具体节点对象的引用，因为 Rust 版本只保存节点 ID。

内存核算是估算协议而非 allocator 精确值：`PushedDownLimit` 返回结构体 `size_of`；`PhysPlanPartInfo` 从空结构尺寸出发累加元素报告的内存；`TableScanAndPartitionInfo` 再加物理计划的 `memory_usage()`。`Vec` 自身容量等额外分配是否计入取决于元素及调用者的核算约定，本文件没有单独累计容量。

## 依赖与调用关系

上游方面，`lib.rs` 将所有符号重导出。`physical_indexlookup_reader.rs`、`physical_indexmerge_reader.rs`、`base_physical_plan.rs` 与 `optimizer_runtime.rs` 使用 `PushedDownLimit`；`physical_table_reader.rs`、`physical_index_reader.rs`、`physical_indexlookup_reader.rs`、`physical_utils.rs` 和 `cache_snapshot.rs` 使用 `PhysPlanPartInfo`；`physical_hash_join.rs` 使用 Runtime Filter 类型、build trait、EXPLAIN 与 PB 批量转换。

下游方面，虚拟列拆分调用 `expression::ContainVirtualColumn` 和 `Expression::CloneExpr`；分区信息依赖 `parser_ast::CIStr`、`expression::Column` 与 `types::metadata::NameSlice` 的克隆/内存协议；Runtime Filter 创建调用 `ExtractColumnsFromColOpCol`，展示调用表达式的 `ExplainInfo`/`String`，编码调用 `base::BuildPBContext`、`kv::Client`、`expression::NewPBConverter` 与 `tipb::RuntimeFilter`。

RustCodeGraph 的文件节点将目标文件识别为 53 个符号并显示被 106 个索引文件引用；精确符号查询找到了 Rust/Go 双版本的 `PushedDownLimit`、`SplitSelCondsWithVirtualColumn`、`PhysPlanPartInfo`、`NewRuntimeFilter` 和 `RuntimeFilterListToPB`。图的 `callers/callees` 命令在本次查询时超时且无输出，因此调用边又用限定为 `*.rs`/`*.go` 的仓库搜索复核；不能把“106 个文件”理解为 106 个直接运行时调用者。

## 错误处理与边界

`NewRuntimeFilter` 假设输入是“列 op 列”的等值标量函数；提取任一侧失败会通过 `expect` panic，而不是返回 `Result`。调用者必须先保证谓词形态。空过滤类型列表会正常返回空列表；ID 饱和会导致重复 ID，接口不报告溢出。

`Assign` 允许重复调用，会继续追加目标表达式并再次通知节点；本文件不验证 build/target 是否匹配原始等值条件，也不去重目标列表。目标节点仅在当前过滤器数量为零时设置 10 秒等待时间。由于当前没有 Rust target 实现，这些行为尚缺生产接线验证。

`ToPB` 对每个无法转换的源或目标表达式构造 `expression::Error`，批量转换立即传播首个错误。未绑定目标时，`target_node_id.unwrap_or_default()` 会编码为字符串 `"0"`，而不是拒绝序列化；`Off` 和 `Local` 都编码成 protobuf 的 `Local`。因此调用者应在进入执行编码前完成绑定并设置预期模式。字段访问器不处理空 `self`；Rust 引用规则替代了 Go 方法中的 nil receiver 分支。

## 并发与资源生命周期

本文件没有线程、锁、channel、异步任务或 I/O。生成器和绑定操作要求 `&mut`，由 Rust 借用规则保证同一时刻独占修改；跨线程共享需要调用方额外包装，本文件没有提供同步语义。表达式、字段名和计划对象的生命周期由拥有它们的 `Vec`、`Box`、trait object 与 `Arc` 管理，离开所有者作用域后自动释放。

`TableScanAndPartitionInfo` 独占一个 `Box<PhysPlanPartInfo>`，可选地独占装箱物理计划；`RuntimeFilter` 不持有 build/target 节点引用，只保存整数 ID，因此不会制造节点引用环。计划缓存克隆中的 `NameSlice::Shallow` 会共享 `Arc<FieldName>`，其生命周期延长到最后一个引用释放；普通克隆则隔离该对象身份。

## 与 Go 版本的对应关系

直接对照文件是 [`physical_plan_misc.go`](physical_plan_misc.go)。Rust 保留了 Go 的四组概念、方法命名和大部分顺序语义，但利用所有权消除了 nil receiver，并用 `Box`/`Option`/trait object 表达指针与抽象节点。Rust `TableScanAndPartitionInfo::TableScan` 是 `Option<Box<dyn PhysicalPlan>>`，比 Go 的具体 `*PhysicalTableScan` 更宽；其 Rust 生产使用目前也远少于 Go。

克隆语义需特别注意：Go `CloneForPlanCache` 调用专门的表达式/列缓存克隆函数，而 Rust 当前对 `PruningConds`、`Columns` 使用容器 `clone`，只明确保证 `ColumnNames` 浅共享；普通 `Clone` 则显式深复制列名 `Arc` 内容。独立 Rust 测试验证了这两种列名身份语义和 `CIStr` 内存统计，但没有证明所有表达式内部状态完全等价于 Go 的专用缓存克隆函数。

Go Runtime Filter 由 `runtime_filter_generator.go` 调用 `NewRuntimeFilter` 并执行 `Assign`，具体持有 `*PhysicalHashJoin`/`*PhysicalTableScan`；Rust 改为节点 trait 与整数 ID，但生成/目标登记接线尚不完整。Go 的 `Clone` 在克隆时从仍存活的节点读取执行器 ID；Rust 在创建/绑定时保存 ID，之后直接复制。Rust 的 `GetNextID` 使用饱和加法也是 Go `util.IDGenerator` 普通递增之外的边界差异。两版 PB 映射均将非 Global 模式落为 Local，并在表达式转换失败时返回错误。

## 扩展指南

扩展 Limit 或分区字段时，应同步修改对应克隆、内存核算、计划缓存快照/生成器，并优先扩展独立测试 [`physical_plan_misc_test.rs`](physical_plan_misc_test.rs)；若影响 reader，还应同步 `physical_indexlookup_reader_test.rs`、`physical_indexmerge_reader_test.rs` 或 `cache_snapshot_test.rs`。不要把测试嵌入生产 `.rs` 文件。

补齐 Runtime Filter 生产链路时，最可能修改 `NewRuntimeFilter`、`RuntimeFilter::Assign`、`PhysicalHashJoin` 的 build trait 实现，并在 `PhysicalTableScan` 上实现 `RuntimeFilterTargetNode`；同时要把生成器接到真实物理计划遍历，而不是复用 `runtime_filter_generator.rs` 的简化同名类型而造成类型混淆。必须验证重复登记、首次等待时间、build 侧方向、join 类型限制、Fragment/Global 模式、未绑定过滤器不得编码，以及 HashJoin/Scan 两端 protobuf 和 EXPLAIN 一致性。

增加过滤类型或模式时，需要同步 `Display`、默认值、`NewRuntimeFilter` 的展开、`ToPB` 枚举映射、Go 对照与 tipb 兼容性。新增复合列过滤时不能只放宽 `Vec`：还需确认等值条件聚合规则、列顺序、类型/排序规则、目标匹配及执行端是否支持。性能风险主要是表达式克隆、PB 转换和过滤器列表重复；兼容风险主要是 executor ID、枚举映射与计划缓存克隆语义。

## 验证依据

- RustCodeGraph：执行 `status`（索引 11,467 个文件、307,296 个节点）、`files --filter pkg/planner/core/operator/physicalop`、目标文件 `node --file ... --offset 1 --limit 500`，并对 `PushedDownLimit`、`SplitSelCondsWithVirtualColumn`、`PhysPlanPartInfo`、`TableScanAndPartitionInfo`、`RuntimeFilterIDGenerator`、`NewRuntimeFilter`、`RuntimeFilter`、`RuntimeFilterListToPB` 执行 `query`。`callers/callees` 查询超时且无结果，未将其作为调用边证据。
- 源码与边界：完整读取 [`physical_plan_misc.rs`](physical_plan_misc.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)；目标包目录不存在 `doc.go`，故无额外包约定可读。
- Rust 直接证据：读取/检索 `physical_hash_join.rs`、`physical_table_reader.rs`、`physical_index_reader.rs`、`physical_indexlookup_reader.rs`、`physical_indexmerge_reader.rs`、`physical_utils.rs`、`base_physical_plan.rs`、`cache_snapshot.rs`、`pkg/planner/core/runtime_filter_generator.rs` 与限定范围内的所有精确符号引用。
- 测试证据：完整读取 [`physical_plan_misc_test.rs`](physical_plan_misc_test.rs)，并读取 `foundation_aster_unit_test.rs` 的 Limit、ID 生成、空条件拆分和空分区信息用例；相关 reader/快照测试通过符号搜索定位。按任务要求这是纯文档分析，未运行 Cargo。
- Go 对照：完整读取 [`physical_plan_misc.go`](physical_plan_misc.go)，并用 `runtime_filter_generator.go`、`find_best_task.go`、`task.go`、各 reader/scan 文件及 `runtime_filter_generator_test.go` 的引用核对生产接线和测试入口。
- 结构验收使用任务指定命令，要求本文恰好包含上述 11 个固定二级标题；另以 `git diff --check` 和人工复核检查链接、事实限定、迁移缺口及扩展建议。
