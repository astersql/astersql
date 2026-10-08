# `pkg/statistics/runtime_stats_builder.rs`

## 文件定位

本文件属于 `astersql-statistics` crate。`pkg/statistics/Cargo.toml` 将 crate 根声明为 `lib.rs`，而 `pkg/statistics/lib.rs` 通过 `mod runtime_stats_builder;` 装入本模块并以 `pub use runtime_stats_builder::*;` 重导出其公开 API。因此调用者通常使用 `astersql_statistics::RuntimeStatsBuilder` 等 crate 级名称，而不是模块路径。

它是 Rust 运行时统计链路中的“类型化样本适配层”：接收会话侧已经解码成 `Vec<Vec<Option<String>>>` 的 SQL 行，把文本值恢复为带类型的 `Datum`，再复用统计 crate 已有的 `SampleCollector`、`BuildHistAndTopN` 和 `FMSketch` 实现。生产主链可由 `pkg/session/runtime/statistics.rs` 的 ANALYZE 构建阶段追到 `pkg/statistics/handle/runtime_stats.rs::BuildRuntimeTableStatsSelectionWithBuilder`，后者调用本文件的直方图、边界编码和 FM Sketch 方法。SHOW 统计值时，`pkg/session/runtime/statistics.rs::value_to_string` 又调用本文件的类型化解码入口。

本文件不是 Go 同路径文件的逐行翻译；Go 仓库没有 `runtime_stats_builder.go`。它把 Rust 会话运行时的文本行边界接到 Go 统计语义对应的底层实现，并显式补齐 Go 行编解码路径天然具备、但文本回读路径需要恢复的行为。

## 核心职责

1. `RuntimeStatsBuilder` 持有一个贯穿整批统计构建的 `stmtctx::StatementContext`，统一提供时区、宽松类型转换标志和内存跟踪器。
2. `build_histogram` / `build_histogram_with_buckets` 将列值或复合索引值转换成 `Datum`，收集 NULL、计数、NDV、样本顺序和总存储大小，最后交给 `BuildHistAndTopN` 生成规范 `Histogram` 与 `TopN`。
3. `encode_fm_sketch` 按列值或完整索引行填充 FM Sketch，并编码为可持久化字节。
4. `encode_histogram_bound` / `decode_histogram_bound` 连接内存中的直方图边界与 handle/cache 使用的持久化字节形态；BIT、普通列和索引分别走不同路径。
5. `DecodeRuntimeStatsValueWithTypes` 将已编码统计键解码成 SHOW 等展示面使用的单值或元组字符串，并对类型数量、解码数量和剩余字节做完整性检查。
6. `collector_memory`、`DropGuard`、`memory_consumed` 和 `max_memory_consumed` 为统计构建过程提供可观测且错误安全的临时内存记账。

## 主要符号

