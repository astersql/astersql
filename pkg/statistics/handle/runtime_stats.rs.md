# `pkg/statistics/handle/runtime_stats.rs`

## 文件定位

本文件属于 `astersql-statistics-handle` crate，由 `pkg/statistics/handle/lib.rs` 以 `pub mod runtime_stats` 声明并通过 `pub use runtime_stats::*` 对外再导出。它位于 Rust ANALYZE 执行链和统计缓存数据模型之间：`pkg/session/runtime/statistics.rs` 将已解码的行、`TableInfo`、ANALYZE 选项和时区交给这里，本文件调用 `astersql_statistics::RuntimeStatsBuilder` 构造直方图、TopN 与 FM Sketch，最后组装成 handle crate 已有的 `TableStats`、`ColumnStats`、`IndexStats`。

该文件还定义三种跨模块传递的轻量运行时视图：`RuntimeHistoricalSnapshot`、`RuntimeColumnUsage` 和 `RuntimeAnalyzeJob`。它们本身不访问存储；`pkg/statistics/handle/handle.rs` 负责保存、查询和发布这些值，`pkg/domain/domain.rs` 与 `pkg/session/runtime/statistics.rs` 负责进一步暴露或填充它们。

## 核心职责

1. `BuildRuntimeTableStats*` 函数组把一组 `HashMap<String, Option<String>>` 行转换为可发布的 `TableStats`。转换过程按表元信息选择列和索引，委托规范统计包完成值解析、编码、直方图、TopN 和 FM Sketch 计算，再补齐 handle 缓存所需的版本、加载状态和计数字段。
2. `MergeRuntimePartitionStats` 将多个物理分区的列/索引直方图和 TopN 合并回逻辑表统计。它先把持久化形态的桶界解码为规范 `Histogram`，调用 `MergePartTopNAndHistToGlobal`，再编码回 `Bucket`；`MergeRuntimePartitionHistograms` 是从已有 global profile 推导 TopN 配额的兼容入口。
3. 三个 `Runtime*` 结构为历史统计摘要、列使用记录和 ANALYZE 作业状态提供稳定的数据载体。实际缓存及生命周期在 `Handle` 中实现，而不是在本文件中实现。

## 主要符号

- `RuntimeHistoricalSnapshot`：包含物理表 ID、版本、行数、修改数和 `is_historical` 标志。`Handle::historical_snapshot` 在命中历史快照时返回历史值，否则回退为当前缓存表的非历史视图。
- `RuntimeColumnUsage`：以 `(table_id, column_id)` 标识列，并用可空字符串保存 `last_used_at`、`last_analyzed_at`。`Handle::record_column_usage` 以该二元组为键覆盖写入。
- `RuntimeAnalyzeJob`：保存作业涉及的物理 ID、请求/上限/实际 worker 数、对象名、状态、错误、实例、进程 ID 和剩余时长。`pkg/session/runtime/statistics.rs` 构造它，`Handle::record_analyze_jobs` 更新内存状态。
- `convert_buckets(builder, histogram, is_index)`：内部函数。遍历规范直方图桶，用偶数/奇数位置分别编码下界和上界，并复制累计 `Count`、`Repeat`、桶 NDV；任一编码错误转为 `String` 并终止收集。
- `canonical_histogram(...)`：内部函数。把 handle 的 `Bucket` 重建为 `astersql_statistics::Histogram`。索引边界使用 builder 的索引解码；BIT 列按 UTF-8 十进制文本解析并重建指定字节宽度的 binary literal；其他列使用 `DecodeColumnTopNValue` 和 builder 时区。
- `canonical_top_n(entries)`：内部函数。将 `(encoded, count)` 列表写入规范 `TopN` 并排序，为分区合并准备输入。
- `MergeRuntimePartitionHistograms(...)`：公开兼容入口，从 global 中所有列/索引现有 TopN 长度的最大值推导合并预算，再使用默认 `SQLKiller` 调用完整合并入口。
- `MergeRuntimePartitionStats(...)`：公开的 ANALYZE 合并边界。分别处理非隐藏且非虚拟生成列与所有索引，仅合并 global 和 partition 中标记为已分析/已合成的对象，并使用显式 TopN、桶预算和 `SQLKiller`。
- `BuildRuntimeTableStats(...)`：公开便捷入口，创建默认 builder 后转发。
- `BuildRuntimeTableStatsWithBuilder(...)`：公开入口，复用调用者 builder，采用 `DefaultHistogramBuckets`，不限制列或索引集合。
- `BuildRuntimeTableStatsSelectionWithBuilder(...)`：完整构建入口，接收 builder、物理 ID、元信息、行、版本、TopN/桶预算以及可选索引/列选择集合。

## 执行流程

