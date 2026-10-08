# `pkg/planner/core/operator/physicalop/physical_table_scan.rs`

## 文件定位

本文件定义 Rust 物理计划层的表扫描叶子算子 `PhysicalTableScan`，所属 crate 是
`astersql-planner-core-operator-physicalop`。同目录 `Cargo.toml` 指定 `lib.rs` 为 crate
入口；`lib.rs` 以 `mod physical_table_scan` 装配本模块、通过
`pub use physical_table_scan::*` 导出符号，并用
`direct_operator_core!(PhysicalTableScan, PhysicalSchemaProducer)` 把它接入统一的
`PhysicalPlan`/具体物理算子接口。

该节点承接逻辑表扫描和访问路径选择产生的表、列、范围、条件、统计及顺序属性，随后被
`PhysicalTableReader`、`PhysicalIndexLookUpReader`、MPP fragment 或 typed 执行路径消费。
它描述“扫描哪个物理表、读取哪些列、按什么范围和顺序读取”，本文件本身不打开 KV
迭代器；实际 KV I/O 可见 `pkg/executor/physical_plan_runtime.rs` 和
`pkg/session/runtime/typed_adapter_bridge.rs`。

## 核心职责

- `PhysicalTableScan` 保存表扫描计划状态，包括逻辑表元数据、物理分区 ID、输出列、
  `Ranges`、访问/残余/延迟物化条件、TiKV/TiFlash 类型及顺序属性。
- `AccessObject`、`TP`、`OperatorInfo`、`ExplainInfo` 和
  `ExplainNormalizedInfo` 生成用户可见或 digest 使用的扫描说明，并隐藏归一化输出中的
  具体范围与分区名。
- `RebuildRangesForPlanCache` 用当前 prepared 参数重新计算整数主键 handle 范围，避免复用
  首次执行时序列化的旧 `Ranges`。
- `ResolveIndicesItself` 对齐 Schema 列下标并解析延迟物化表达式；`ToPB` 把扫描编码成
  `tipb::TableScan`。
- `Clone`、`MemoryUsage`、两个代价入口和两个构造辅助函数支持计划复制、缓存、优化与公共
  物理计划生命周期。

## 主要符号

- `ColumnarIndexExtra`：列存索引的 `IndexInfo` 与 `tipb::ColumnarIndexInfo` 组合。Rust
  当前只保存/克隆它；本文件没有 Go 的 inverted/full-text 构造器和 Explain 细分逻辑。
- `PhysicalTableScan`：主结构体。`PhysicalSchemaProducer` 提供上下文、计划 ID、Schema、
  统计与公共子树行为；`Table`/`Columns`/`PhysicalTableID` 描述扫描对象；
  `AccessCondition` 生成范围，`FilterCondition` 表示残余过滤，
  `LateMaterializationFilterCondition` 可编码进存储请求。
- `New(ctx)` / `Init(ctx, offset)`：创建 `TypeTableScan` 空节点，并可重新绑定上下文、类型码
  和 query-block offset。其余扫描元数据由物理化调用者继续填充。
- `Clone(new_ctx)`：切换计划上下文，克隆 producer、Schema、表达式、表/列、范围、统计、
  属性及列存索引元数据；producer 克隆失败时返回 `expression::Error`。
- `RebuildRangesForPlanCache()`：要求 `Table` 存在且能找到主键列；克隆 ranger context，
  对所有 `AccessCondition` 调用 `BuildTableRange`。若仍有未被范围使用的条件则报错，保证
  typed handle-range 编码不会静默丢谓词。
- `AccessObject()` / `access_object(normalized)`：优先使用表别名，否则使用表名；分区扫描
  尝试按 `PhysicalTableID` 找分区名，归一化时以 `?` 代替。缺表时退化为
  `table:unknown`。
- `TP()` / `IsFullScan()`：显式 `TableRowIDScan` 类型优先；否则在无 `RangeInfo`、无相关
  访问列且范围为空或均为 full range 时返回 full scan，其他情况为 range scan。整数主键
  是否 unsigned 会传给 `Range::IsFullRange`。
- `OperatorInfo(normalized)`：按 `RangeInfo`、相关访问条件、静态 `Ranges` 的优先级输出
  范围，再附加 `KeepOrder`、TiFlash 延迟物化过滤与 `Desc`。归一化分支隐藏具体值。
- `AppendExtraHandleCol`：克隆或新建 Schema，同时把调用者传入的 handle `Column` 与
  `ColumnInfo` 追加到 Schema/列清单；它不负责自行从 DataSource 推导 handle。
