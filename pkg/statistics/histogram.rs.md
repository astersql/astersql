# [`pkg/statistics/histogram.rs`](./histogram.rs)

## 文件定位

`histogram.rs` 是 `astersql-statistics` crate 的直方图核心实现。crate 根 `pkg/statistics/lib.rs` 通过私有模块 `mod histogram` 装配它，再以 `pub use histogram::*` 将其 API 暴露给列统计、索引统计、表统计和统计加载链路。`pkg/statistics/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/statistics`，本文件的直接 Go 对照是 `pkg/statistics/histogram.go`。

它位于统计信息生产、持久化和优化器消费之间：ANALYZE 或存储加载得到桶边界与计数，`Histogram` 提供等值、范围和越界行数估计；全局统计构建可合并多个分区直方图；`Column`、`Index` 等上层对象还复用这里定义的加载/驱逐状态。RustCodeGraph 显示该文件被约 30 个文件引用；可见的直接消费点包括 `pkg/statistics/index.rs` 的 `Index::QueryBytes`、`pkg/statistics/column.rs`/`table.rs` 的加载状态判断，以及 `pkg/statistics/handle/runtime_stats.rs` 的分区统计合并。

## 核心职责

1. 定义等深直方图的数据模型和不变量：`Histogram::Bounds` 以 `[lower0, upper0, lower1, upper1, ...]` 交错保存边界，`Buckets[i].Count` 是截至第 `i` 桶的累计非 NULL 行数，`Repeat` 是该桶上界的重复次数，`NDV` 是桶内不同值数。
2. 维护桶及边界，包括追加、截断、压缩、删除 TopN 值、V2 索引标准化、解码和字段类型转换。
3. 提供选择率基础估计：定位值所在桶，计算等值、小于、大于、半开区间 `[lower, upper)` 及直方图值域之外的 `RowEstimate`。
4. 在索引直方图边界中寻找高频前缀，结合 `CMSketch` 抽取 `TopN`。
5. 在 Rust/Tipb 边界序列化和反序列化桶，并合并相邻或分区直方图。
6. 定义 `StatsLoadedStatus`，供列和索引统计判断是否初始化、是否完整加载以及是否需要重新加载。

本文件不负责生成采样、构建 CMS/FMSketch、从系统表读取统计或直接生成执行计划；这些职责分别位于同 crate 的 `sample.rs`、`cmsketch.rs`、handle 子树以及规划器。标量插值的具体实现位于 `pkg/statistics/scalar.rs`，本文件通过 `Histogram::calcFraction`、`PreCalculateScalar`、`convertDatumToScalar` 和 `calcFraction4Datums` 使用它。

## 主要符号

- `Histogram`：主体类型。`Tp` 是规范化后的字段类型；`Bounds`、`Buckets`、`Scalars` 必须按桶下标对应；`ID`、全局 `NDV`、`NullCount`、`LastUpdateVersion`、`TotColSize`、`Correlation` 保存统计元数据。`NewHistogram` 会为字符串类型强制使用 binary collation，`NewPseudoHistogram` 创建无桶伪统计。
- `Bucket`：保存累计 `Count`、上界 `Repeat` 和桶内 `NDV`。`BucketCount(i)` 通过相邻累计值差分得到单桶行数。
- `scalar`：缓存一个桶上下界的浮点表示和字节公共前缀长度；`PreCalculateScalar` 在 `scalar.rs` 中填充它，`LessRowCountWithBktIdx` 的桶内插值会读取它。
- `OutOfRangeShape`：把与行数无关的越界几何缓存为 `OneValue`、`HistNDV`、三角分布重叠比例及 `Empty`/`Impossible` 标志。`OutOfRangeShape` 负责几何探测，`ScaleOutOfRangeShape` 再结合实时行数、修改量和偏斜参数缩放；`OutOfRangeRowCount` 是二者的组合入口。
- `RowEstimate`：包含期望值 `Est`、下界 `MinEst` 和上界 `MaxEst`，支持逐分量加减、整体乘除与 `Clamp`。`DefaultRowEst` 构造三点相同的确定估计。
- `HistogramRange`：`SplitRange` 使用的单列范围，包含端点 Datum 与开闭标志；`checkKind`、`validRange` 提供类型和范围合法性辅助判断。
- `HistogramToProto` / `HistogramFromProto`：映射 `NDV` 及每个桶的累计计数、边界、重复数和桶 NDV。Proto 不承载本类型的 ID、NULL 数、版本、总列大小和相关系数，反序列化时这些字段取构造默认值。
- `MergeHistograms`：按值域顺序合并两个直方图，并通过 `mergeBuckets` 将桶数压到预算内；相邻边界相等时先合并接缝桶并消除一次 NDV 重计。
- `bucket4Merging`、`mergeBucketNDV`、`mergePartitionBuckets`、`MergePartitionHist2GlobalHistWithLocation`：分区直方图合并中间表示和主流程。中间桶使用非累计 `Count`，显式携带上下界和 `disjointNDV`。
- `GetIndexPrefixLens` / `ExtractTopN`：按 `codec::CutOne` 解出完整索引键的每列前缀长度，从边界候选估计频率，再从 CMSketch 查询真实计数并转移到 TopN。
- `StatsLoadedStatus`：两个内部字段分别表示“曾从存储初始化”和驱逐程度；构造器为 `NewStatsFullLoadStatus`、`NewStatsAllEvictedStatus`。
- 版本和状态常量：`Version0` 表示未 ANALYZE/伪统计，`Version1` 是基础直方图，`Version2` 配合 TopN 和桶 NDV；`AllLoaded`/`AllEvicted` 是当前两个加载状态。`IsAnalyzed` 与 `IsColumnAnalyzedOrSynthesized` 提供版本判断。

