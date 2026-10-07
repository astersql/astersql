# `pkg/planner/cardinality/row_size.rs`

## 文件定位

本文件属于 `astersql-planner-cardinality` crate，是优化器的平均行宽估算实现。`pkg/planner/cardinality/lib.rs` 以私有模块 `mod row_size` 装入它，再通过 `pub use row_size::*` 将其中七个公开函数暴露给规划器其他 crate。它不估算行数或选择率，而是把 `statistics::HistColl`、列类型、存储格式和会话变量转换成“每行/每列预计占用多少字节”，供扫描、网络、内存和磁盘代价计算使用。

`pkg/planner/cardinality/Cargo.toml` 将该目录定义为独立库 crate（`lib.rs` 为 crate 根、`autotests = false`），并声明了这里直接使用的 expression、kv、mysql、planctx、statistics、tablecodec、chunk 等工作区依赖。测试不是内嵌在本文件中，而由 `lib.rs` 的 `#[path = "row_size_test.rs"] mod row_size_test` 接入。

## 核心职责

- `GetIndexAvgRowSize` 和 `GetTableAvgRowSize` 在通用列宽之上补齐索引键或记录键的固定结构开销，形成扫描行宽。
- `GetAvgRowSize` 根据统计是否可信、是否按 key 编码、是否为扫描以及 `EnableChunkRPC`，在伪宽度、普通编码宽度和 chunk 宽度之间选择。
- `GetAvgRowSizeDataInDiskByRows` 估算 `chunk::DataInDiskByRows` 形态的行宽，缺统计时按静态类型宽度回退，并为每列加入 8 字节的大小记录。
- `AvgColSize`、`AvgColSizeChunkFormat`、`AvgColSizeDataInDiskByRows` 分别实现普通 key/value 编码、chunk RPC 和磁盘行格式下的单列公式。

这些结果直接进入优化器成本或保护性判断：`pkg/planner/cascades/old/implementation_rules.rs` 用表/索引扫描行宽计算扫描成本；`pkg/planner/core/operator/physicalop/physical_sort.rs` 用磁盘行宽判断排序是否 spill；`pkg/planner/core/plan_cost_ver2.rs` 用行宽计算标准成本和 TiFlash late materialization 成本；`pkg/planner/core/optimizer_runtime.rs` 用行宽判断超长类型是否应放弃 chunk 复用。

## 主要符号

- `pseudoColSize: f64 = 8.0`：统计不可用或单列统计缺失时的每列兜底宽度。
- `GetIndexAvgRowSize(ctx, coll, cols, isUnique) -> f64`：先以 `isEncodedKey=true`、`isForScan=true` 调用 `GetAvgRowSize`，再加入 19 字节的 table/index key 前缀；非唯一索引另加 1 字节分隔符。调用契约假定 `cols` 已包含 handle。
- `GetTableAvgRowSize(ctx, coll, cols, storeType, handleInCols) -> f64`：按 value 编码取得基础行宽；TiKV 加 `tablecodec::RecordRowKeyLen` 后扣除已包含的 8 字节 row ID，TiFlash 在 handle 不在输出列时补 8 字节，最后钳制到非负。
- `GetAvgRowSize(ctx, coll, cols, isEncodedKey, isForScan) -> f64`：通用行宽入口。伪统计、空列统计或实时行数为零时，每列取 8 字节；否则逐列查直方图并选用普通或 chunk 公式。普通格式最后每列加 1 字节 flag，非扫描的 chunk RPC 格式每列加 1/8 字节 null bitmap。
- `GetAvgRowSizeDataInDiskByRows(coll, cols) -> f64`：逐列用磁盘格式公式；缺统计时调用 `chunk::EstimateTypeWidth`，并在汇总后每列加 8 字节元信息。
- `AvgColSize(c, count, isKey) -> f64`：handle 固定近似为 8 字节；浮点、时间类按 8 字节乘非 NULL 比例；整数等 key 编码也按 8 字节乘非 NULL 比例；其余情况使用 `TotColSize / count` 并保留两位小数。
- `AvgColSizeChunkFormat(c, count) -> f64`：定长类型直接使用 `chunk::GetFixedLen`；变长类型以平均数据长度减 `log2` 长度开销的近似值，并加 8 字节 offsets 开销。
- `AvgColSizeDataInDiskByRows(c, count) -> f64`：定长类型宽度乘非 NULL 比例；变长类型使用与 chunk 公式相同的两位小数和 `log2` 修正，但不加 offsets，因为行级汇总函数统一加入每列 8 字节大小记录。

本文件没有类型、trait、`impl` 或条件编译项；七个函数都是 crate 外可见的公开 API，常量仅模块内可见。

## 执行流程

