# `pkg/statistics/builder.rs`

## 文件定位

`builder.rs` 属于 `astersql-statistics` crate（`pkg/statistics/Cargo.toml`），负责把采样值或已经按值有序的数据构造成优化器可消费的 `Histogram` 与 `TopN`。模块在 `pkg/statistics/lib.rs` 中以 `mod builder` 装配，并通过 `pub use builder::*` 将公开符号重导出为 crate API。

当前生产接线的直接上游是 `RuntimeStatsBuilder::build_histogram_with_buckets`（`pkg/statistics/runtime_stats_builder.rs`）：它把 SQL 行转换为 `SampleCollector`，区分列和索引的字段类型后调用 `BuildHistAndTopN`。`BuildColumnHist`、`BuildColumn` 和 `SortedBuilder` 还作为通用构建 API 对外暴露；仓库内可见的 Rust 直接使用主要位于统计集成测试和规划器测试。

该文件不是持久化层，也不负责采样本身。采样输入由 `SampleCollector`/`SampleItem` 提供，直方图和 TopN 容器由同 crate 的 `histogram.rs`、`cmsketch.rs` 等模块提供；本文件承担“排序、分组、缩放、切桶和高频值剥离”的算法层。

## 核心职责

1. `SortedBuilder` 对有序 `Datum` 流在线切桶，在桶数达到上限时两两合并，并保证相同取值不会被拆到不同桶。
2. `BuildColumnHist`/`BuildColumn` 从列样本构造直方图，把样本桶的累计计数和重复数按总体行数缩放，并写回 NDV、NULL 数及列总大小。
3. `BuildHistAndTopN` 对 `SampleCollector` 排序，按相同编码值分组，选择并按总体采样比例放大 TopN，再跳过 TopN 的样本区间构建剩余直方图。
4. `SequentialRangeChecker` 利用样本下标单调递增这一前提，以单向游标跳过 TopN 覆盖区间，避免每个样本都全量扫描候选范围。
5. `calcCorrelation` 计算排序位置与原始序号之间的 Pearson 式相关性；`pruneTopNItem` 用“剩余 NDV 均匀分布”假设和超几何分布方差阈值裁掉不显著的低频候选。

两个调节常量只影响本文件算法：`topNPruningThreshold = 10` 使默认 TopN 模式可在候选达到目标容量的 10% 后跳过非末尾单例；`bucketNDVDivisor = 2` 在默认桶数且 TopN 未填满时，用剩余 NDV 的一半收紧桶数。

## 主要符号

- `TopNWithRange { TopNMeta, startIdx, endIdx }`：TopN 候选及其在已排序样本切片中的闭区间。`TopNMeta.Count` 在候选阶段是样本频次，最终写入 `TopN` 前才乘以采样放大系数。
- `SequentialRangeChecker`、`NewSequentialRangeChecker`、`IsIndexInTopNRange`：构造时按 `startIdx` 排序；查询时仅向前推进 `currentRangeIdx`。它适合按非递减下标查询，不是任意顺序的通用区间索引。
- `processTopNValue`：接收一个完整同值分组，执行单例剪枝，将候选按频次降序、编码字节升序排列，并截断到 `num_top_n`。
- `compareDatum`：使用 `types::DefaultStmtNoWarningContext` 和二进制 collator 比较两个 `Datum`，把底层错误转换为 `astersql_errors::SharedError`。
- `SortedBuilder`：内部持有 `hist`、桶上限 `numBuckets`、当前桶目标容量 `valuesPerBucket`、上一桶累计计数 `lastNumber`、当前桶下标 `bucketIdx`、总输入数 `Count` 和 v2 桶 NDV 开关 `needBucketNDV`。
- `NewSortedBuilder`：建立空直方图；内部桶上限至少为 1，`stats_version >= Version2` 时使用 `AppendBucketWithNDV`。
- `SortedBuilder::Hist`/`IntoHist`：分别借用和消费构建器以取得直方图。
- `SortedBuilder::Iterate`：在线接收下一个有序值，是 `SortedBuilder` 的状态推进核心。
- `BuildColumnHist`：从可变 `Datum` 切片构造列直方图；会原地排序样本。
- `buildHist`：内部辅助函数，在可选 TopN 区间过滤后按缩放后的容量建桶，并返回相关性计算所需的点积和。
- `BuildColumn`：从 `SampleCollector` 克隆样本值，并把收集器的 `Count`、FM Sketch NDV、`NullCount`、`TotalSize` 转交 `BuildColumnHist`。
- `BuildHistAndTopN`：本文件最完整的公开入口，返回 `(Histogram, TopN)`。
- `calcCorrelation`：由样本数与位置点积和计算相关系数；零或一个样本以及退化分母返回 `1.0`。
- `pruneTopNItem`：从已按频次排序候选的尾部连续裁剪不显著项。