## 执行流程

### 构造、维护与序列化

1. `NewHistogram` 调用 `prepareFieldTypeForHistogram` 克隆字段类型；字符串类型改为 binary collation，然后按桶预算预分配 `Bounds` 和 `Buckets`。
2. `AppendBucketWithNDV` 同步追加一个 `Bucket` 和两个边界；`AppendBucket` 将桶 NDV 设为 0。调用方必须提供有序、非递减累计计数，本函数不自行校验。
3. `DecodeTo` 仅解码 `KindBytes` 边界并更新字段类型；`ConvertTo` 则克隆整个直方图，逐个调用 `Datum::ConvertTo` 生成转换后的边界。
4. `HistogramToProto` 遍历桶，把交错边界写到 `tipb::Bucket`；`HistogramFromProto` 固定构造 Blob 类型直方图，再按 Proto 顺序追加桶。该往返只保证 Proto 所含字段。

### 等值与范围估计

1. `LocateBucket` 在交错 `Bounds` 上用 `partition_point` 找到首个大于等于目标值的边界，并区分：超出最大值、落在桶间隙、命中桶上界、位于桶内部。空直方图返回越界且未找到。
2. `EqualRowCount` 命中上界时直接返回 `Repeat`；未命中但存在有效桶 NDV 时，用“单桶行数减 Repeat”除以“桶 NDV 减一”；否则按全局 `NotNullCount / NDV` 均分。第二个返回值指出是否使用了桶内精确信息。
3. `LessRowCountWithBktIdx` 先取前一桶累计计数；若命中上界，返回当前累计计数减 `Repeat`；若在桶内，则由 `scalar.rs` 的 `calcFraction` 线性插值。`GreaterRowCount` 用非 NULL 总数减去小于和等于估计并钳制到非负。
4. `BetweenRowCount` 以两个“小于”估计之差计算 `[lower, upper)`，对过小结果以低端等值估计和全局 NDV 平均值修正；两个端点落在同一有效桶时，把 `MaxEst` 至少提升到整个桶的行数。

### 越界估计

1. `OutOfRangeShape` 对空直方图直接标记 `Empty`；否则将 NDV 至少置 1，并以非 NULL 行数除以 NDV 得到单值基线。
2. 字符串/字节边界先计算直方图端点与查询端点的公共前缀，再统一标量化。无符号列会把负端点钳到 0；若范围因负数钳制完全坍缩，或上界小于下界，则标记 `Impossible`。
3. 以直方图宽度在左右各扩一倍，`calculateLeftOverlapPercent` 和 `calculateRightOverlapPercent` 用三角分布面积求查询范围在两侧扩展带的重叠比例。
4. `ScaleOutOfRangeShape` 在禁止使用修改量时直接返回单值基线；否则根据实时行数与直方图行数差、低 NDV 的 1% 下限、默认 0.5 或显式偏斜比，形成 `MinEst/Est/MaxEst`。空形状优先返回 0；允许修改量时，不可能范围返回 0。

### TopN 抽取与分区合并