1. 扫描成本调用方把规划上下文、列直方图集合和实际输出列传入表扫描或索引扫描入口；其他代价路径可直接选择通用行宽或磁盘行宽入口。
2. 表/索引入口先委托 `GetAvgRowSize`。该函数只读取 `ctx.GetSessionVars()`，再检查 `HistColl.Pseudo`、`ColNum()` 和 `RealtimeCount`，决定使用固定伪宽度还是逐列统计。
3. 逐列路径通过列的 `UniqueID` 调用 `HistColl::GetCol`。没有该列，或旧统计缺少 `TotColSize` 且该列并非 handle/全 NULL 时，单列回退为 8 字节。
4. 统计可用时，非扫描且开启 chunk RPC 的路径调用 `AvgColSizeChunkFormat`；其余路径调用 `AvgColSize`，并把 `isEncodedKey` 传入以区分整数 key 与 value 的编码宽度。
5. 汇总值先钳制为非负，再加入格式元数据：chunk RPC 加 `列数 / 8`，普通编码加 `列数`。表/索引入口随后再加入各自 key/row ID 固定开销。
6. 磁盘路径独立遍历列：伪统计或旧统计回退到静态类型宽度，可信统计调用 `AvgColSizeDataInDiskByRows`，最后加入 `8 * 列数` 并钳制为非负。

单列公式都首先处理 `count == 0` 并返回零。普通格式根据 `IsHandle`、SQL 类型和 `isKey` 决定固定宽度分支，否则使用平均 `TotColSize`；chunk/磁盘格式先由 `GetFixedLen` 区分定长与变长，变长平均宽度小于 1 时跳过 `log2` 修正，避免零或小数输入产生不适用的长度估算。

## 数据与状态

输入均为借用数据，函数不修改统计或会话状态。核心状态来自：

- `statistics::HistColl`：`Pseudo`、`RealtimeCount`、`ColNum()` 以及按 `UniqueID` 查询到的 `statistics::Column`。
- `statistics::Column`：`IsHandle`、`TotColSize`、`NullCount`、`TotalRowCount()` 和 `Histogram.Tp`。
- `expression::Column`：`UniqueID` 用于统计匹配，`GetStaticType()` 用于缺统计时的类型宽度回退。
- `SessionVars.EnableChunkRPC`：只影响非扫描的通用行宽格式选择及 null bitmap 开销。
- `kv::StoreType` 与 `handleInCols`：只影响表扫描的 TiKV/TiFlash 记录键补偿。

重要不变量是宽度结果不应为负：三个行级入口均在关键汇总点使用 `max(0.0)`；`AvgColSize` 和 chunk 变长分支也钳制统计计算值。`AvgColSizeDataInDiskByRows` 的定长宽度依赖正常统计下 `NullCount <= TotalRowCount` 的统计不变量，本文件不另行修复畸形直方图。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 文件使用关系与精确调用点核对：

- `pkg/planner/cascades/old/implementation_rules.rs` 的 `AverageRowSize`、`TableScanCostPlan::AverageRowSize` 和 `IndexScanCostPlan::AverageRowSize` 分别调用 `GetAvgRowSize`、`GetTableAvgRowSize`、`GetIndexAvgRowSize`。
- `pkg/planner/core/operator/physicalop/physical_sort.rs::GetCost` 调用 `GetAvgRowSizeDataInDiskByRows`，将 `row_size * count` 与内存配额比较，并据此计算 spill 的磁盘成本。
- `pkg/planner/core/optimizer_runtime.rs` 的超长行判断在可信统计存在时调用 `GetAvgRowSizeDataInDiskByRows`，用“每 chunk 行数 × 每行字节”决定是否跳过 chunk 复用。
- `pkg/planner/core/plan_cost_ver2.rs` 的 `canonical_row_size`、late materialization 路径调用磁盘行宽；`canonical_avg_row_size` 直接使用 `AvgColSizeChunkFormat`，或回退到 `GetAvgRowSize`。

下游依赖由 `use crate::*` 从 crate 根的重导出命名空间取得：`planctx::PlanContext` 提供会话变量；`statistics` 提供集合、列和直方图；`expression` 提供列标识和静态类型；`chunk` 提供定长判定与类型宽度；`mysql` 提供类型码；`kv` 提供存储类型；`tablecodec` 提供记录键长度常量。所有调用均为同步的纯计算或只读查询。

## 错误处理与边界

本文件不返回 `Result`，也不主动产生错误；不完整统计通过保守回退而不是错误传播处理。

- `count == 0`：三个单列函数立即返回 `0.0`，避免除零。
- 伪统计、无列统计或 `RealtimeCount == 0`：通用行宽每列取 8 字节；磁盘行宽按静态类型宽度估算。
- 找不到列直方图，或旧版本统计中非 handle、非全 NULL 列的 `TotColSize == 0`：使用相应伪宽度/类型宽度。
- 全 NULL 列不是“缺失统计”：chunk 变长格式仍保留 8 字节 offsets，磁盘单列数据宽度为 0；这一差异由测试明确覆盖。
- 未识别的 `StoreType`：`GetTableAvgRowSize` 不加入 TiKV/TiFlash 特有开销，只保留基础宽度。
- 空列切片：普通行宽和磁盘行宽都得到 0；表/索引入口仍可能加入自己的固定键开销。

