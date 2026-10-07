# `pkg/executor/analyze_col_sampling.rs`

对应源码：[`analyze_col_sampling.rs`](./analyze_col_sampling.rs)。

## 文件定位

本文件是 `astersql-executor` crate 内一个自包含的列统计行采样实现。crate 根模块通过 `pkg/executor/lib.rs` 的 `pub mod analyze_col_sampling` 将它公开；同文件中的 `#[cfg(test)] mod analyze_col_sampling_test` 挂载独立测试。实现只依赖 Rust 标准库，并借助 `AnalyzeSamplingBackend` 抽象存储读取、响应解码、虚拟列计算、handle 构造、索引编码与取消检查，因此 `pkg/executor/Cargo.toml` 没有为本文件增加专用 feature 或第三方依赖。

当前接线边界需要特别说明：仓库搜索和 RustCodeGraph 的调用者查询没有找到生产代码调用本文件的 `AnalyzeColumnsExec::analyzeColumnsPushDown`；`pkg/executor/analyze_col.rs` 另有生产侧同名执行器类型。因而本文件目前是可公开、可独立测试的移植实现，而不是已经接入 SQL `ANALYZE` 主链的证明。Go 的生产实现位于 `pkg/executor/analyze_col_sampling.go`。

## 核心职责

1. `AnalyzeColumnsExec::analyzeColumnsPushDown` 组织一次完整采样：校验并发度、识别前缀索引或含虚拟列的特殊索引、单独取得其 NDV，再执行行采样统计构建。
2. `buildSamplingStats` 把后端原始包读取、并行解码/合并、样本后处理和并行直方图/TopN 构建串成两级 worker 流水线。
3. `RowSampleCollector::merge` 合并行数、NULL 数、总尺寸和 FM Sketch，并按 `priority` 保留最多 `sample_size` 个水库样本。
4. `build_one_stat` 针对列或索引提取可参与统计的值，应用字符串排序键、超长值过滤和唯一键 TopN 抑制规则；`build_hist_topn` 再生成简化的 TopN 与等分桶直方图。
5. `MemoryTracker`、`drainPendingSamplingMergeTasks` 和错误路径上的释放逻辑跟踪原始包与根收集器内存，避免已计费对象在取消或失败时遗留。

## 主要符号

- `Datum`：本文件的统计值表示，覆盖 NULL、有/无符号整数、字节串和文本；私有 `bytes` 是长度计算、哈希与编码辅助入口。
- `ColumnInfo`、`IndexColumn`、`IndexInfo`：采样所需的精简列/索引元信息。关键标志包括虚拟/存储生成列、字符串类型、单列唯一性和索引前缀长度。
- `AnalyzeSamplingBackend`：`Send + Sync + 'static` 的后端契约。它把 I/O 和 TiDB 领域操作隔离在实现之外，共有 `open_sampling`、`next_raw`、`close_sampling`、`decode_collector`、`decode_column`、`evaluate_virtual_columns`、`build_handle`、`analyze_index_ndv`、`collate_key`、`encode_index_value`、`killed` 十一个方法。
- `RowSampleCollector` 与 `ReservoirRowSampleItem`：保存累计行数、样本、逐列/索引 NULL 数、总尺寸、FM Sketch 和估算内存；`merge` 是其主要状态转换。
- `FMSketch`：用有序哈希集合提供简化 NDV 估计，合并时裁剪到 `MAX_SKETCH_SIZE`（10,000）。它不是 Go `statistics.FMSketch` 算法完整性的等价证明。
- `AnalyzeColumnsExec<B>`：公开编排器，持有元信息、采样/桶/TopN 配置、并发度、统计版本、handle 符号属性、后端与内存追踪器。
- `AnalyzeResult`、`AnalyzeResults`、`Histogram`、`HistogramBucket`、`TopN`：返回模型；最终结果按列和索引拆成两个 `AnalyzeResult`。
- `analyzeTask`、`analyzeIndexNDVTotalResult`、`samplingMergeResult`、`samplingBuildTask`：特殊索引 NDV、合并 worker 和构建 worker 的任务/结果载体。命名保留 Go 风格；其中 `analyzeIndexNDVTotalResult.error` 当前没有在 Rust 流程中使用。
- 自由函数：`readDataAndSendTask` 负责生产原始包，`drainPendingSamplingMergeTasks` 释放未消费包计费，`printAnalyzeMergeCollectorLog` 只格式化诊断文本；`build_hist_topn`、`collector_memory`、`stable_hash`、`panic_error` 是文件内算法辅助。