文件没有 trait、宏或条件编译项。公开 API 保留了 Go 风格命名；crate 根的 `#![allow(non_snake_case, ...)]` 明确允许这种移植命名。

## 执行流程

`SortedBuilder::Iterate` 的状态机如下：

1. 每次输入先增加 `Count`。首个值创建第一桶并把全局 NDV 设为 1。
2. 后续值与当前桶上界二进制比较。相等时只增加当前桶累计 `Count` 与 `Repeat`，从而维持“同值不跨桶”。
3. 不相等且当前桶未超过 `valuesPerBucket` 时，调用 `updateLastBucket` 扩大上界并增加 NDV。
4. 当前桶已满且桶数已达 `numBuckets` 时，调用 `mergeBuckets` 两两合并，把目标桶容量翻倍，重新定位 `bucketIdx` 与 `lastNumber`。
5. 合并后再次判断容量；能容纳则更新末桶，否则新开一桶。每个新不同值只增加一次全局 NDV。

`BuildColumnHist` 先将 `ndv` 限制为不超过 `count`。空总体或空样本直接返回无桶直方图；否则用 `compareDatum` 原地排序样本，将其逐个送入 v2 `SortedBuilder`。构建完成后以 `count / samples.len()` 缩放每桶累计计数与重复数，最后以传入的真实/估算 NDV、NULL 数和总大小覆盖统计元数据。

`BuildHistAndTopN` 的主链为：

1. 从收集器取得总行数和 FM Sketch NDV，并将 NDV 上限设为总行数；空表、空样本或零 NDV 立即返回空桶直方图与空 TopN。
2. `sortSampleItems` 原地按样本值排序。列统计额外以“排序后下标 × 原始 `Ordinal`”求和，并调用 `calcCorrelation`。
3. 计算 `sample_factor = count / sample_count`。仅当请求的 `num_top_n` 等于运行时 `vardef::AnalyzeDefaultNumTopN` 时启用统计剪枝；显式非默认值被视为用户意图，不自动剪枝。
4. 列值用语句时区执行 `codec::EncodeKey`；索引样本已是编码字节，直接取 `Datum::GetBytes`。随后线性扫描相同编码值的连续区间，通过 `processTopNValue` 保留至多 `num_top_n` 个候选。
5. 默认 TopN 模式调用 `pruneTopNItem`。若真实 NDV 大于样本 NDV、采样放大系数大于 1 且候选吞掉全部样本不同值，则至少留下一个样本值供直方图建桶。
6. 将候选样本频次乘以 `sample_factor` 写入 `TopN` 并排序，同时累计被 TopN 占用的样本数和估算总行数。
7. 若候选数已等于 NDV，或请求桶数为 0，则直接返回。否则计算剩余 NDV；在默认桶数且 TopN 未填满时，用 `remaining_ndv / bucketNDVDivisor` 把桶数限制在 `[1, 原桶数]`。
8. 用候选范围创建 `SequentialRangeChecker`，调用 `buildHist` 跳过 TopN 样本，并以扣除 TopN 后的行数、NDV 和样本数建剩余桶。

`buildHist` 用 `count / sample_count_excluding_top_n` 放大样本，以 `min(count / ndv, sample_factor)` 作为单次不同值的保守重复估计。它先寻找第一个非 TopN 样本初始化桶，然后顺序处理余下样本：同值更新累计数和重复数，异值在容量允许时扩大当前桶，否则新开桶；同时累计未跳过样本的下标与 `Ordinal` 点积。

## 数据与状态

`Histogram.Buckets[*].Count` 是累计计数而不是单桶局部计数；`lastNumber` 因而记录前一桶累计值，用于计算当前桶实际容量。`Repeat` 表示桶上界值的估算频次。`Histogram.NDV` 在在线构建时按新不同值递增，但 `BuildColumnHist` 最终会以 FM Sketch 或调用者传入的 NDV 覆盖它。