`GetAvgRowSize` 对 `Option` 先判空再 `unwrap`，在当前无并发修改的借用模型下不会走到空值解包。浮点 `log2` 仅在 `avgSize >= 1.0` 时调用；代码未显式拒绝负的 `TotColSize`，而是依赖统计数据合法性并在若干返回点钳制结果。

## 并发与资源生命周期

本文件没有锁、原子变量、任务、通道、事务、I/O 或堆资源所有权。所有上下文、统计和列都通过共享借用传入，临时状态仅是局部 `f64` 累加值和短生命周期的 `Option<&Column>`。因此函数可随调用方并发使用，前提是传入类型自身满足调用环境的共享访问约束；本文件既不建立也不延长任何资源生命周期。

性能上，每个行级函数对 `cols` 做一次线性遍历；每列至多进行一次直方图查找、类型判断及常数次浮点运算。`log2` 只用于变长类型。扩展时应避免在此热路径加入 I/O、锁或与列数无关的全表扫描。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cardinality/row_size.go`，Rust 保留了相同的常量和七个导出函数，并对应以下语义：

- 索引 19 字节前缀、非唯一索引 1 字节分隔符、TiKV 记录键补偿及 TiFlash 隐式 row ID 均一致。
- 伪统计/旧统计回退、chunk RPC 分支、普通格式每列 1 字节 flag、chunk null bitmap 每列 1/8 字节一致。
- handle、数值/时间类型、整数 key 的固定 8 字节近似，以及 `TotColSize / count` 的两位小数舍入一致。
- chunk 和 `DataInDiskByRows` 对定长类型、变长 offsets、`log2(avgSize)` 修正和全 NULL 列的处理一致。

Rust 用 `chunk::VarElemLen` 的不等判断对应 Go 的 `fixedLen >= 0`，并用 `f64::round`/`max` 表达 Go 的 `math.Round`/`max`。Rust 的 `planctx::PlanContext` 实际是 crate 根定义并重导出的 object-safe `CardinalityContext` 子集，只暴露此类估算所需的上下文能力，而不是复制 Go 完整接口。

`pkg/planner/cardinality/row_size_test.rs::test_avg_col_len` 是 Go `pkg/planner/cardinality/row_size_test.go::TestAvgColLen` 的独立 Rust 对照测试：Rust 直接构造直方图，Go 通过建表、插入和 analyze 获得统计；二者验证相同的单行/双行整数、varchar、float、datetime 和全 NULL varchar 期望值。当前 Rust 测试主要直接覆盖三个单列公式；表/索引固定开销和伪统计行级分支应在扩展时补充独立测试。

## 扩展指南

- 修改编码布局或类型宽度时，应先确定影响的是普通 key/value、chunk RPC 还是 `DataInDiskByRows`，分别更新 `AvgColSize`、`AvgColSizeChunkFormat` 或 `AvgColSizeDataInDiskByRows`；同时核查实际 encode/decode、`chunk::GetFixedLen` 与 `chunk::EstimateTypeWidth` 的契约。
- 新增 SQL 类型时，应明确其定长/变长属性、key/value 编码差异及 NULL 比例是否影响宽度，不能仅加入类型枚举而忽略三种格式。
- 修改索引键、记录键或 row ID 布局时，应更新 `GetIndexAvgRowSize`/`GetTableAvgRowSize` 的固定开销，并检查 TiKV、TiFlash、唯一/非唯一索引四类路径。
- 修改统计兼容规则时，应同时审查 `GetAvgRowSize` 与 `GetAvgRowSizeDataInDiskByRows` 中完全对应的缺列/旧 `TotColSize` 判断，避免两类成本模型产生不一致回退。
- 测试应继续放在独立的 `pkg/planner/cardinality/row_size_test.rs`，不要内嵌到生产文件。建议扩充零计数、伪统计、缺列统计、handle、key/value、chunk 开关、空列，以及 TiKV/TiFlash 和唯一/非唯一索引矩阵；同步核对 `row_size_test.go` 的 Go 意图。
- 任何公式变更都要复查四个直接使用方，因为误差不仅改变扫描排序成本，还可能改变 spill、late materialization 和超长行保护分支，存在计划选择、内存与性能兼容风险。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件可完整读取。
- RustCodeGraph `files --filter pkg/planner/cardinality`：确认 `row_size.rs`、独立 `row_size_test.rs`、Go 对照与 crate 内相邻模块。
- RustCodeGraph `node --file pkg/planner/cardinality/row_size.rs`：核对 223 行源码、一个常量、七个公开函数及文件使用关系。
- RustCodeGraph 文件使用关系及精确调用点：`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/operator/physicalop/physical_sort.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/plan_cost_ver2.rs`。
- crate 边界：`pkg/planner/cardinality/Cargo.toml` 与 `pkg/planner/cardinality/lib.rs`，确认依赖、模块重导出和独立测试接线。
- 移植与边界测试：`pkg/planner/cardinality/row_size.go`、`pkg/planner/cardinality/row_size_test.go`、`pkg/planner/cardinality/row_size_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的 11 章节结构命令验证，并人工确认没有把未覆盖路径写成已测试事实。