- `BuildPushedDownSelection`：当前仅把传入条件追加到 `FilterCondition`，并不返回
  `PhysicalSelection`，也不执行 Go 版本的 TiFlash 谓词拆分。
- `BuildIndexMergeTableScan`：克隆节点、替换范围并强制 `KeepOrder = false`，供 Index Merge
  表侧扫描使用；clone 失败会以 `expect("table scan clone")` panic。
- `ResolveCorrelatedColumns` / `ResolveIndicesItself` / `ResolveIndices`：前者当前只是调用
  下标解析，并不重建相关范围；后两者先规范 Schema 列的 `Index`，再解析延迟物化条件。
- `ToPB(ctx, store)`：要求 `Table` 和 PB client（存在延迟物化条件时）可用；编码表/分区
  ID、方向、列类型与主键 handle 标记，附加下推过滤，返回 `TypeTableScan` executor。
- `GetPhysicalScan4LogicalTableScan(ctx, schema, stats)`：仅创建节点并注入 Schema/统计；
  `GetOriginalPhysicalTableScan(ctx, property)` 仅注入 `Desc`、`KeepOrder` 与 `Prop`。二者都
  不是 Go 同名函数的完整 DataSource/AccessPath 转换。

## 执行流程

1. 物理化阶段创建基础节点。旧 cascades 的 `ImplTableScan::OnImplement` 调用
   `GetPhysicalScan4LogicalTableScan` 后补入表、列和范围；传统路径在
   `base_physical_plan.rs` 中补入数据库名、别名、物理表 ID、访问条件、store 类型、
   顺序及过滤状态。
2. 节点通过 `direct_operator_core!` 成为 `PhysicalPlan`，可放入 reader、lookup reader 或
   MPP fragment。reader/fragment 通过类型下转收集扫描节点，Explain 路径调用本文件的
   类型与描述函数。
3. prepared typed KV 路径在执行前调用 `RebuildRangesForPlanCache`，把当前参数对应的单列
   有符号整数 handle 范围编码为记录 key；普通执行路径也可直接按
   `PhysicalTableID`（否则表 ID）打开记录前缀迭代器。
4. 存储下推路径调用 `ToPB`。它把每个 `ColumnInfo` 的 ID、类型、collation、长度、精度、
   flag、枚举值、数组标志与主键 handle 标志写入 protobuf，并将延迟物化条件经 client
   转成 PB 表达式。
5. 执行层 `physical_plan_runtime.rs` 将本节点识别为可流式扫描叶子，按 `Desc` 选择正向或
   反向迭代，解码记录 key/行值，并在整数 PK handle 未存入 row value 时从 key 补回。

## 数据与状态

状态可以分为五组：计划基座（`PhysicalSchemaProducer`、`Prop`、`FilterStats`）；对象身份
（`Table`、`DBName`、`TableAsName`、`PhysicalTableID`、`IsPartition`）；列与范围
（`Columns`、`Ranges`、`RangeInfo`、`IsCommonHandle`）；谓词
（`AccessCondition`、`FilterCondition`、`LateMaterializationFilterCondition` 及其选择率）；
执行/估算属性（`StoreType`、`IsMPPOrBatchCop`、`Desc`、`KeepOrder`、`TblColHists`、
`UsedColumnarIndexes`）。

关键不变量包括：分区 PB 使用 `PhysicalTableID`，非分区 PB 使用 `Table.ID`；
`RebuildRangesForPlanCache` 必须消费全部访问条件；Schema 中每列的 `Index` 应等于当前位置；
需要有序输出时构造方设置 `KeepOrder` 并以首个 sort item 决定 `Desc`；Index Merge 的表侧
克隆必须关闭 `KeepOrder`。

`MemoryUsage` 是近似值：计算 producer、`Columns` capacity、三组表达式、ranges 和三个
字符串，但未完整计算表元数据、统计、属性及列存索引堆内存。`GetScanRowSize` 也是简化
估算，仅按 Schema 列数乘 8，并非基于直方图和类型宽度的真实平均行宽。

## 依赖与调用关系

上游直接证据包括：

- `pkg/planner/cascades/old/implementation_rules.rs::ImplTableScan::OnImplement` 从
  `LogicalTableScan` 创建节点并补齐表、列、范围与顺序。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的逻辑扫描物理化路径创建并
  填充 table scan，还在 double-read 场景把它标成 `TypeTableRowIDScan`。
- `index_join_probe.rs`、Index Merge 和多处物理计划接线直接创建或下转该类型。

下游主要关系包括：