## 执行流程

1. `analyzeColumnsPushDown` 首先拒绝 `samplingStatsConcurrency == 0`，生成一个由 `handle_unsigned` 决定符号属性且 `non_null = true` 的扫描范围。
2. 它扫描全部索引；只要任一索引列有前缀长度，或对应列是虚拟生成列，就把该索引位置加入 `special_offsets`。`handleNDVForSpecialIndexes` 以至多配置并发度的 scoped threads 调用 `backend.analyze_index_ndv`，按索引 ID 汇总 sketch 和 NULL 数。
3. `buildSamplingStats` 调用 `open_sampling` 后启动一个生产者和多个合并 worker。生产者 `readDataAndSendTask` 每轮先检查取消标志和 `backend.killed()`，再读取原始包、按 `Vec` capacity 记账并发往通道。
4. 每个 `subMergeWorker` 维护私有 `RowSampleCollector`，通过 `decode_collector` 解码包、合并 collector，并同步调整内存计数。主线程再将各 worker collector 合入根 collector；首个非取消错误被保留，同时设置全局取消标志。
5. scoped threads 全部 join 后，流程把残留原始包排空并释放计费。无错误时关闭采样后端，解码普通列、计算虚拟列、构造每行 handle，并按 handle 排序以保留后续相关性计算所需顺序。
6. 对每条样本逐索引调用 `encode_index_value`，更新索引 NULL 数、总字节数与稳定哈希；特殊索引的位置随后由单独下推得到的 NDV/NULL 结果覆盖。
7. 第二阶段为每一列和索引发送一个 `samplingBuildTask`。`subBuildWorker` 调用 `build_one_stat`，结果按 `slicePos` 写回固定位置。普通虚拟非存储列返回空统计；列值过滤 NULL 和超过 1,024 字节的值，字符串先变成 collation key；索引按每个源列分别做超长过滤后才编码复合键。
8. 唯一单列（列元信息标记，或无前缀的唯一单列索引）将 TopN 数量强制为零。其他对象由 `build_hist_topn` 先按频率和值排序截取 TopN，再把剩余不同值近似等分到最多配置桶数所表达的分组中。
9. 成功时释放根 collector 的 `mem_size`，返回行数、直方图、TopN 与 sketches；入口再按 `columns.len()` 切成列结果和索引结果。失败时入口释放追踪器当前的全部字节并把错误放入 `AnalyzeResults.error`。

## 数据与状态

- 总数组槽位不变式是 `total_len = columns.len() + indexes.len()`：前半段属于列，后半段属于索引。`slicePos`、`null_count`、`total_sizes`、`fm_sketches`、直方图和 TopN 均依赖这一布局。
- `RowSampleCollector::merge` 累加完整输入行数，但只保留优先级最高的 `sample_size` 条样本；合并后重新用 `collector_memory` 估算样本内存。
- `sample_rate` 存在于执行器配置中但当前 Rust 实现没有读取它；采样率如何作用于原始 collector 取决于后端，不能从本文件断言其已经生效。
- `FMSketch::ndv` 当前直接返回保留哈希数。索引 sketch 在样本后处理阶段更新，特殊索引的 sketch/NULL 数则由 `analyze_index_ndv` 结果替换。
- `Histogram` 中的 `ndv` 和 `null_count` 来自 collector；桶 `count` 是当前样本频率合计，不包含 Go 统计库中的完整缩放与估计算法语义。
- `MemoryTracker` 是原子有符号计数器。原始包按 capacity、collector 按估算 `mem_size` 计费；它不分配或回收真实内存，只维护生命周期账本。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开模块，`pkg/executor/analyze_col_sampling_test.rs` 直接构造本文件的泛型执行器并调用入口；没有找到生产调用边。生产 SQL 侧的 Rust executor 定义在 `pkg/executor/analyze_col.rs`，不能与本类型混为一谈。

RustCodeGraph 确认的文件内主调用边为：`analyzeColumnsPushDown -> handleNDVForSpecialIndexes`、`analyzeColumnsPushDown -> buildSamplingStats`；`buildSamplingStats -> decodeSampleDataWithVirtualColumn/subMergeWorker/subBuildWorker/drainPendingSamplingMergeTasks/stable_hash`；`subBuildWorker -> build_one_stat`。图还确认 `buildSamplingStats` 通过 trait 调用 `open_sampling`、`close_sampling`、`build_handle` 和 `encode_index_value`。

