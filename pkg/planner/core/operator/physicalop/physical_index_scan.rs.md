# `pkg/planner/core/operator/physicalop/physical_index_scan.rs`

## 文件定位

本文件定义 Rust 物理计划层的索引扫描叶子算子 `PhysicalIndexScan`。它位于
`astersql-planner-core-operator-physicalop` crate；`Cargo.toml` 将 crate 根指定为
`lib.rs`，后者通过 `mod physical_index_scan` 装配并以
`pub use physical_index_scan::*` 对外导出。`direct_operator_core!(PhysicalIndexScan,
PhysicalSchemaProducer)` 又为该类型接入统一的物理算子/计划接口，因此它能作为
`Box<dyn PhysicalPlan>` 出现在 reader 的子计划中。

在完整链路中，它承接逻辑索引扫描或访问路径选择的结果，保存表、索引、范围、输出
Schema 和顺序属性；随后由 `PhysicalIndexReader`、`PhysicalIndexLookUpReader` 或
Index Merge 计划包裹。执行侧读取这些元数据生成 KV 索引范围或 `tipb::IndexScan`。
它描述“扫描什么、按什么范围和顺序扫描”，自身不执行 KV I/O。

## 核心职责

- `PhysicalIndexScan` 集中保存索引扫描的计划状态：`Table`/`Index` 元数据、
  `IdxCols`/`IdxColLens`、`Ranges`、访问条件和残余过滤条件，以及分区、顺序、回表和
  common handle 标志。
- `InitSchema` 构造存储侧索引扫描的输出 Schema，补齐声明索引列、整数句柄或 common
  handle 后缀，并在全局索引场景加入物理表 ID。该 Schema 可能比 TiDB reader 最终需要
  的列更多。
- `AccessObject`、`TP`、`OperatorInfo`、`ExplainInfo` 和
  `ExplainNormalizedInfo` 生成普通或归一化计划展示，并区分全索引扫描与范围扫描。
- `build_ranges_with_quota` 及四个公开的计划缓存辅助方法用当前表达式上下文重建范围、
  检查配额回退并生成缓存诊断信息。
- `ToPB` 把计划元数据编码为 `tipb::Executor` 中的 `IndexScan`；代价、下标解析和内存
  估算则委托或汇总到物理计划公共层。

## 主要符号

- `PhysicalIndexScan`：唯一的公开结构体。基座字段
  `PhysicalSchemaProducer` 提供上下文、计划 ID、Schema、统计信息和公共计划行为。
  `AccessCondition` 用于构造索引范围；`FilterCondition` 是扫描后的残余过滤，并在缓存
  范围重建时与访问条件合并，以恢复因配额回退而移出的后缀谓词。
- `New(ctx)` / `Init(ctx, offset)`：前者创建带 `TypeIdxScan` 的空节点，后者重新绑定上下文
  和 query-block offset。元数据字段在构造后由物理化路径补入。
- `Clone(new_ctx)`：克隆计划并切换上下文；表达式、Schema、表/索引信息、列和范围均按
  各自 `Clone` 语义复制。返回 `expression::Error`，因为基座克隆可能失败。
- `ExtractCorrelatedCols()`：只从 `AccessCondition` 提取相关列；不会检查
  `FilterCondition`，这与 Go 实现和独立测试保持一致。
- `InitSchema(index_columns, double_read)`：复用已存在的 `IdxCols`，补齐索引列；处理整数
  handle、common handle、全局索引的 `ExtraPhysTblID`，最后设置输出 Schema。
- `AccessObject()` / `access_object(normalized)`：输出表别名、分区名和索引列；隐藏索引列
  显示生成表达式，归一化输出以 `?` 隐藏具体分区名。表元数据缺失时返回
  `table:unknown`。
- `IsFullScan()` / `TP()`：只有在没有 `RangeInfo`、没有相关访问条件，且范围为空或全部
  为 full range 时才判定为 `IndexFullScan`，否则为 `IndexRangeScan`。
- `OperatorInfo(normalized)`：按优先级展示 `RangeInfo`、相关列决定的范围、静态范围，最后
  附加 `KeepOrder` 和 `Desc`。归一化形式隐藏具体值。
- `RebuildRangesForPlanCache()`、`PlanCacheRangeString()`、
  `PlanCacheRangesFitQuota(range_max_size)`、`PlanCacheTP()`：用完整谓词集合和零配额重建
  可复用范围，并与受限配额结果的 `Range::String` 串比较。这里刻意不用将范围还原成 SQL
  谓词的 `RangesToString`。
- `NeedExtraOutputCol()` / `IsPartitionTable()` / `IsPointGetByUniqueKey(ctx)`：分别报告全局
  分区索引是否需要物理表 ID、当前分区身份，以及唯一索引单个非空点范围能否视为
  point get。