- `DropGuard<F: FnOnce()>`：私有 RAII 守卫。`new` 保存一次性回调，`Drop::drop` 通过 `Option::take` 保证回调最多执行一次。它只服务于一次 `build_histogram_with_buckets` 调用的余额清理。
- `collector_memory(&SampleCollector) -> i64`：估算收集器已持有内存，包括样本向量容量、FM Sketch、样本 `Datum` 的非空载荷、可选 CM Sketch 与 TopN；样本载荷扣除每个空 `Datum` 的固定大小，避免重复计算。
- `datum(...) -> Result<Datum, SharedError>`：私有文本到类型值转换器。`None` 生成 NULL；BIT 的合法偶数字符十六进制 `0x`/`0X` 文本直接恢复为 `BinaryLiteral`，其他值通过 `NewStringDatum(...).ConvertTo(...)` 按字段类型转换。
- `stored_value_flags(Flags) -> Flags`：私有标志修饰器，开启忽略零日期、日期内零分量和非法日期错误，模拟 Go ANALYZE 从行编码读取已落盘值时不会再次受 `sql_mode` 拦截的语义。
- `RuntimeStatsBuilder { statement_context }`：唯一公开类型。`Default` 使用默认时区；`NewWithTimeZone` 和 `NewWithTimeZoneName` 支持显式时区。三个构造路径都会设置宽松标志并初始化不限额内存跟踪器。
- `TimeZone`：返回当前构建器时区，供 handle 层创建同语义上下文或解码列值。
- `build_histogram`：默认桶数门面，委托 `build_histogram_with_buckets(..., DefaultHistogramBuckets)`。
- `build_histogram_with_buckets`：本文件的核心入口，负责类型恢复、索引键编码、采样、总大小与内存记账，再调用 `BuildHistAndTopN`。桶数通过 `buckets.max(1)` 保证至少为 1。
- `encode_fm_sketch`：列调用 `FMSketch::InsertValue`，索引调用 `InsertRowValue`，最后用 `EncodeFMSketch` 编码。
- `memory_consumed` / `max_memory_consumed`：分别暴露跟踪器当前值与历史峰值，前者在每次构建结束后应回到 0，后者用于确认实际发生过记账。
- `encode_histogram_bound`：索引边界直接取 `Datum::GetBytes`；BIT 列转换成十进制文本 Blob；其他列以构建器时区执行 `codec::EncodeKey`。
- `decode_histogram_bound`：索引边界保留为完整 bytes Datum；普通列以 `codec::DecodeOne` 解码。BIT 的十进制 Blob 逆变换需要字段长度，因此由 `pkg/statistics/handle/runtime_stats.rs::canonical_histogram` 处理，而不是本方法处理。
- `BuildRuntimeStatsHistogram`、`EncodeRuntimeHistogramBound`：使用默认构建器的公开便捷函数。目前仓库内未发现本文件外的直接 Rust 调用，主要生产链使用可保留时区的 builder 方法。
- `DecodeRuntimeStatsValue`：无字段类型提示的展示解码门面。
- `DecodeRuntimeStatsValueWithTypes`：有类型时调用 UTC 下的 `codec::DecodeRange`，无类型时调用 `codec::Decode`；最后将 NULL 格式化为 `NULL`，多值格式化为 `(a, b, …)`。

## 执行流程

直方图与 TopN 构建流程如下：

1. 调用者通常在 ANALYZE 批次开始时按会话统计时区创建 `RuntimeStatsBuilder`。串行批次复用一个实例；并行分支在每个 scoped worker 内创建各自实例，证据见 `pkg/session/runtime/statistics.rs` 的 `stats_builder` 与 worker-local `builder`。
2. `BuildRuntimeTableStatsSelectionWithBuilder` 为每列或索引整理输入行、字段类型、TopN 和桶预算，然后调用 `build_histogram_with_buckets`。
3. 方法取得首个字段类型；空 `field_types` 立即报错。随后创建 `SampleCollector::New(rows.len(), MaxSketchSize)`，计算初始内存并登记到 statement context 的跟踪器。
4. 列路径只读取每行首值。非 NULL 值用 `EncodeValue` 估算不含 flag byte 的存储大小；收集成功后把样本的 `Ordinal` 设为原始行序，用于列相关系数。额外列值会被忽略，这是当前 API 的明确边界。
5. 索引路径要求每行元素数等于 `field_types.len()`。每个分量先经 `datum` 恢复类型；所有非 NULL 分量分别 `EncodeValue` 累加存储大小，再将整行用 `EncodeKey` 编码为一个 bytes Datum，保证复合键按完整行参与比较、TopN 和 FM 哈希。
6. 每次 `Collect` 前后分别登记临时值和收集器实际持有内存的变化。任意转换、编码、收集或构建错误都通过 `?` 返回；栈展开时 `DropGuard` 释放当前余额。
7. 收集完成后，删除字节长度超过 `MaxSampleValueLength` 的样本，但保留从所有行累计的 `TotalSize`。这复现 Go `SampleCollectorFromProto` 的恢复边界，而不改变计数和总大小语义。
8. 索引直方图把字段类型设为 Blob，列直方图保留原字段类型；随后调用 `BuildHistAndTopN`，`is_column` 参数取 `!is_index`。
9. 返回前将结果对象内存加入临时账本；守卫随函数退出释放全额，所以结果所有权交给调用者后 builder 的当前消耗仍为 0。