下游实际 I/O 全由 `AnalyzeSamplingBackend` 承担；并发、集合、格式化和原子计数只使用 `std::{thread,sync,collections,fmt}`。Go 对照的生产入口是 `(*AnalyzeColumnsExec).analyzeColumnsPushDown`，由 Go executor 的 ANALYZE 流程使用；该事实不能反向证明 Rust 版本已经接线。

## 错误处理与边界

- `AnalyzeError` 区分后端错误、取消、解码、worker panic 和非法配置。多数后端错误通过 `?` 传播，入口统一转成 `AnalyzeResults.error`，而不是把 `Result` 暴露给调用者。
- 并发度为零会在启动线程前返回 `InvalidConfig`。但列/索引 offset 的合法性依靠构造者保证，越界访问会 panic；这是调用方必须维持的元信息不变量。
- worker panic 只在 `buildSamplingStats` join 生产者/合并 worker 时经 `panic_error` 转成 `WorkerPanic`；第二阶段构建 worker 的 join handle 未被收集，panic 可能表现为结果缺位而非显式错误。这与 Go 实现的 panic 捕获和指标上报不同。
- `close_sampling` 总会在第一阶段 scoped threads 返回后尝试执行；若线程阶段先失败，`result?` 会优先返回该错误，可能遮蔽 close 错误。后续解码、collation、handle 或索引编码失败则直接上抛。
- `MAX_SAMPLE_VALUE_LENGTH` 对列值和索引的每个源列分别生效；复合索引的最终编码可以超过 1,024 字节。这一边界由独立 Rust 回归测试明确覆盖。
- 非存储虚拟列不建直方图/TopN；NULL 不进入列值统计。索引 NULL 的语义依赖后端编码返回 `Datum::Null`，与 Go 对单列 NULL 的专门判断并非逐行同构。

## 并发与资源生命周期

两级流水线都使用 `thread::scope`，借用 `&self` 的线程不能逃离作用域。第一阶段是单生产者、多合并者：`mpsc::Receiver` 本身不能克隆，因此包在 `Arc<Mutex<Receiver<_>>>` 中，实际领取任务被互斥串行化，解码和合并可在锁外并行。`AtomicBool` 以 Acquire/Release 顺序传递取消状态。

第一阶段通过丢弃原始 `merge_tx` 使主接收循环在 worker 退出后结束；随后 join 所有线程并排空 `raw_rx`。第二阶段同样用带锁接收器分发列/索引任务，根 collector 通过 `Arc` 只读共享，结果经 channel 汇总。线程数被限制为 `min(samplingStatsConcurrency, task_count)`，同时至少为一；当前模型每次执行都会创建操作系统线程，不使用 Go 版的全局 goroutine pool。

内存生命周期以显式记账为主：生产者对包 capacity `consume`，worker 解码后释放包并转记 collector，根合并只记尺寸差；成功末尾释放根 collector。入口错误路径会把 tracker 当前总值整体释放。`pkg/executor/analyze_col_sampling_test.rs::build_error_releases_root_collector_memory` 验证 collation 失败后计数归零。

## 与 Go 版本的对应关系

结构上，Rust 的 `analyzeColumnsPushDown`、`decodeSampleDataWithVirtualColumn`、`buildSamplingStats`、`handleNDVForSpecialIndexes`、`subIndexWorkerForNDV`、`buildSubIndexJobForSpecialIndex`、`subMergeWorker`、`subBuildWorker`、`drainPendingSamplingMergeTasks` 和 `readDataAndSendTask` 均对应 `pkg/executor/analyze_col_sampling.go` 中的同名逻辑。两者都采用特殊索引 NDV 下推、行样本合并、按 handle 排序、并行构建列/索引统计的总体次序，也都按源列而非最终复合键过滤超长索引样本。