- `AddSelectionConditionForGlobalIndex(conditions)`：当前 Rust 版本只是全局索引前置条件
  检查；非全局索引原样返回条件，全局索引缺少额外输出列时报错，否则也原样返回。
- `ToPB(ctx, store)`：要求 `Table`、`Index` 和 Schema 都存在，设置物理/逻辑表 ID、索引
  ID、唯一性、列 protobuf 元数据和方向，生成 `TypeIndexScan` executor。
- `GetPhysicalIndexScan4LogicalIndexScan(ctx, schema, stats)`：创建只带 Schema 和统计信息的
  基础物理节点。调用者必须继续填充表、索引、列、范围等字段。
- `GetOriginalPhysicalIndexScan(ctx, property)`：只从属性初始化 `Desc`、`KeepOrder` 和
  `Prop`，并非 Go 同名函数的完整 DataSource/AccessPath 转换。
- `ConvertToPartialIndexScan(scan, conditions)`：当前是原样返回二元组的占位门面，尚未
  构造 Selection 或执行全局索引分区过滤。

## 执行流程

1. 物理化阶段创建节点。旧 cascades 规则 `ImplIndexScan::OnImplement` 调用
   `GetPhysicalIndexScan4LogicalIndexScan`，随后填入 `Table`、`Index`、`Columns`、
   `Ranges` 及顺序标志；传统路径在 `base_physical_plan.rs` 中也从
   `LogicalIndexScan` 构造并补入访问条件和范围。
2. 构造路径必须在使用前补齐扫描元数据并设置 Schema。需要回表时，`InitSchema` 保证
   第一阶段索引扫描输出 handle；common handle 保留完整物理后缀；全局索引还携带
   `ExtraPhysTblID`。
3. 计划展示调用 `TP` 和 `OperatorInfo`。范围信息优先展示明确的 `RangeInfo`，其次是
   相关访问条件，最后才是已经物化的 `Ranges`；归一化路径屏蔽参数和分区值。
4. prepared-plan/cache 路径不能盲用生成计划时的 `Ranges`。会话层
   `collect_process_plan_snapshot` 调用 `PlanCacheTP`/`PlanCacheRangeString`，配额准入遍历
   计划树调用 `PlanCacheRangesFitQuota`；typed adapter 也用
   `RebuildRangesForPlanCache` 编码当前执行参数对应的 KV key ranges。
5. 下推到存储时，`ToPB` 从 Schema 逐列查回 `ColumnInfo`，为额外句柄列构造特殊元数据，
   判断唯一范围是否覆盖索引全部列且不含 NULL，再生成 `tipb::IndexScan`。
6. 执行构建阶段从 reader 子树找到本节点。`pkg/executor/builder.rs` 校验表/索引信息和
   covering index 列，创建 typed index reader 或 index lookup；残余
   `FilterCondition` 会被包装成 typed selection。

## 数据与状态

结构体可分成五组状态：

- 计划基座：`PhysicalSchemaProducer`、`Prop` 和统计/代价入口。
- 谓词与范围：`AccessCondition`、`FilterCondition`、`Ranges`、`RangeInfo`、
  `ConstColsByCond`。缓存重建必须同时读取两类条件，这是范围配额回退后的关键不变量。
- 元数据与列：`Table`、`Index`、`IdxCols`、`IdxColLens`、`Columns`、
  `DataSourceSchema`、`PKIsHandleCol` 和 `TblColHists`。
- 身份与展示：`DBName`、`TableAsName`、`PhysicalTableID`、`IsPartition`。
- 执行属性：`Desc`、`KeepOrder`、`DoubleRead`、`NeedCommonHandle`。

`InitSchema` 的核心不变量是：已有 `IdxCols` 的对象身份/唯一 ID 不被替换；声明索引列
不足时才从传入列或表元数据补齐；double read 必须有可供回表的 handle；全局索引必须
有物理表 ID。common handle 的后缀允许与声明索引列出现相同列 ID，因为执行器按“声明
索引列之后”的固定偏移读取句柄。

`MemoryUsage` 是近似汇总：公共 producer、访问条件、索引列、ranges 和三个字符串被
计算；`FilterCondition`、表/索引元数据、若干可选字段等未计入，因此不能视为完整堆
占用。`GetScanRowSize` 同样是 Rust 当前的简化估算（Schema 列数乘 8）。

## 依赖与调用关系

上游主要调用关系：