构建流程从 `BuildRuntimeTableStatsSelectionWithBuilder` 开始：

1. 若给出 `analyzed_indexes`，先从匹配索引的列名推导 `analyzed_columns`；空索引集合表示选择所有索引。未给索引选择时才沿用 `selected_columns`。
2. 遍历非隐藏且被选中的列。若列被单列、非前缀、非 MV 的唯一索引覆盖，则把该列 TopN 配额降为 0，与 Go 的唯一值处理一致。每行按小写列名取值，缺失键按 `NULL` 处理，然后调用 `build_histogram_with_buckets(..., is_index=false)`。
3. 把列直方图转换为 `ColumnStats`：写入 NDV、NULL 数、总大小、相关性、平均大小、TopN、桶和 FM Sketch。之后为每个未隐藏但未分析的 schema 列补一个默认 `ColumnStats`，保留字段类型，使未选择列/虚拟生成列仍对应一个 `stats_histograms` 零值行。
4. 遍历被选择的索引。空索引直接报错；每个索引列的 `Offset` 必须非负、在 `table.Columns` 范围内，且偏移位置的列名必须匹配。单列索引先剔除 NULL 输入并单独保存 NULL 数；唯一单列非前缀、非 MV 索引同样禁用 TopN。
5. 调用 `build_histogram_with_buckets(..., is_index=true)`，组装 `IndexStats` 并编码 FM Sketch。再为未分析的 schema 索引补默认项。
6. 以输入行数设置 `realtime_count` 和 `analyze_count`，以传入版本填充各版本字段，返回 `pseudo=false`、`initialized=true`、`stats_version=2`、`pre_scalar_ready=true` 的 `TableStats`。

分区合并流程由 `MergeRuntimePartitionStats` 执行：它先建立与 builder 时区一致的 statement context，并检查 `usize -> u32`、`usize -> i64` 预算转换。对每个可合并列/索引，按同一个 partition stats 对象成对收集 histogram 与 TopN，跳过不存在或未分析的分区项；无输入时保持 global 原值。存在输入时调用 `MergePartTopNAndHistToGlobal`，以分区快照而不是旧 global TopN 决定新 TopN，随后回写桶、NULL 数、大小和相关性。列保留 global 原有 NDV（它来自 FM Sketch 等上游结果）；索引回写时把 `total_column_size` 设为 0。

## 数据与状态

- 输入行是借用的不可变切片；本文件会为列/索引构建临时二维输入向量，但不保留原始行引用。
- 列和索引统计按数值 ID 存入 `HashMap`。列/索引选择采用 `BTreeSet<String>`，匹配 `Name.L`，因此依赖上游已规范化的小写名称。
- 桶的 `count` 是累计计数；TopN 保存独立质量。测试用“最后一个桶累计数 + NULL 数 + TopN 计数”核对总质量。
- 单列索引的 NULL 不进入索引直方图输入，但被显式写回 `histogram.NullCount`；多列索引不走该特殊分支。
- `average_size` 为 `TotColSize / rows.len()`；空输入明确返回 `0.0`，避免除零。
- `RuntimeStatsBuilder` 持有时区及构建过程所需的临时资源状态。本文件只借用它；构建出的 `TableStats` 不持有 builder 或规范直方图对象。
- 三个 `Runtime*` 结构均可克隆、比较并提供默认值。它们是快照/消息值，不在内部加锁或执行 I/O。

## 依赖与调用关系

上游主链是 `pkg/session/runtime/statistics.rs`：单 worker 和 scoped 多 worker 两条 ANALYZE 路径都调用 `BuildRuntimeTableStatsSelectionWithBuilder`；动态分区裁剪模式收集本批新 profile 与未分析分区的持久化 profile 后调用 `MergeRuntimePartitionStats`。该调用点在构建前后执行 kill 检查，合并时把同一个 `SQLKiller` 传入规范合并算法。

下游依赖如下：

- `astersql_meta_model::{TableInfo, mysql, types}`：表、列、索引元信息以及字段类型常量。
- `astersql_statistics::RuntimeStatsBuilder`：值解码/编码、直方图和 FM Sketch 构建；`Histogram`、`TopN`、`MergePartTopNAndHistToGlobal` 提供规范合并实现。
- `datum`：BIT 边界重建所需的 `Datum` 与 binary literal。
- `stmtctx`：创建携带 builder 时区的合并上下文。
- `sqlkiller`：让合并算法可响应取消；只有兼容包装函数使用默认 killer。
- crate 内 `Bucket`、`ColumnStats`、`IndexStats`、`TableStats`：最终缓存数据形态。