FM Sketch 流程复用相同的 `datum` 与行形状校验，但不经过 `SampleCollector`：列逐值 `InsertValue`，索引逐行 `InsertRowValue`，最终序列化。边界展示流程则从 handle 层保存时调用 `encode_histogram_bound`，在分区合并时由 `canonical_histogram` 恢复，在 SHOW 路径由 `DecodeRuntimeStatsValueWithTypes` 格式化。

## 数据与状态

- builder 的持久状态只有装箱的 `StatementContext`。时区、类型标志和 `MemTracker` 在构造后跨多次方法调用复用；直方图、收集器、Datum 向量和 FM Sketch 都是单次调用的局部状态。
- `Cell<i64>` 保存一次构建的未释放记账余额。它允许两个闭包在共享借用下更新数值，但不是跨线程同步原语。
- `SampleCollector` 汇总 `Count`、`NullCount`、FM Sketch、样本及 `TotalSize`。列相关系数依赖非 NULL 样本的原行 `Ordinal`；NULL 仍占原始序号，但不产生 `SampleItem`。
- `total_size` 表示行编码的值载荷：每个非 NULL 值调用 `EncodeValue` 后减去一个 flag byte。复合索引按每个非 NULL 分量累加，而不是使用最终可比较索引键的总长度；该选择与 Go 行采样器语义对齐。
- 直方图索引边界始终是完整编码键 bytes；列边界保留类型化 Datum，落盘时才根据 BIT/普通列分支编码。
- `DecodeRuntimeStatsValueWithTypes` 不修改 builder 状态，是独立公开函数；类型化解码固定使用 UTC，因为其输入是已规范化的持久化键编码，而非会话文本值。

## 依赖与调用关系

上游直接调用关系：

- `pkg/session/runtime/statistics.rs` 创建带 `stats_time_zone` 的 builder，并把它传给 `pkg/statistics/handle/runtime_stats.rs::BuildRuntimeTableStatsSelectionWithBuilder`。并行 ANALYZE 每个 worker 独享 builder；动态分区全局统计合并使用单独的 `merge_builder`。
- `pkg/statistics/handle/runtime_stats.rs` 在列和索引分支调用 `build_histogram_with_buckets` 与 `encode_fm_sketch`，用 `encode_histogram_bound` 生成缓存桶边界；分区统计合并时调用 `decode_histogram_bound` 或类型化列解码重建规范直方图。
- `pkg/session/runtime/statistics.rs::value_to_string` 调用 `DecodeRuntimeStatsValueWithTypes`，形成 SHOW 统计值的显示字符串。

主要下游依赖：

- crate 内：`SampleCollector`、`BuildHistAndTopN`、`Histogram`、`TopN`、`NewFMSketch`、`EncodeFMSketch`、`DefaultHistogramBuckets`、`MaxSketchSize` 和 `MaxSampleValueLength`。
- `stmtctx`：statement context、类型上下文、时区与内存跟踪。
- `types`：`FieldType`、`Datum`、BIT `BinaryLiteral`、类型转换及内存估算。
- `codec`：值/键编码及通用、范围、单 Datum 解码。
- `chrono-tz`：命名时区解析；`astersql-errors`：统一 `SharedError`。

`pkg/statistics/Cargo.toml` 直接声明上述本地 crate 依赖，并以 `[package.metadata.porting] go-package = "pkg/statistics"` 标明 Go 对照包。模块没有条件编译项；测试装配位于 crate/handle 的独立 `*_test.rs` 文件中。

RustCodeGraph 文件索引报告本文件含 34 个符号，并被多个生产/测试文件引用；精确名称查询确认核心上游包括 `pkg/statistics/handle/runtime_stats.rs` 的构建与合并函数，以及 `pkg/session/runtime/statistics.rs` 中的 builder 实例。对重名方法的通用 `callers/callees` 查询未能稳定消歧，以上边由文件级图读取与仓库引用搜索共同核验。

## 错误处理与边界