- `physical_table_reader.rs` 收集 table scan，并使用其 Explain/访问对象信息；`fragment.rs`
  将其识别为 MPP scan；`physical_hash_join.rs` 用 `TP` 展示子计划类型。
- `typed_adapter_bridge.rs::encode_table_record_ranges` 调用
  `RebuildRangesForPlanCache`，只接受非 common-handle 的整数主键范围。
- `physical_plan_runtime.rs` 的 `PhysicalTableSource`、`direct_table_scan`、
  `stream_rows_while` 和 `execute_node` 读取本节点并执行 KV 扫描。
- `cache_snapshot.rs::CachedTableScan` 捕获/恢复本结构体；存在
  `UsedColumnarIndexes` 时明确拒绝缓存。

直接 crate 依赖由 `physicalop/Cargo.toml` 复核：`base`/`property`/`costusage` 提供计划、
属性和代价接口，`expression` 提供表达式与 Schema，`model` 提供表/列/索引元数据，
`ranger` 负责范围，`kv` 提供 store 类型，`tipb` 提供存储下推协议，`mysql` 与
`plancodec` 提供标志和算子类型。

## 错误处理与边界

- `RebuildRangesForPlanCache` 对缺失主键 handle、ranger 错误或未完全消费的访问条件返回
  明确 `expression::Error`。它只实现单列 table-handle 范围；common handle 会被 typed
  adapter 的调用前检查拒绝。
- `ToPB` 对缺失 `TableInfo`、缺失 PB client 或表达式 PB 转换失败返回错误；但它没有 Go
  的动态分区 TiFlash PB、runtime filter、默认列值、fast scan、executor ID 和列存索引
  PB 接线。
- `BuildIndexMergeTableScan` 使用 `expect`，因此极少数 producer clone 错误会 panic 而非
  向上传播；扩展此路径时应评估是否改为 `Result`。
- `AccessObject` 可容忍缺表，但 `ToPB` 和执行层不允许缺表；构造辅助函数返回的半成品在
  进入下游前必须由调用者补齐元数据。
- `ResolveIndicesItself` 只有普通 `ResolveIndices` 路径，没有 Go 的
  `ResolveIndicesByVirtualExpr` 回退；重复虚拟表达式列可能因此表现不同。
- `BuildPushedDownSelection` 的名称容易误导：Rust 当前仅追加条件，不会创建 selection 或
  删除已被 late materialization 消费的条件。

## 并发与资源生命周期

本文件没有锁、线程、异步任务或通道。计划节点通常在优化阶段由单一可变所有者组装，
随后作为 `Box<dyn PhysicalPlan>` 进入计划树；修改范围、Schema 或过滤条件的方法使用
`&mut self`，展示、范围重建和 PB 编码使用 `&self`。

`ContextRef` 与 `HistCollRef` 承担共享上下文/直方图的生命周期；表达式以 `ExprBox` 独占，
`Clone` 会调用 `CloneExpr` 建立可独立修改的表达式副本。计划缓存不用普通对象引用跨执行
复用，而由 `CachedTableScan` 捕获可缓存字段、在新表达式上下文中恢复；列存索引计划被
拒绝缓存。执行侧 KV iterator 的打开/关闭属于 `physical_plan_runtime.rs`，不由本节点
持有资源。

## 与 Go 版本的对应关系

直接对照文件是同目录 `physical_table_scan.go`。Rust 已覆盖表扫描核心字段、基础构造与
克隆、访问对象、范围/全扫判定、Explain 主干、分区 ID、附加 handle、下标解析、延迟
物化过滤 PB、成本入口和内存估算，并由独立 Rust 测试覆盖空范围 Explain、别名/分区
归一化、有符号 handle 的 full-range 判定、列存索引克隆/缓存拒绝等行为。

仍有重要差异：

- Go 的两个同名构造函数从 `LogicalTableScan`/`DataSource`/`AccessPath` 完整复制元数据，
  调整 row count 与统计并处理 full-text pushdown；Rust 构造函数仅设置 Schema/统计或
  属性，完整接线分散在调用者中。
- Go 结构体还拥有 `HandleIdx`、`HandleCols`、`ByItems`、`PlanPartInfo`、`SampleInfo`、
  `TblCols`、used stats、runtime filters、grouped ranges 等字段；Rust 本类型未包含它们。
- 当前 Go `ExtractCorrelatedCols` 同时检查 `AccessCondition` 与
  `LateMaterializationFilterCondition`，且 `physical_plan_test.go` 验证后者；Rust 仅检查
  `AccessCondition`。Rust 独立测试把“只检查访问条件”固定为当前行为，两边尚未完全对齐。