1. `ExtractTopN` 先调用 `PreCalculateScalar`，对每个桶边界用 `GetIndexPrefixLens` 枚举编码列前缀并去重；用 `[prefix, prefixNext(prefix))` 的范围估计筛掉低于平均桶深度的候选。
2. 候选按估计频率降序截断后，真实频率由 `CMSketch::QueryBytes` 获取；随后从 CMS 扣除并追加到 `TopN`，最后排序。`num_columns` 只作为返回 Vec 的容量提示，不能截断一个合法多列编码键。
3. `MergePartitionHist2GlobalHistWithLocation` 把每个累计桶转成独立计数的 `bucket4Merging`，再按会话时区将弹出的 TopN 项转成单点桶；空桶被过滤。
4. 中间桶按上界、下界排序后，从右向左按目标桶预算聚合；相同上界必须进入同一全局桶，跨越当前左边界的桶按 `calcFraction4Datums` 拆分。`mergeBucketNDV` 按相同、不相交、包含或部分重叠区间估算 NDV，`mergePartitionBuckets` 再以每多合并一桶乘 1.15 的启发式膨胀，并以各桶 NDV 总和封顶。
5. 最终桶反转为升序并恢复累计 Count；上界 `Repeat` 至少取所有输入直方图对该值的等值估计之和。列直方图的桶 NDV 被清零，索引直方图保留桶 NDV。

## 数据与状态

- 核心不变量是 `Bounds.len() == Buckets.len() * 2`，并且边界按值域有序。多个方法直接按 `index * 2` 索引，违反该不变量会 panic 或产生错误估计。
- `Bucket::Count` 是累计值，而 `bucket4Merging::Bucket.Count` 是单桶质量；`buildBucket4Merging` 通过 `BucketCount` 完成转换，最终输出阶段再恢复累计值。扩展合并算法时不能混用两种语义。
- `NotNullCount` 取最后一个桶的累计 Count；`TotalRowCount` 再加 `NullCount`。空直方图的非 NULL 行数为 0。
- `Scalars` 是可重建缓存而非权威边界。`TruncateHistogram` 会同步截断它；`DestroyAndPutToPool` 只是清空三个 Vec，并没有实际全局对象池。
- `MemoryUsage` 使用 Vec capacity 估算 `Bounds/Buckets/Scalars` 的持有内存；三个容器均为空时按 Go chunk 语义返回 0，即使 Vec 已预留容量。
- `StatsLoadedStatus::default()` 表示未初始化；此时 `IsLoadNeeded`、`IsEssentialStatsLoaded`、`IsAllEvicted` 和 `IsFullLoad` 都为 false。完整加载状态不需要重载；全部驱逐状态需要重载。
- `HistogramEqual` 按 Go 的字符串契约比较格式化桶内容，可选择忽略 ID；它不会比较未进入 `ToString` 的 NULL 数、版本和相关系数。需要完整结构相等时应使用 `Histogram::Equal`。

## 依赖与调用关系

- 类型与比较：`types` 提供 `Datum`、`FieldType`、MySQL 类型和上下文；`collate::GetBinaryCollator` 用于边界比较；`types-field` 提供字符串 EvalType 判断。
- 编解码：`codec::DecodeOne` 解码字节边界，`codec::CutOne` 切分索引键，`crate::topNMetaToDatum` 在指定 `chrono_tz::Tz` 中恢复 TopN Datum；错误统一转成 `astersql_errors::SharedError`。
- 线协议：`protobuf::RepeatedField` 与 `tipb::{Histogram, Bucket}` 承载下推/分析协议中的直方图。
- 同 crate 下游：`scalar.rs` 提供标量转换和分数；`cmsketch.rs`/TopN 提供频率结构；`index.rs` 用 `EqualRowCount`、加载状态和 TopN/CMS 组合查询；`column.rs`、`table.rs` 使用加载状态控制驱逐与按需加载。
- 上游/应用链：`pkg/statistics/handle/storage/read.rs` 读取存储后会预计算标量；`pkg/statistics/handle/runtime_stats.rs::MergeRuntimePartitionHistograms` 及其测试覆盖运行时分区统计合并。RustCodeGraph 对 `HistogramToProto` 的精确结果还显示 `pkg/statistics/histogram_test.rs::histogram_proto_and_merge_keep_cumulative_counts` 为直接调用者。
- crate 边界：`pkg/statistics/Cargo.toml` 声明本地依赖 `astersql-errors`、chunk、codec、collate、stmtctx、tablecodec、types、types-field、vardef 和远端 `tipb`；本文件直接使用其中 errors、codec、collate、types、types-field、tipb，以及 `chrono-tz` 与 `protobuf`。