- 时区名称解析失败返回 `invalid statistics time zone: ...`。
- 直方图或 FM Sketch 构建在字段类型为空时返回 `runtime statistics requires a field type`。
- 索引行宽与字段类型数不一致时返回 `runtime index value and field type counts differ`；列路径只消费首值，不要求行宽恰为 1。
- BIT 仅在 `0x`/`0X` 后为偶数个十六进制字符时走二进制快速路径；其他形式回退到普通字段类型转换。转换、编码、采样、直方图和 FM Sketch 错误均转换或传播为 `SharedError`。
- 桶数 0 不会下传，而被规范化为 1；TopN 数量没有在本文件内改写。
- 超长样本仅从用于桶/TopN 的 `Samples` 中删除，不回退已累计的 Count、NDV 或 TotalSize。扩展此处时必须维持 Go 序列化后恢复的同一边界。
- `encode_histogram_bound` 对越界的 bound 索引返回 `histogram bound index out of range`。
- 类型化展示解码要求 `field_types.len() == expected_values`，要求 `DecodeRange` 消耗全部字节，并要求实际 Datum 数与预期相等；任一条件不满足均报带数量信息的错误。
- BIT 落盘边界是十进制 Blob，而 `decode_histogram_bound(false)` 只适用于 `EncodeKey` 形态。带字段类型的 BIT 恢复由 handle 层 `canonical_histogram` 完成，调用者不能把两条解码路径互换。
- `DropGuard` 保障 Rust 错误返回和 panic 展开时释放账本；若底层跟踪器缺失，记账闭包直接无操作。构造函数当前总会初始化跟踪器。

## 并发与资源生命周期

`RuntimeStatsBuilder` 没有内部锁，且构建过程使用 `Cell<i64>`；它应当按当前生产用法在单线程内复用，而不是由多个线程共享同一实例。会话 ANALYZE 的并发分支在 `std::thread::scope` 中为每个 worker 创建独立 builder，避免共享 statement context、类型转换状态或内存账本。动态分区合并也另建 builder，与样本构建阶段隔离。

一次直方图调用的临时资源生命周期由栈管理：Datum 向量和编码缓冲在循环迭代内释放，`SampleCollector` 活到 `BuildHistAndTopN` 完成，`DropGuard` 最后释放跟踪器中的未结余额。返回的 `Histogram` 与 `TopN` 由调用者拥有；本文件只将其内存纳入峰值观察，不把这部分消耗永久挂在 builder 上。

没有异步任务、通道、事务或 I/O 资源。唯一外部可观察资源状态是 statement context 内存跟踪器；测试 `runtime_collector_releases_exact_input_memory_on_success_and_error` 证明正常和转换错误路径结束后当前消耗均为 0，且峰值确实增长。

## 与 Go 版本的对应关系

- Rust 最终调用的 `BuildHistAndTopN` 对应 `pkg/statistics/builder.go::BuildHistAndTopN`：二者都从 `SampleCollector` 的 count、NDV、NULL、样本与 TotalSize 构建 Histogram/TopN，并区分列值比较编码与索引 bytes 比较。
- `MaxSampleValueLength` 截断对应 `pkg/statistics/sample.go::SampleCollectorFromProto`。Go 在反序列化 collector 时丢弃过长样本；Rust 运行时不经过 protobuf 往返，因此在调用 builder 前显式 `retain`，同时保留完整 TotalSize。
- `stored_value_flags` 是 Rust 文本回读路径的适配。Go ANALYZE 样本从 row codec 直接得到 Datum，不会对已接受并落盘的零日期/非法日期再次执行 SQL mode 校验；Rust 因输入是字符串，必须显式放宽这些错误标志。
- BIT 的 `0x…` 快速恢复是 Rust 会话 DML 文本形态与 Go Datum 形态间的桥接，避免整数 0 与文本 `"0"` 在中间表示上混淆。
- `encode_histogram_bound` 的 BIT 分支对应 `pkg/statistics/handle/storage/save.go::convertBoundToBlob`；Go 转成 Blob 时把可表示的 BIT 格式化为十进制。逆过程对应 `pkg/statistics/handle/storage/read.go::convertBoundFromBlob`，需要字段 `flen`，因此落在 Rust handle 层 `canonical_histogram`。
- 对非 BIT 列，当前 Rust 仍保存 `EncodeKey` 字节，以兼容既有 SHOW/解码消费者；源码注释明确这是尚未整体迁移到 Go `convertBoundFromBlob` 语义的兼容边界。不能把它描述成与 Go 所有类型完全同形。
- 复合索引的 FM Sketch 使用 `InsertRowValue`、直方图使用完整 `EncodeKey`；列使用单 Datum。这与 Go `row_sampler.go` 对列 FM Sketch 和 column group/index 行 FM Sketch 的区分一致。
- 当前 Rust API 还提供默认 builder 的便捷函数和展示字符串解码，这是 Rust 运行时集成所需接口，不存在同名 Go API。