- `pkg/planner/cascades/old/implementation_rules.rs` 的 `ImplIndexScan::OnImplement`：
  `LogicalIndexScan -> GetPhysicalIndexScan4LogicalIndexScan -> NewIndexScanImpl`。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：传统物理化和访问路径逻辑
  创建、填充 `PhysicalIndexScan`，并在多个 reader/Index Join 路径中查找它。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs`：为 Index Join probe 构建索引
  扫描。

下游主要调用关系：

- `physical_index_reader.rs`、`physical_indexmerge_reader.rs` 和 lookup reader 将扫描放入
  reader 子计划，读取 `AccessObject` 或按类型下转。
- `pkg/session/runtime/planning.rs` 读取缓存重建后的算子类型、范围字符串及配额适配结果；
  `typed_adapter_bridge.rs` 将重建范围编码成索引 seek key ranges。
- `pkg/executor/builder.rs` 读取表、索引、列、方向和残余过滤条件，构建 typed 执行器；
  `statement_ru_plan_walk.rs` 将它识别为扫描节点参与 RU 计划遍历。
- `cache_snapshot.rs` 捕获和恢复所有 Rust 结构体字段，使计划缓存快照不会依赖原对象生命
  周期。

直接 crate 依赖可由 `physicalop/Cargo.toml` 复核：`base`/`property`/`costusage` 提供计划
接口、属性和代价，`expression` 提供表达式与 Schema，`model` 提供表/索引元数据，
`ranger` 构造范围，`kv` 提供 store 类型，`tipb` 提供下推协议，`types`/`mysql`/
`plancodec` 提供值、标志和算子编码。

## 错误处理与边界

- `Clone`、范围重建、字符串格式化、PB 编码、代价和下标解析统一返回
  `expression::Error`，调用者以 `?` 传播。`ranges_to_string` 当前没有自身失败分支，但
  保留 `Result` 以匹配上层链路。
- `build_ranges_with_quota` 调用 `DetachCondAndBuildRangeForIndex`；表达式求值或范围构造
  失败会直接中止缓存诊断/准入，而不是退化成旧范围。
- `PlanCacheRangesFitQuota(0)` 明确返回 `true`；非零配额通过有限范围和完整范围字符串的
  精确相等判断是否发生回退。
- `ToPB` 对缺失 `Table`、`Index`、Schema 或 Schema 列无法映射回表列均返回明确错误。
  唯一性只有在 unique index 的每个范围都覆盖全部索引列且上下界无 NULL 时才下推。
- `AccessObject` 对缺表信息容错为 `table:unknown`；但索引列 offset 会直接索引
  `table.Columns`，因此调用前仍要求 `IndexInfo.Columns` 与表元数据一致。
- `InitSchema` 对越界/缺失的可选输入采取跳过策略；若关键元数据尚未设置，只会生成
  当前 `IdxCols` 的 Schema。构造方应满足注释列出的初始化前置条件。
- `AddSelectionConditionForGlobalIndex` 目前不生成 Go 版本的 partition-pruning 表达式；
  `ConvertToPartialIndexScan` 也只是占位。这些限制会影响声称“完整支持全局索引 Index
  Merge”的正确性，扩展时必须补实现和测试，不能依赖现有返回值推断已完成。

## 并发与资源生命周期

该文件没有锁、线程、异步任务或通道。节点通常在优化器阶段由单个所有者构造，再通过
`Box<dyn PhysicalPlan>` 进入计划树；可变方法要求 `&mut self`，只读展示/编码方法要求
`&self`。共享上下文采用 `ContextRef`，直方图采用 `HistCollRef`，其具体共享生命周期由
对应引用类型管理。

`Clone(new_ctx)` 明确建立可独立修改的表达式、Schema、列和范围副本，同时按类型的
`Clone` 语义共享或复制元数据。计划缓存另有 `CachedIndexScan::capture/restore`，把表达式
和列转换为缓存安全表示，并在新 `ContextRef` 下恢复；修改字段时必须同步这套快照结构，
否则缓存恢复会丢状态。`AllocPlanColumnID` 只在补列时从 session vars 分配标识，要求有效
上下文存在。

## 与 Go 版本的对应关系

直接对照文件是同目录 `physical_index_scan.go`。Rust 已对应的主干包括结构体核心字段、
克隆、相关列提取、Schema 初始化、访问对象/EXPLAIN、全范围判定、point-get 判定、PB
编码和两个代价版本的入口。独立 Rust 测试特别锁定了 Go 语义：空范围展示、只从访问
条件提取相关列、别名/分区/隐藏索引列展示、PB 列元数据、预建索引列保持以及 common
handle 后缀布局。

仍存在必须显式记录的迁移差异：

- Go 结构体还有 `GenExprs`、`ByItems`、`UsedStatsInfo`、`GroupedRanges` 和
  `GroupByColIdxs`；Rust 本结构体没有这些字段。
- Go `GetOriginalPhysicalIndexScan` 接收 `DataSource`、`AccessPath` 和属性，负责范围、
  统计缩放、顺序与 Schema 的完整组装；Rust 同名函数只初始化属性相关字段。
- Go `ConvertToPartialIndexScan` 会构建 partial scan、处理动态分区/分组范围、全局索引
  Selection 及剩余过滤；Rust 当前原样返回输入。
- Go `AddSelectionConditionForGlobalIndex` 运行 partition pruning 并生成 PID 的 `IN`/`NOT
  IN` 条件；Rust 只验证是否具备物理表 ID 输出列。
- Go `NeedExtraOutputCol` 还覆盖动态分区裁剪加嵌入 limit 的 `ByItems` 场景；Rust 只覆盖
  分区表全局索引。
- Go `GetScanRowSize` 使用直方图、索引唯一性和列类型估算平均行宽；Rust 只是列数乘 8。
- Go PB 会填 common handle 的 primary column IDs；当前 Rust PB 编码未设置等价字段。
- Rust 额外包含为 prepared plan/typed execution 服务的范围重建和配额一致性方法；这些
  方法依赖 `FilterCondition` 保留因配额回退移出的条件。

因此本文件是可用的 Rust 索引扫描核心，但不是 Go 文件全部能力的逐项等价实现；上述
差异应作为扩展边界，而不是从名称相同推导行为相同。

## 扩展指南

- 新增或改变扫描状态时，先修改 `PhysicalIndexScan`，并同步 `New`、`Clone`、
  `MemoryUsage`（若需要计量）、`cache_snapshot.rs` 的 `CachedIndexScan` 捕获/恢复，以及
  `lib.rs` 中的缓存契约说明。状态影响下推时还要同步 `ToPB`。
- 修改 Schema/handle 布局应从 `InitSchema` 接入，并扩展独立测试
  `physical_index_scan_test.rs`；重点覆盖普通整数 handle、common handle、覆盖/非覆盖
  索引、double read、全局索引和已存在 `ExtraPhysTblID`，不要把 Rust 测试嵌入源文件。
- 修改范围或计划缓存行为应同时检查 `build_ranges_with_quota`、四个公开缓存方法、
  `pkg/session/runtime/planning.rs` 和 `typed_adapter_bridge.rs`。必须保持完整谓词集合和
  quota=0 重建不变量，并测试参数变宽、范围配额回退、NULL 和 full range。
- 完善全局索引或 partial Index Merge 时，应以 Go 的
  `AddSelectionConditionForGlobalIndex`/`ConvertToPartialIndexScan` 为行为基准，在 Rust 中
  接入真实 partition pruning 和 Selection 构造，而非扩写当前占位返回；同步新增独立
  测试及 reader/executor 集成测试。
- 调整 EXPLAIN 时同步普通与 normalized 分支，确保敏感值、分区名和 ranges 被正确隐藏；
  `physical_index_scan_test.rs` 是最近的单元测试入口，计划缓存的端到端边界还可在
  `pkg/planner/core/integration_test.rs` 中验证。
- 性能风险主要在范围数量、表达式重复克隆、缓存重建和 Schema 列膨胀；兼容风险主要在
  EXPLAIN 字符串、PB 列顺序/唯一标志、common handle 偏移及 Go/Rust 计划行为差异。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `node --file ...physical_index_scan.rs` 读取了完整 674 行源码，并报告该文件被 16 个
  文件使用。对精确符号执行的 `query/callers/callees` 未返回可用 Rust 调用边，因此按
  技能规则用 `rg` 补查具体引用。
- 源码事实：`physical_index_scan.rs` 中的 `PhysicalIndexScan`、`InitSchema`、
  `build_ranges_with_quota`、`ToPB`、三个构造/转换函数；`lib.rs` 中的模块导出、
  `direct_operator_core!` 与 cache contract。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 的 `[package]`、`[lib]`、
  `[dependencies]` 和 `package.metadata.porting`。
- 上下游证据：`implementation_rules.rs::ImplIndexScan::OnImplement`、
  `base_physical_plan.rs` 的逻辑索引扫描物理化路径、`physical_index_reader.rs`、
  `pkg/session/runtime/planning.rs`、`typed_adapter_bridge.rs`、
  `pkg/executor/builder.rs`、`cache_snapshot.rs::CachedIndexScan`。
- Go 对照：完整读取同目录 `physical_index_scan.go` 的 757 行，逐项核对结构体、Schema、
  EXPLAIN、全局索引、PB、代价与构造/partial conversion 行为。
- 测试依据：完整读取同目录独立测试 `physical_index_scan_test.rs` 的 298 行；另由引用搜索
  确认 `pkg/planner/core/integration_test.rs` 覆盖缓存范围字符串和 IndexScan 计划集成。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令验证目标文件存在且恰有
  11 个固定二级章节，并人工复核没有把 Go 独有逻辑或 Rust 占位函数写成已支持能力。