- Go `ResolveCorrelatedColumns` 会替换相关列并为整数/common handle 重建范围；Rust 同名
  方法仅解析下标。Rust 另有 prepared-cache 专用的整数 handle 范围重建，但用途和覆盖面
  不等价。
- Go `BuildPushedDownSelection` 为 TiFlash 调用谓词下推并返回剩余
  `PhysicalSelection`；Rust 只追加 `FilterCondition`。Go `AppendExtraHandleCol` 也会从
  DataSource 查找或创建 handle，Rust 要求调用者传入两种列表示。
- Go `ToPB` 支持动态分区 TiFlash、keep-order/fast-scan、默认值、runtime filter、列存索引
  和 telemetry；Rust 当前 PB 只覆盖基础 table scan 与延迟物化条件。
- Go `GetScanRowSize` 使用统计和列类型区分 TiKV/TiFlash；Rust 是列数乘 8。Go
  `MemoryUsage` 与 Explain 也覆盖更多字段、stats、runtime filter、列存索引和 cluster
  table 分支。

因此该 Rust 文件是已接入规划和 typed 执行的可用表扫描核心，但不能视为 Go 文件全部
能力的逐项移植；上述差异是扩展时的兼容边界。

## 扩展指南

- 新增字段时同步 `New`、`Clone`、`MemoryUsage`、`cache_snapshot.rs::CachedTableScan` 的
  capture/restore、`lib.rs` 的 cache contract；若字段影响下推，还要同步 `ToPB` 和执行
  构建路径。
- 修改范围语义时同时检查 `IsFullScan`、`OperatorInfo`、
  `RebuildRangesForPlanCache`、`typed_adapter_bridge.rs::encode_table_record_ranges`，并覆盖
  prepared 参数、unsigned handle、开闭区间、未消费谓词和 common handle。
- 完善 late materialization 时应以当前 Go 的 `ExtractCorrelatedCols`、
  `BuildPushedDownSelection`、`ResolveIndicesItself` 和 `ToPB` 为行为基准，明确补上相关列、
  虚拟表达式回退及残余 selection，而不是扩大现有简化函数的注释承诺。
- 修改 Explain 或访问对象时同步普通/normalized 分支，确保 range、参数和分区名脱敏；
  最近的独立测试入口是 `physical_table_scan_test.rs`。
- 新增单元测试必须继续放在同目录独立测试文件，而非本生产 `.rs`。PB/下标的补充回归可
  放入 `physical_table_scan_aster_unit_test.rs`；Go 对照行为则参考
  `pkg/planner/core/physical_plan_test.go::TestPhysicalTableScanExtractCorrelatedCols`。
- 正确性风险集中在范围漏谓词、分区 ID、Schema/列顺序和相关列；兼容风险集中在 Explain
  文本与 PB 字段；性能风险集中在表达式/范围克隆、范围爆炸和过粗行宽估算。

## 验证依据

- RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `query PhysicalTableScan` 定位 Rust/Go 主类型及两个构造函数，`node --file` 完整读取本
  文件 553 行并报告 35 个使用文件。`callers/callees` 因 Rust/Go 同名及常用方法名产生
  歧义，故按技能规则用精确 `rg` 引用搜索补足调用边。
- 源码与模块证据：完整读取 `physical_table_scan.rs`、`physicalop/Cargo.toml`、`lib.rs`
  的模块导出、`direct_operator_core!` 接线与 cache contract。
- 上下游证据：`implementation_rules.rs::ImplTableScan::OnImplement`、
  `base_physical_plan.rs` 的逻辑扫描物理化、`physical_table_reader.rs`、`fragment.rs`、
  `typed_adapter_bridge.rs::encode_table_record_ranges`、`physical_plan_runtime.rs` 和
  `cache_snapshot.rs::CachedTableScan`。
- Go 对照：读取同目录 `physical_table_scan.go` 的结构体、构造、Clone、Explain、范围、
  selection、下标、PB 与代价相关实现；其文件共 1,026 行。
- 测试证据：完整读取 `physical_table_scan_test.rs` 151 行和
  `physical_table_scan_aster_unit_test.rs` 64 行；读取 Go
  `physical_plan_test.go::TestPhysicalTableScanExtractCorrelatedCols`，确认 Go 的延迟物化
  相关列与 PB 下推边界。
- 本任务仅新增说明文档，按计划不运行 Cargo；交付前使用任务指定命令验证文件存在且恰有
  11 个固定二级章节，并人工确认未把 Go 独有或 Rust 简化逻辑写成已支持能力。