## 扩展指南

- 新增可接受输入类型或文本形态时，优先修改私有 `datum`，并同步验证列与复合索引两条路径、会话时区及 SQL mode 已落盘值语义；不要在 handle 层重复实现转换。
- 修改样本大小、TotalSize 或内存估算时，应同时审查 `collector_memory`、循环中的 `account` 调用、`SampleCollector::Collect`、`MaxSampleValueLength` 与 Go `sample.go`。错误返回和 panic 展开必须继续保证 `memory_consumed() == 0`。
- 修改边界持久化格式时，必须成对检查 `encode_histogram_bound`、`decode_histogram_bound`、`pkg/statistics/handle/runtime_stats.rs::canonical_histogram`、SHOW 的 `DecodeRuntimeStatsValueWithTypes` 以及 Go `convertBoundToBlob`/`convertBoundFromBlob`。这是兼容已有缓存/持久化数据的高风险变更。
- 新增索引编码行为时，保持严格的行宽检查，并明确 NULL、前缀索引、BIT、时间类型和复合键排序语义；索引 Histogram 的字段类型应继续是 Blob，除非所有下游一起迁移。
- 并发扩展应继续采用“每 worker 一个 builder”。若需要共享，必须先证明 `StatementContext`、内存跟踪器及所有方法满足同步要求，不能仅在外层包一个引用。
- 测试应继续放在独立文件，而不是内嵌本源文件。直接行为优先扩展 `pkg/statistics/handle/analyze_runtime_aster_unit_test.rs`；边界重建和分区合并扩展 `pkg/statistics/handle/runtime_stats_test.rs`；统计 handle 的查询行为可扩展 `pkg/statistics/handle/handletest/handle_test.rs`。Go 语义变更还应对照 `pkg/statistics/sample_test.go`、`fmsketch_test.go` 和 handle storage 测试。
- 性能风险集中在每值字符串转换、复合索引为 TotalSize 额外执行的逐分量 `EncodeValue`、完整键编码和 `collector_memory` 每行扫描现有样本。优化必须保持记账、样本顺序和 Go 兼容结果，不应以删减逻辑换取速度。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/statistics/runtime_stats_builder.rs` 报告 34 个符号；`node --file ... --offset 1/261` 完整读取 509 行源码；精确 `query` 命中 `RuntimeStatsBuilder`、`BuildRuntimeStatsHistogram`、`DecodeRuntimeStatsValueWithTypes` 及四个核心方法，并定位到 handle/session 上游。通用未限定方法名的 `callers/callees` 未能产生可消歧结果，因此调用边又以目标文件级图读取和 `rg` 引用搜索复核。
- Rust 源与 crate 边界：`pkg/statistics/runtime_stats_builder.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`、`pkg/statistics/handle/runtime_stats.rs`、`pkg/session/runtime/statistics.rs`。
- 独立 Rust 测试：`pkg/statistics/handle/analyze_runtime_aster_unit_test.rs` 验证时区敏感边界、文本排序规则、复合索引编码/TopN、解码数量错误和成功/失败内存释放；`pkg/statistics/handle/runtime_stats_test.rs` 验证分区合并、BIT/ENUM/SET/时间边界重建；`pkg/statistics/handle/handletest/handle_test.rs` 以解码后的桶和 TopN 执行行数估算。
- Go 对照：`pkg/statistics/builder.go::BuildHistAndTopN`、`pkg/statistics/sample.go::{SampleCollectorFromProto, SampleCollector.collect, MaxSampleValueLength}`、`pkg/statistics/row_sampler.go`、`pkg/statistics/fmsketch.go`、`pkg/statistics/handle/storage/save.go::convertBoundToBlob`、`pkg/statistics/handle/storage/read.go::convertBoundFromBlob`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付验证以固定章节结构、源码链接和人工事实复核为准。