`pkg/statistics/handle/Cargo.toml` 将以上依赖声明为 workspace 内 path crate，并以 `package.metadata.porting.go-package = "pkg/statistics/handle"` 标明 Go 包来源。图索引确认本文件由 `lib.rs` 装配，直接测试模块为 `runtime_stats_test.rs`；文本引用搜索还确认生产调用者位于 `pkg/session/runtime/statistics.rs`，三个运行时视图由 `handle.rs`、`domain.rs` 和 executor/session runtime 使用。

## 错误处理与边界

- 所有规范统计包错误均被 `map_err(|error| error.to_string())` 转为 `Result<_, String>`；本层不增加类型化错误，也不吞掉失败。
- TopN 配额超过 `u32` 或桶配额超过 `i64` 时，合并在调用下游前返回明确错误。
- 列值缺失按 `NULL` 处理；隐藏列永不生成统计。虚拟生成列在构建阶段可得到默认零值项，但分区 histogram 合并明确跳过 `IsVirtualGenerated()` 列。
- 索引没有列、列偏移为负、偏移越界、偏移位置列名不匹配都会直接失败，防止用不一致 schema 生成错误索引编码。`analyze_runtime_aster_unit_test.rs` 覆盖越界偏移错误文本。
- BIT 边界必须是合法 UTF-8 十进制无符号整数；解析或位宽重建错误会传播。其他列解码服从字段类型和 builder 时区。
- 某个 global 列/索引不存在、未分析，或者所有 partition 都缺少可用统计时，合并跳过该对象而不是创建或清空它。
- 本文件不校验 `physical_id` 是否属于 `TableInfo`；发布阶段的批次重复、缓存注册等一致性由 `Handle::publish_runtime_stats_with_source` 校验。

## 并发与资源生命周期

本文件没有线程、锁、channel 或异步任务。所有构建与合并函数都同步执行：输入通过共享借用传入，仅 `global: &mut TableStats` 被原地修改；因此同一个 global profile 不能被并发写入。`pkg/session/runtime/statistics.rs` 的并行 ANALYZE 使用 `std::thread::scope` 为每个 worker 创建独立 builder，并把完成的 `TableStats` 推入外层 `Mutex<Vec<_>>`，并发控制不属于本文件。

每个列/索引的输入、规范 histogram 和 TopN 都是局部临时值，函数返回或当前迭代结束后释放。FM Sketch 被编码成字节并存入最终 stats；逻辑表 profile 的 FM Sketch 随后由上游清除。builder 的精确内存跟踪与成功/失败释放由规范统计包负责，`pkg/statistics/handle/analyze_runtime_aster_unit_test.rs` 的 `runtime_collector_releases_exact_input_memory_on_success_and_error` 验证该下游契约。

取消边界只存在于完整分区合并入口传入的 `SQLKiller` 中；逐列/索引构建函数不接收 killer，上游在构建任务之间调用 `check_killed()`。因此扩展长循环时应保持或增强上游取消检查，不能假定本模块会自动中断所有构建工作。

## 与 Go 版本的对应关系

Rust 并非逐函数复制一个同路径 `runtime_stats.go`；仓库中不存在该文件。其语义分散对应 Go 统计构建、全局分区合并和 handle 状态模型：

- `BuildRuntimeTableStatsSelectionWithBuilder` 对应 `pkg/executor/analyze_col_sampling.go` 中 `AnalyzeColumnsExec` 的采样统计构建。Go 在调用 `statistics.BuildHistAndTopN` 前，同样对单列非前缀唯一索引覆盖的列及唯一单列索引把 TopN 数量设为 0。
- `MergeRuntimePartitionStats` 对应 `pkg/statistics/handle/globalstats/global_stats.go` 收集各分区 histogram/TopN 后调用 `statistics.MergePartTopNAndHistToGlobal` 的路径。两边都从同一分区集合成对收集 histogram 与 TopN，使用 ANALYZE 的 TopN/桶选项和 SQL killer，并让合并函数从 histogram 重复值中生成 global TopN。
- `RuntimeColumnUsage` 对应 Go `pkg/statistics/handle/types/interfaces.go` 的 `ColStatsTimeInfo` 加表/列键；Go 时间是 `*types.Time`，Rust runtime 边界暂以 `Option<String>` 表示系统表文本。
- `RuntimeHistoricalSnapshot` 对应 Go `pkg/statistics/handle/storage/json.go::TableHistoricalStatsToJSON` 查询 `stats_meta_history` 所得的 table ID、版本、modify count 与 count 摘要；Rust 另加 `is_historical` 以表达回退当前缓存的情况。
- `RuntimeAnalyzeJob` 承接 Go `pkg/statistics/analyze_jobs.go::AnalyzeJob` 及 `mysql.analyze_jobs` 可见字段，但 Rust 结构还显式记录并发度、active worker、实例、进程 ID和剩余时间。因此它是 runtime 展示/发布 DTO，不应被误认为 Go `AnalyzeProgress` 原子计数器的逐字段翻译。