## 错误处理与边界

- `compareDatum` 在比较失败时以相等（0）继续，这是兼容性兜底而不是错误传播；边界 Datum 无法比较时可能隐藏排序问题，新增类型时必须专门覆盖。
- `DecodeTo`、`ConvertTo`、`GetIndexPrefixLens`、`ExtractTopN` 和分区合并公开返回 `SharedError`。典型错误源是 Datum/索引键解码、类型转换、TopN Datum 恢复，以及非法桶顺序。
- `mergeBucketNDV` 会对右桶上界小于左桶，或同上界但右桶下界更小返回 `illegal bucket order`；`mergePartitionBuckets` 对空输入返回 `not enough buckets to merge`；全局合并拒绝 0 桶预算。
- 空输入直方图在估计、Proto、合并和全局合并中均有显式分支：分区列表为空返回 `Ok(None)`，全部桶质量为 0 则返回保留元数据的空直方图。
- `GetLower`、`GetUpper`、`typeMatch`、`validRange` 默认调用方提供非空且结构正确的数据；它们不做完整防御性检查。`updateLastBucket` 也要求已有至少一个桶。
- `BinarySearchRemoveVal` 名称保留 Go API，但当前 Rust 实现实际上逐桶线性查找；找到后从该桶及所有后续累计 Count 扣减并钳到 0，命中上界时把 Repeat 清零。
- `prefixNext` 对全 `0xff` 前缀追加 0，保持与 `kv.Key.PrefixNext` 的排他上界语义。
- 浮点估计使用启发式并存在截断：分区拆桶的 Count/NDV 由 `f64` 转 `i64`，NDV 合并也会取整。它们是估计值，不能作为精确数据计数。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、后台任务或异步生命周期。`Histogram`、`Bucket`、`RowEstimate` 和加载状态均由调用方拥有；变更操作要求 `&mut self`，共享并发策略由外层统计缓存或表统计对象负责。

资源生命周期主要是内存和缓存：构造时按桶预算预分配；`PreCalculateScalar` 可重建标量缓存；`TruncateHistogram` 返回深克隆后的前缀；`Copy`/`CloneBucket` 也复制拥有的数据。`newbucket4MergingForRecycle` 与 `releasebucket4MergingForRecycle` 仅保留 Go 对象池风格接口，当前没有共享池，release 只重置计数状态。大规模分区合并会暂存所有中间桶和拆分缓冲，空间复杂度与输入桶总数相关。

## 与 Go 版本的对应关系

Rust 类型和主要 API 名称刻意保持 Go 风格，对照文件为 `pkg/statistics/histogram.go`，Rust 独立测试注释也关联 `TestTruncateHistogram`、`TestMergePartitionLevelHist`、`TestMergeBucketNDV`、`TestNewPseudoHistogramReuseChunk` 等 Go 用例。已核对的一致语义包括累计 Count、上界 Repeat、桶 NDV、交错边界、Proto 字段、TopN 从 CMS 扣减、越界三角面积以及加载/驱逐判定。

当前仍有必须显式看待的差异：

- Go 使用 chunk 保存边界，Rust 使用 `Vec<Datum>`；Rust 的 `initGlobalPseudoChunk` 是空函数，`getGlobalPseudoChunk` 每次返回空 Vec，没有 Go 的共享 chunk 池。
- Go 的估计 API从 `planctx.PlanContext`/`StatementContext` 获取调试追踪、会话时区和 `RiskRangeSkewRatio`；Rust 多数接口不接收计划上下文。Rust `BetweenRowCount` 对同桶范围只扩展 `MaxEst`，未执行 Go 中由会话偏斜比调整 `Est` 的分支；越界接口把 `allow_modify_count` 和 `skew_ratio` 作为显式参数。
- Go `ExtractTopN` 对 nil CMS 直接返回；Rust 参数是必需的 `&mut CMSketch`，类型层面排除了 nil。
- 当前 Go 分区全局直方图构建包含 `bucketRef`/最小堆等面向海量分区的低分配双遍实现；Rust `MergePartitionHist2GlobalHistWithLocation` 将全部桶物化为 Vec，并使用旧式排序、拆桶和 `bucket4Merging` 合并逻辑。两者的性能、内存占用及复杂重叠分区下的估计结果不能假定完全一致。
- Go `DecodeTo` 接收时区；Rust `DecodeTo` 仅调用 `codec::DecodeOne`。需要时区敏感 Datum 时应先核查 codec 层语义，不能仅凭同名方法认定等价。