Rust 版本目前是明显收敛的领域模型：后端 trait 代替 Go 的 distsql/session/statistics/job/protobuf 依赖；直方图与 FM Sketch 算法为本地简化实现；没有 Go 版的 context cause、全局 worker pool、failpoint、panic 指标、AnalyzeJob 进度、对象池、内存超限动作、prefix datum 截断、statement error context、结果内存计费等完整行为。`buildSubIndexJobForSpecialIndex` 也只是把 `IndexInfo` 包装成任务，而 Go 版会创建索引执行器和统计任务。故文档中的“对应”表示流程与意图可追踪，不表示生产等价或迁移完成。

测试方面，Rust 独立测试只有两个聚焦回归：`composite_index_keeps_value_when_each_component_is_within_limit` 对应 Go `subBuildWorker` 的逐索引列长度过滤意图；`build_error_releases_root_collector_memory` 对应 Go 的 collector 错误清理约束。Go 的更广泛并发、panic、取消、内存与虚拟列覆盖散布在 executor 的 analyze 测试中，Rust 当前没有同等覆盖。

## 扩展指南

- 若要接入生产主链，先明确如何与 `pkg/executor/analyze_col.rs::AnalyzeColumnsExec` 统一或桥接，避免长期保留两个同名执行器；接线必须从 builder/analyze 上游补真实调用证据，并新增独立测试文件，不能把测试嵌入本源文件。
- 扩充后端能力应优先修改 `AnalyzeSamplingBackend` 及其实现，并同步 `pkg/executor/analyze_col_sampling_test.rs` 的 mock。新增必需方法会影响所有 trait 实现；仓库当前搜索只发现该测试实现，但接线前仍应重新查询。
- 修改数组布局、索引 offset 或特殊索引判定时，应同时检查 `analyzeColumnsPushDown`、`buildSamplingStats`、`build_one_stat` 和 `analyzeIndexNDVTotalResult`，保持“列在前、索引在后”不变量，并添加越界/空列或空索引回归。
- 对齐 Go 行为时，优先补全真实统计算法、prefix 截断、取消/panic 传播和细粒度内存记账，而不是仅扩展本地数据结构；每一项都应以 `pkg/executor/analyze_col_sampling.go` 的对应增量和独立 Rust 测试为依据。
- 调整采样值过滤必须保留复合索引“逐组件检查”的规则，并同步 `composite_index_keeps_value_when_each_component_is_within_limit`。调整错误或资源释放则同步 `build_error_releases_root_collector_memory`，并建议补生产者失败、解码失败、worker panic、close 失败和构建 worker panic 测试。
- 性能风险集中在每次执行创建线程、带锁的共享 receiver、样本合并排序、`Datum::bytes` 的重复分配以及 BTree 集合操作；优化时必须同时验证统计语义和 tracker 归零，不应仅以吞吐量作为完成条件。

## 验证依据

- 源码：[`pkg/executor/analyze_col_sampling.rs`](./analyze_col_sampling.rs)，已核对全部常量、类型、trait、impl、自由函数及线程/通道生命周期；该文件没有条件编译项。
- 模块与 crate：[`pkg/executor/lib.rs`](./lib.rs) 公开生产模块并以 `#[cfg(test)]` 挂载独立测试；[`pkg/executor/Cargo.toml`](./Cargo.toml) 声明 crate 根为 `lib.rs`，唯一 crate feature `nextgen` 与本文件无直接条件关系。
- Rust 测试：[`pkg/executor/analyze_col_sampling_test.rs`](./analyze_col_sampling_test.rs)，覆盖复合索引按组件限制长度以及构建错误后释放根 collector 内存。
- Go 对照：[`pkg/executor/analyze_col_sampling.go`](./analyze_col_sampling.go)；辅助定位还参考 [`pkg/executor/analyze_col.go`](./analyze_col.go) 与 Rust 的 [`pkg/executor/analyze_col.rs`](./analyze_col.rs)，用于区分真实生产执行器和本文件的当前接线状态。
- RustCodeGraph：索引状态为 11,467 files / 307,296 nodes / 1,848,419 edges；`query` 精确定位 `analyzeColumnsPushDown`（第 286 行）、`buildSamplingStats`（第 378 行）、`subBuildWorker`（第 637 行）；`callees` 验证入口、两级 worker 和 backend trait 的上述调用边。`callers` 未返回本文件入口的生产调用者，仓库 `rg` 仅发现独立测试直接使用本模块。
- 人工边界复核：本说明明确标出了未接线、简化统计算法、未使用的 `sample_rate`、不完整 panic 传播和 Go/Rust 覆盖差异，没有把预期设计写成当前支持事实。