`BuildHistAndTopN` 会修改 `collector.Samples` 的顺序；调用者若仍需要原始顺序，必须先克隆收集器或样本。样本的 `Ordinal` 保留原始物理次序，使排序后仍可计算列相关性。列 TopN 保存 `EncodeKey` 的可比较字节，索引 TopN 保存收集器中已有的索引键字节。

`TopNWithRange` 的 `startIdx..=endIdx` 必须对应排序后的 `collector.Samples`。候选经频次重排和截断后，`NewSequentialRangeChecker` 会再次按 `startIdx` 排序，因此 TopN 展示顺序与区间扫描顺序互不混淆。

`SequentialRangeChecker.currentRangeIdx` 是有状态游标；正确性依赖查询下标单调不减。`buildHist` 正好按 `enumerate()` 顺序查询，满足这一不变量。若外部调用者倒序或随机查询，游标不会回退，结果可能错误。

浮点缩放最终通过 `as i64`/`as u64` 截断；`BuildColumnHist` 的桶缩放则显式 `round()`。这两条路径的舍入语义不同，修改时不能互换而不检查 Go 兼容输出。

## 依赖与调用关系

上游关系：

- `pkg/statistics/lib.rs` 装配并公开本模块。
- `RuntimeStatsBuilder::build_histogram_with_buckets`（`pkg/statistics/runtime_stats_builder.rs`）是已确认的生产直接调用者：列路径传 `is_column = true`，索引路径传 `false`，并在调用前完成 Datum/索引键构造、超长样本过滤和总大小统计。
- `pkg/statistics/integration_test.rs` 直接调用 `BuildColumnHist`；`main_test.rs`、规划器 `cbo_test.rs` 使用 `NewSortedBuilder`；`sample_test.rs`、`go_merge_47_test.rs` 使用 `BuildHistAndTopN`；`handle/handletest/handle_test.rs` 使用重导出的 `calcCorrelation`。

下游关系：

- crate 内部数据类型/API：`Histogram`、`NewHistogram`、`TopN`、`NewTopN`、`TopNMeta`、`SampleCollector`、`SampleItem`、`sortSampleItems`。
- 数据比较与编码：`types::Datum`/`FieldType`、`collate::GetBinaryCollator`、`codec::EncodeKey`、`stmtctx::StatementContext`。
- 动态默认值：`vardef::AnalyzeDefaultNumTopN` 与 `AnalyzeDefaultNumBuckets`。它们决定请求值是“默认可优化”还是“显式必须尊重”。
- 错误边界：所有可能失败的公开构建入口返回 `astersql_errors::SharedError`。

`pkg/statistics/Cargo.toml` 将这些依赖限定在 `astersql-statistics` crate 内，关键本地依赖包括 `astersql-errors`、`astersql-util-codec`、`astersql-util-collate`、`astersql-sessionctx-stmtctx`、`astersql-sessionctx-vardef` 和 `astersql-types-datum`；该 Cargo 文件没有为 builder 定义 feature 分支。

## 错误处理与边界

- `compareDatum` 的比较错误统一转换成共享错误。`SortedBuilder::Iterate` 与 `buildHist` 立即向上传播；`BuildColumnHist` 因标准排序闭包不能返回 `Result`，会记录排序期间的比较错误，并在排序后返回错误。
- `BuildHistAndTopN` 会传播 `sortSampleItems`、列值 `EncodeKey` 和 `buildHist` 的错误。索引路径的 `GetBytes` 本身不返回错误。
- 空输入：`BuildColumnHist` 返回带元数据但无桶的直方图；`BuildHistAndTopN` 返回同类直方图以及容量为 0 的 TopN。
- `ndv` 在两个公开构建路径中都被限制为不超过总行数。`buildHist` 又以 `ndv.max(1)` 防止内部除零，且对空样本、非正的排除后样本数或非正桶数直接返回 `0.0`。
- `num_buckets = 0` 在 `BuildHistAndTopN` 中表示不建直方图；`NewSortedBuilder` 则会把直接传入的桶数提升到至少 1。两种入口的零值语义不同。
- `num_top_n = 0` 会使候选向量每次截断为空，最后正常进入纯直方图路径。
- `calcCorrelation` 对样本数小于等于 1 或理论分母为 0 返回 `1.0`，避免 NaN/除零。
- `pruneTopNItem` 在总行数不大于 1、候选已经覆盖 NDV 或候选不足两个时不剪枝；方差先以 `max(0.0)` 保护再开方。