因此扩展或修复时应同时读取两端实现和独立测试；文档只陈述当前 Rust 行为，不以 Go 的新实现替代 Rust 事实。

## 扩展指南

- 新增桶字段或改变 Count/Repeat/NDV 语义时，需要同步 `Bucket`、追加/更新/压缩、`BucketCount`、Proto 往返、`buildBucket4Merging`、两个合并入口和格式化函数，并扩展独立的 `pkg/statistics/histogram_test.rs`；不要把测试内嵌到生产文件。
- 新增 Datum 类型或排序规则时，先核查 `prepareFieldTypeForHistogram`、`compareDatum`、`scalar.rs::{PreCalculateScalar, calcFraction, convertDatumToScalar}`、`checkKind` 和 Proto 编码。风险包括错误地把不可比值视为相等，以及字节公共前缀导致的低估。
- 修改行数估计时，应分别覆盖命中上界、桶内插值、桶间隙、低/零 NDV、空直方图、无符号负范围、退化宽度和实时行数增减；保持 `MinEst <= Est <= MaxEst`。会话偏斜语义还需与 Go 的 `RiskRangeSkewRatio` 分支对齐。
- 修改 TopN 抽取时，保持候选估计和真实 CMS 计数两个阶段分离，并验证 CMS 扣减后 TopN 排序；多列索引要确保 `GetIndexPrefixLens` 解完整键，不能用 `num_columns` 截断。
- 修改分区合并时，必须区分累计桶和独立质量，保留相同上界归入同一全局桶、桶预算、总非 NULL Count、NULL/总列大小求和、列桶 NDV 清零和索引 Repeat 汇总。复杂重叠、相同边界、TopN、时区值及大分区数都应有独立测试。
- 修改加载状态时同步 `column.rs`、`index.rs`、`table.rs` 与 handle 加载/缓存路径；当前仅有全载和全驱逐两个有效值，任意中间值会由 `StatusToString` 显示为 `unknown`。
- 所有 Rust 行为改动按仓库规则先与 Go 增量和测试意图核对，修复源文件时保留版权头并同步同目录独立测试；完成后再执行适用的格式化和验证。本说明任务本身不修改 Rust 代码，也不运行 Cargo。

## 验证依据

- 生产源：`pkg/statistics/histogram.rs`（RustCodeGraph `node --file` 分段核对全部 1762 行）。主要确认了 `Histogram`/`Bucket`/`OutOfRangeShape`/`RowEstimate`/`StatsLoadedStatus`、估计入口、TopN、Proto、双层合并及辅助函数。
- 图查询：`rustcodegraph status` 确认索引包含 11467 文件、307296 节点和 1848419 边；`rustcodegraph files --filter pkg/statistics` 确认 Rust/Go 对照及测试均已索引；`query/node PreCalculateScalar` 和 `query/node calcFraction` 确认桶内插值下沉到 `pkg/statistics/scalar.rs`；对目标文件的 `node --file` 报告约 30 个使用文件。
- crate/模块：`pkg/statistics/Cargo.toml`、`pkg/statistics/lib.rs`，用于确认 crate 名、Go 包映射、依赖和重导出边界；本包无 `doc.go`。
- Rust 测试：`pkg/statistics/histogram_test.rs`，覆盖内存计量、桶定位与行数估计、Proto 往返、普通/分区合并、加载状态、V2 标准化、伪直方图、索引前缀、TopN、HistogramEqual 以及越界几何/缩放；补充调用证据来自 `pkg/statistics/main_test.rs`、`histogram_bench_test.rs`、`index.rs`、`column.rs`、`table.rs` 和 `handle/runtime_stats.rs` 的定向搜索。
- Go 对照：`pkg/statistics/histogram.go` 与 `pkg/statistics/histogram_test.go`，用于核对同名 API、累计桶语义、会话偏斜参数、TopN、全局合并实现和加载状态。Rust 测试并非覆盖 Go 文件的全部大型合并矩阵，因此复杂分区合并的完全行为等价性未在本纯文档任务中证明。
- 结构验收使用任务指定命令，要求目标文件存在且恰好包含本文 11 个固定二级标题。任务明确禁止 Cargo，本次未进行编译或运行时测试。