当前 Rust 实现固定产出 stats version 2 的 cache 形态，但 `analyzed_indexes` 保留 Go statistics version 1 的 `ANALYZE TABLE ... INDEX` 选择语义。文档中的“对齐”仅指上述已由源码与测试验证的行为，不代表 Go 统计子系统的所有持久化和后台任务都在本文件实现。

## 扩展指南

- 新增列/索引统计字段时，优先修改 `BuildRuntimeTableStatsSelectionWithBuilder` 的 `ColumnStats`/`IndexStats` 组装和 `convert_buckets`/`canonical_histogram` 的往返转换；同时确认分区合并是否需要回写该字段，避免构建值在 merge 后丢失。
- 新增字段类型或编码规则时，应在 `canonical_histogram` 增加与 `RuntimeStatsBuilder` 编码完全对称的解码，并在 `runtime_stats_test.rs` 增加 typed persisted bounds 往返测试。BIT、枚举、集合和时区敏感类型是现有回归基线。
- 改变 ANALYZE COLUMNS/INDEX 选择规则时，应修改 `analyzed_columns` 推导及两个 schema 零值补齐循环，并同步 `pkg/session/runtime/statistics.rs` 的调用参数和独立测试；不要把测试写回生产文件。
- 改变唯一索引 TopN 规则时，必须同时维护列分支和索引分支，并与 Go 的 `isColumnCoveredBySingleColUniqueIndex`、`isSingleColNonPrefixUniqueIndex` 比较，特别注意 prefix index 和 MV index。
- 改变 global merge 时，应使用显式 `MergeRuntimePartitionStats` 而非依赖旧 global profile 推导预算的包装入口；保持 histogram 与 TopN 来自同一 partition item，并保留 `SQLKiller`。相关质量守恒与 prior-global 隔离测试位于 `pkg/statistics/handle/runtime_stats_test.rs`。
- 扩展三个 `Runtime*` DTO 时，需要同步检查 `pkg/statistics/handle/handle.rs`、`pkg/domain/domain.rs`、`pkg/session/runtime/statistics.rs` 和 `pkg/executor/analyze.rs` 的构造/克隆/展示路径，以及 `handle_test.rs` 中的状态生命周期测试。
- 性能风险主要是按每列和每索引复制全部输入、重新编码 FM Sketch，以及分区合并时重建所有桶。新增昂贵处理前应评估行数乘以列/索引数的复杂度和 builder 内存跟踪，不应在本文件私自引入共享可变缓存。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/statistics/handle/runtime_stats.rs` 读取了完整 579 行，并报告该文件被 `runtime_stats_test.rs` 等模块引用；`query` 确认三个 DTO、三个构建入口、两个 merge 入口及三个内部转换函数的位置。
- RustCodeGraph 调用边：`BuildRuntimeTableStats -> BuildRuntimeTableStatsWithBuilder -> BuildRuntimeTableStatsSelectionWithBuilder`；`MergeRuntimePartitionHistograms -> MergeRuntimePartitionStats -> {canonical_histogram, canonical_top_n, convert_buckets}`。图对跨 crate PascalCase caller 未返回完整边，随后以直接符号引用搜索核对生产调用点。
- 读取的 Rust/Cargo 路径：`pkg/statistics/handle/runtime_stats.rs`、`pkg/statistics/handle/Cargo.toml`、`pkg/statistics/handle/lib.rs`、`pkg/session/runtime/statistics.rs`、`pkg/statistics/handle/handle.rs`、`pkg/domain/domain.rs`、`pkg/executor/analyze.rs`。
- 读取的独立 Rust 测试：`pkg/statistics/handle/runtime_stats_test.rs` 覆盖 global TopN 与 histogram 质量守恒、缺失分区列、旧 global TopN 隔离、BIT/ENUM/SET/时间类型边界重建；`pkg/statistics/handle/analyze_runtime_aster_unit_test.rs` 覆盖多列索引编码、非法偏移和 builder 内存释放；`pkg/statistics/handle/handle_test.rs` 覆盖 DTO 在 handle 中的记录行为。
- 读取的 Go 对照：`pkg/executor/analyze_col_sampling.go`、`pkg/statistics/handle/globalstats/global_stats.go`、`pkg/statistics/analyze_jobs.go`、`pkg/statistics/handle/types/interfaces.go`、`pkg/statistics/handle/storage/json.go`。仓库搜索确认不存在同路径 `pkg/statistics/handle/runtime_stats.go`，因此采用上述真实语义来源而未臆造一一对应文件。
- 本任务为纯文档分析，按计划不运行 Cargo；验收使用固定十一个二级标题的结构检查，并人工复核符号、调用关系、边界和安全扩展入口。