调用契约没有在类型系统中表达“输入给 `SortedBuilder::Iterate` 的值已经有序”和“区间检查查询单调递增”。违反这些前置条件通常不会报错，而会产生错误桶或区间判断，扩展时需要额外测试保护。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。所有构建状态都由调用者独占的 `&mut SampleCollector`、`&mut Histogram`、`&mut SequentialRangeChecker` 或拥有所有权的 `SortedBuilder` 承载，因此一次调用内部是同步、单线程的。

`BuildColumn` 克隆 `SampleItem.Value` 后排序，不改变收集器样本顺序；`BuildHistAndTopN` 则直接原地排序 `collector.Samples`。`IntoHist` 消费 `SortedBuilder`，明确结束构建器生命周期；`Hist` 只提供不可变借用。

TopN 构建的临时资源包括全部编码值 `compared`、候选向量及排序开销。Rust 当前使用 `Vec` 每次插入后全量排序并截断，而不是 Go 的有界最小堆。`builder.rs` 自身没有内存跟踪器参数；生产上游 `RuntimeStatsBuilder` 对收集器和最终 `Histogram`/`TopN` 做内存记账，并用 `DropGuard` 在错误或正常退出时释放其管理的余额，但本文件内部 `compared` 等临时分配没有逐项接入 Go 风格的 buffered memory tracker。

动态默认值由 `vardef` 全局原子值读取。文件只读取、不写入；相关测试通过 Drop 守卫恢复测试期间修改的默认值。若并行测试同时修改这些全局值，调用结果仍可能受共享状态影响，测试应继续使用仓库既有隔离约定。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/statistics/builder.go`，类型和函数基本一一对应：`TopNWithRange`、`SequentialRangeChecker`、`SortedBuilder`、`BuildColumnHist`、`buildHist`、`BuildColumn`、`BuildHistAndTopN`、`calcCorrelation`、`pruneTopNItem` 均保留了 Go 名称和总体意图。常量值、二进制排序规则、相关性公式、TopN 统计显著性阈值、默认值判定和“采样未覆盖真实 NDV 时至少留下一个直方图值”的保护也相对应。

当前可见差异必须视为真实实现现状，而非自动假定等价：

- Go `SortedBuilder` 持有调用者 `StatementContext` 并用其 `TypeCtx` 比较；Rust `compareDatum` 固定使用 `DefaultStmtNoWarningContext`。涉及类型转换警告或上下文敏感比较时需要单独核验。
- Go `BuildHistAndTopN` 用 `generic.BoundedMinHeap` 保留候选，Rust用 `Vec` 的“插入、排序、截断”；结果的频次优先级和编码 tie-break 由 Rust显式排序实现，但复杂度更高。
- Go 入口接收 `memory.Tracker`，并对编码和临时 Datum 做缓冲记账；Rust入口没有该参数，记账位于生产上游且粒度不同。
- Go `BuildColumnHist` 直接调用 `buildHist` 并计算相关性；Rust版本改为对 Datum 排序后走 `SortedBuilder`、缩放桶计数，且不在该入口写相关性。因此它们的内部算法和相关性结果并非逐句复刻。
- Go 空输入的 `BuildHistAndTopN` 返回 `nil` TopN；Rust总是返回具体 `TopN` 值。
- Go 判定 TopN 是否覆盖全部样本不同值使用 `sampleNDV == lenTopN && lenTopN > 0`；Rust提前返回条件是 `candidates.len() as i64 == ndv`。两者在估算 NDV 与样本 NDV 不一致时并不完全相同，应由兼容性测试决定是否需要进一步对齐。
- Go 的候选 TopN 在相同频次下主要由有界堆行为决定；Rust显式以编码字节升序作为 tie-break，任何 golden 输出对顺序敏感时都应复核。

Go 基准 `pkg/statistics/builder_test.go` 覆盖高 NDV、低 NDV 性能场景和区间检查器（包括空、未排序范围）。Rust `pkg/statistics/builder_test.rs` 将两个基准场景改成规模更小的行为断言，并覆盖桶合并不拆重复值、区间推进及“TopN 不吞完采样值”。它验证核心契约，但不等同于 Go 的百万行性能证据。

## 扩展指南

- 调整在线切桶策略时，修改 `SortedBuilder::Iterate`，保持累计 `Count`、`lastNumber`、`bucketIdx` 和 `valuesPerBucket` 的联动，并继续保证同值不跨桶。同步扩展独立测试 `pkg/statistics/builder_test.rs` 和 `pkg/statistics/main_test.rs`，不要把测试内嵌进 `builder.rs`。
- 调整 TopN 选取、tie-break 或剪枝时，重点修改 `processTopNValue`、`pruneTopNItem` 和 `BuildHistAndTopN`。同步覆盖 `builder_test.rs`、`statistics_test.rs`、`sample_test.rs` 与 `go_merge_47_test.rs` 中的默认值/低频/采样 NDV 场景，并与 `builder.go` 的对应增量核对。
- 增加新的编码类型或改变列/索引键语义时，在 `BuildHistAndTopN` 的 `is_column` 分支和生产上游 `RuntimeStatsBuilder::build_histogram_with_buckets` 一起检查，尤其要保持语句时区、NULL、复合索引与超长样本过滤契约。
- 改变桶缩放或舍入时，要分别检查 `BuildColumnHist` 的 `round()` 和 `buildHist` 的截断逻辑，验证总行数、Repeat、NDV 与 TopN 扣减后的守恒关系。
- 若要优化性能，Rust候选向量是明确热点候选，可移植 Go 的有界堆；但必须保留相同频次的确定性排序、范围下标和剪枝前后语义，并补充大 NDV 基准，不能只以功能测试代替。
- 若要补齐内存限制，应沿现有 `RuntimeStatsBuilder` 记账模型设计，不要让本文件持有隐式全局 tracker；错误返回、提前返回和 panic 展开都需有对称释放证据。
- 任何暴露新 API 的变更都应检查 `pkg/statistics/lib.rs` 的重导出；本 crate 当前无 builder feature gate，不应假定依赖方可通过 feature 隔离变化。

兼容性风险主要是 Go/Rust统计输出差异会进一步影响优化器基数估计和计划选择；性能风险主要是候选每次全量排序、全部列值预编码和样本克隆；正确性风险集中在上下文敏感 Datum 比较、全局默认值、浮点取整以及区间检查器的单调查询前提。

## 验证依据

本说明依据以下直接证据形成：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/builder.rs` 报告目标文件含 21 个符号。
- RustCodeGraph `node --file pkg/statistics/builder.rs`：完整读取 550 行目标源码；图中报告该文件被 `runtime_stats_builder.rs`、`go_merge_47_test.rs`、`sample_test.rs` 使用。
- RustCodeGraph `query`：核对了 `NewSequentialRangeChecker`、`IsIndexInTopNRange`、`processTopNValue`、`NewSortedBuilder`、`Iterate`、`BuildColumnHist`、`buildHist`、`BuildColumn`、`BuildHistAndTopN`、`calcCorrelation`、`pruneTopNItem` 的 Rust/Go 定义。
- RustCodeGraph `node BuildHistAndTopN`：确认 Rust入口调用 `sortSampleItems`、`calcCorrelation`、`processTopNValue`、`pruneTopNItem`、`NewSequentialRangeChecker`、`buildHist` 和 `NewTopN`；同时取得 Go 对照入口及其生产调用者轨迹。精确 `callers/callees --file pkg/statistics/builder.rs` 在限定时间内未返回输出，因此调用方改由模块重导出、目标文件的“used by”关系和直接引用搜索交叉验证，未据此臆造额外生产调用者。
- 已读 Rust 路径：`pkg/statistics/builder.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/runtime_stats_builder.rs`、`pkg/statistics/builder_test.rs`、`pkg/statistics/main_test.rs`、`pkg/statistics/statistics_test.rs`、`pkg/statistics/integration_test.rs`、`pkg/statistics/sample_test.rs`、`pkg/statistics/go_merge_47_test.rs`。
- 已读边界与 Go 对照：`pkg/statistics/Cargo.toml`、`pkg/statistics/builder.go`、`pkg/statistics/builder_test.go`。
- 独立测试证据：`builder_test.rs` 验证桶合并、区间游标、一般/低 NDV TopN 和采样保护；`statistics_test.rs` 验证低频 TopN 剪枝；`integration_test.rs` 验证采样到列直方图的总行数/NDV；`go_merge_47_test.rs` 与 `sample_test.rs` 验证运行时默认 TopN/桶数对剪枝的影响。

本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付前执行任务指定的结构命令，要求文档存在且恰有十一个固定二级标题；此外人工复核文档已回答文件定位、运行主链、安全扩展点与已知 Go 差异。
