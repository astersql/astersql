# `pkg/dumpformat/parquetfile/file_parser.rs`

## 文件定位

本文件是 `astersql-dumpformat-parquetfile` crate 中面向真实 Parquet 数据页的流式行解码器，也是 Parquet 解码结果接入 Lightning/mydump `Parser` 接口的适配层。模块由 `pkg/dumpformat/parquetfile/lib.rs` 公开为 `file_parser`；crate 边界、`parquet`、`chrono`、`chrono-tz` 与 `astersql-lightning-mydump` 依赖见 `pkg/dumpformat/parquetfile/Cargo.toml`。

在应用链路中，上游先把对象存储或本地数据包装成 `SourceReader`，再构造 `FileParser`，最后按需要包成 `ImportParser`。直接生产调用包括 `lightning/pkg/importer/chunk_process.rs` 的分块导入恢复、`lightning/pkg/importer/get_pre_info.rs` 的预检查采样，以及 `pkg/executor/importer/import.rs::{OpenParquetFile, open_parquet_file}`。因此本文件不是通用 schema 构造器，也不负责打开对象存储连接；它负责在已经具备随机范围读取能力的源上校验 schema、逐行解码，并维持导入检查点语义。

## 核心职责

1. `normalized_logical_type` 将 Parquet 新式 logical annotation 与旧式 converted type 归一化，并调用 `validate_parquet_logical_type` 拒绝当前解码器不支持或与物理类型不匹配的声明。
2. `FileParser::{new, new_with_location}` 打开 Parquet metadata，拒绝嵌套/重复字段，建立小写列名、row-group/column 范围和 Spark legacy 时间重基准信息。
3. `Column::next` 以有限批次调用 parquet-rs `read_records`，把六类物理列读取器转换成内部 `Datum`，同时还原 definition level 表示的 NULL。
4. `FileParser::read_row` 让各列同步前进一行，跨 row group 重建列读取器，并更新读取行数和行 ID。
5. `ImportParser` 将内部 `Datum` 转为 `astersql_lightning_mydump::Datum`，实现位置、扫描进度、行回收、列名及关闭接口。
6. `FileParser::adjust_memory_estimate` 在整文件预加载路径上，用共享整文件缓冲大小替换原先第一 row group 的预加载计费，避免重复计算。

本文件只支持扁平标量列；`FileParser::new_with_location` 明确拒绝 `max_rep_level() > 0` 或路径层级不为一的 schema，`validate_parquet_logical_type` 还拒绝 List、Map、Float16 和 Variant。

## 主要符号

- `normalized_logical_type(&ColumnDescriptor) -> Result<Option<LogicalType>>`：私有兼容桥。有效 logical type 优先；缺失或 `_Unknown` 时，将 UTF8、ENUM、JSON、BSON、DECIMAL、DATE、TIME、TIMESTAMP 和有/无符号整数 converted type 映射为新式类型。无法映射的 legacy annotation 返回带列名的错误。
- `validate_parquet_logical_type(...) -> Result<()>`：crate 内可见的 schema 守卫。先做当前实现的范围限制，再借助 parquet-rs primitive builder 校验物理宽度、定长长度、decimal precision/scale 等组合。
- `Column`：单列运行态，持有 `ColumnReader`、批大小、`ParquetColumnType`、可空性、时区与 `VecDeque<Result<Datum>>`。它不对外暴露。
- `Column::next() -> Result<Datum>`：列级拉取入口。缓存空时至多读取 `batch_size` 条 record，将转换结果（包括逐值错误）排入 `rows`，随后弹出一项。
- `FileParser`：文件级解码状态。公开观测字段为 `read_rows`、`row_id`；其余状态包含共享 `SourceReader`、`SerializedFileReader`、当前各列、列名、row-group 游标、内存估算数据、时区及各列类型信息。
- `FileParser::{new, new_with_location}`：构造入口；`new` 固定使用 `UTC`，空 location 也回退到 `UTC`。
- `FileParser::{set_batch_size, read_row, columns, column_is_utf8, total_rows, adjust_memory_estimate, source, close}`：分别控制解码批量、读行、schema 查询、UTF-8 判定、总行数、内存估算、源访问与显式释放。
- `FileParser::build_group()`：按当前 `group` 构建所有列读取器和 `ParquetColumnType`，并依据文件 metadata 初始化 Spark DATE/TIMESTAMP/INT96 rebase lookup。
- `ImportParser`：公开适配器，拥有 `FileParser`、最后一行、可配置列名和可复用行向量池。
- `mydump_error`：仅把精确字符串 `"EOF"` 映射为 `MydumpError::Eof`，其余内部错误映射为 `MydumpError::Io`。
- `datum_row_size`：NULL 计 0 字节、字节串按实际长度、其余内部值统一按 8 字节估算。

## 执行流程

构造阶段从 `FileParser::new_with_location` 开始：解析时区；通过 `parser::open_file_reader` 创建 `SerializedFileReader<SourceReader>`；读取 schema 并拒绝嵌套/重复列；归一化和验证每列 logical type；把列名转为小写。随后它从 metadata 组装 `reader_wrapper::FileMeta`，包括老版本 parquet-mr 标志及各 column chunk 的 data/dictionary offset 和压缩大小，经 `row_group_range_from_meta` 计算 row-group 与 column 范围，再调用 `SourceReader::set_ranges` 配置后续预加载/流式范围。若存在 row group，立即调用 `build_group`。

`build_group` 清空上一组的列和类型信息，取得当前 row group，重置 `group_rows/group_read`。每列重新归一化类型；对于 INT96、DATE 及非 NANOS TIMESTAMP，根据 `created_by`、key-value metadata、Spark 版本截止点和解析时区选择 legacy rebase zone，并构建 `SparkRebaseMicrosLookup`。最后取得 parquet-rs 的类型化 `ColumnReader`，连同可空性、批大小和类型信息放入 `columns`。

每次 `read_row` 先检查 `closed`。当前组耗尽时清空列、递增组号；文件末尾返回 `"EOF"`，否则重建下一组。然后依次调用所有 `Column::next`，只有整行全部成功后才递增 `group_read`、`read_rows` 和 `row_id`。`Column::next` 在本列缓存为空时按批读取：definition level 为 0 生成 `Datum::Null`；非 NULL 值按物理类型分派到 `type_converter`；INT96 先按小端字节重建微秒值，再按解析时区补偿 UTC offset。缓存中若保存了转换错误，该错误只在对应行被弹出时暴露，而不会提前破坏更早的 NULL 行。

导入链路调用 `ImportParser::ReadRow`：先递增最后一行的 row ID 并清零长度，再读内部行、计算估算长度，从 `row_pool` 取或新建缓冲，把内部值转换为 mydump 值。整数保留为 `I64`；无符号数、浮点、decimal、duration 和时间格式化为字节串；DATE 输出 `YYYY-MM-DD`，其他时间输出到微秒。成功后替换 `last.row`。调用者通过 `LastRow` 取得克隆，并可用 `RecycleRow` 把其中的向量放回池。

## 数据与状态

`FileParser` 同时维护三组进度：`group/group_rows/group_read` 控制当前 row group；`read_rows` 是已成功读出的全文件行数；`row_id` 是内部顺序行号。`ImportParser` 另有 `last.row_id`，用于 mydump 检查点接口。`Pos` 返回 `(inner.read_rows, last.row_id)`；`SetPos(pos, row)` 通过反复调用 `read_row` 只向前消费 `pos - last.row_id` 次，再直接设置外部行 ID，不支持真正的 seek 或回退。

`infos` 与 `columns` 按 schema 列序一一对应，`ImportParser::ReadRow` 依靠同一 index 判断时间值应格式化为 DATE 还是 TIMESTAMP。`names` 在构造时取 schema 小写名；`ImportParser` 拷贝一份列名，因此 `SetColumns` 只改变适配层返回的名字，不改变底层 schema 或解码顺序。

`Column::rows` 的元素是 `Result<Datum>` 而不是纯值，使一个批次内某个后续值的转换失败可以延迟到那一行。其规模至多受 `batch_size` 和已消费速度控制；默认批大小来自 `parser::READ_BATCH_SIZE`。`row_pool` 复用 mydump datum 向量，但 `LastRow` 会克隆 `Row`，调用者仍应按接口调用 `RecycleRow` 以回收容量。

`ScannedPos` 不读取底层 reader 的真实字节游标，而用 `read_rows / total_rows * source.len()` 估算；空文件或恰好读完时返回完整文件大小。这避免预加载和预读导致的进度跳跃。

## 依赖与调用关系

上游生产边为：

- `lightning/pkg/importer/chunk_process.rs`：`parquet_source -> FileParser::new -> ImportParser::new -> SetPos`，把适配器装入分块导入的 `DataParser`。
- `lightning/pkg/importer/get_pre_info.rs`：构造解析器后读取 `columns/column_is_utf8`，再通过 `ImportParser` 采样预检查行。
- `pkg/executor/importer/import.rs`：`open_parquet_file` 创建带对象存储 range opener 的 `SourceReader`，调用 `FileParser::new_with_location`；`OpenParquetParser*` 再包装成 `ImportParser`。

主要下游边为：

- `parser::open_file_reader` 与 parquet-rs `SerializedFileReader/FileReader/ColumnReader`：metadata、row group 和批式物理值读取。
- `reader_wrapper::row_group_range_from_meta` 与 `SourceReader::{set_ranges, whole_file_preloaded, buffer_bytes, close}`：范围策略、共享预加载缓冲和资源释放。
- `type_converter::{convert_logical_int32, convert_logical_int64, convert_logical_bytes, int96_micros_with_rebase}`：物理值到内部 `Datum` 的语义转换。
- `spark_rebase::{AppVersion, spark_rebase_time_zone_id, SparkRebaseMicrosLookup}`：Spark legacy 日历与时区兼容。
- `astersql_lightning_mydump::{Parser, Row, Datum, MydumpError}`：导入器可消费的统一接口。

RustCodeGraph 索引将本文件识别为 44 个符号，并显示其被 `parser.rs`、`parser_test.rs`、`type_converter.rs`、`pkg/dumpformat/testutils/parquet_writer.rs`、`pkg/executor/importer/import.rs` 等文件使用；精确仓库搜索补全了 Lightning 两个生产调用点。Cargo 声明中的 parquet 依赖固定到 `astersql-parquet-v60.0.0-streaming-pages.1` tag，页流式行为和 API 兼容性因此受该上游版本约束。

## 错误处理与边界

构造阶段会拒绝无效时区、嵌套/重复字段、不支持的 converted/logical type、logical/physical 不匹配、无效 column chunk 范围、SourceReader 范围配置失败以及 Spark rebase lookup 创建失败。parquet-rs 和 IO 类错误通常由局部 `error` 转成字符串包装的 crate `Error`，因此调用者不应依赖底层具体错误类型。

读取阶段的关键边界是：已关闭返回 `"parser is closed"`；文件或 row group 结束最终返回精确 `"EOF"`；parquet 声称有 record 却缺少物理 value 时返回 `"missing parquet column value"`；INT96 时区换算越界和 mydump 时间格式化越界返回 `"timestamp out of range"`。`set_batch_size(0)` 被拒绝。`adjust_memory_estimate` 使用 checked arithmetic，并拒绝下溢、上溢或负结果。

`LogicalType::Unknown`（Parquet NULL annotation）允许 definition level 表示的实际 NULL，但遇到非 NULL 物理值时由 `Column::next` 返回 unsupported logical 错误。`parser_test.rs::logical_null_later_non_null_value_does_not_fail_an_earlier_null_row` 验证了错误按行延迟的行为。

需注意 `ImportParser::ReadRow` 在调用底层读取前就递增 `last.row_id`；若底层返回 EOF 或其他错误，外部 row ID 已前进，这与其所对齐的 Go 接口行为一致。`SetPos` 的差值以 `last.row_id` 计算，调用者必须提供单调且语义一致的检查点；负差值不会回退读取位置。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道；`FileParser` 和 `ImportParser` 通过 `&mut self` 串行推进。与 Go `Parser::buildRowGroupParser` 用上限 8 的 error group 并行创建各列 reader 不同，Rust `build_group` 顺序创建列读取器。不要在没有额外同步和语义设计的情况下让多个消费者共享同一解析器。

内存生命周期分三层：`SourceReader` 拥有可能共享的整文件/row-group 压缩预加载；`SerializedFileReader`/`ColumnReader` 持有 parquet 解码状态；每个 `Column` 只缓存一个有限批次的已转换行。跨 row group 时 `columns.clear()` 释放上一组列解码器；`close` 清空列缓存、调用 `SourceReader::close` 并设置 `closed`，测试 `tests/whole_file_preload.rs::closing_releases_shared_preload_and_range_errors_remain_errors` 覆盖共享预加载释放。该类型没有 `Drop` 实现，要求拥有者遵守 mydump `Close` 生命周期；`ImportParser::Close` 负责转调。

整文件预加载时，`adjust_memory_estimate` 仅调整计费，不迁移或复制缓冲：从既有峰值扣除构造时记录的第一 row-group 预加载值，再加入 `SourceReader::buffer_bytes`。`tests/whole_file_preload.rs::whole_buffer_replaces_row_group_charge_without_changing_decoder_estimate` 验证其避免双重计费；`tests/page_streaming_peak.rs::importer_decoder_streams_large_pages_with_one_row_batches_and_bounded_peak` 验证大页在一行批次下仍保持受限峰值。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/parquetfile/parser.go`。Rust 的 `FileParser` 对应 Go `Parser` 的 schema 校验、row-group 推进、列批读、行数/进度、Spark rebase 和预加载内存语义；Rust `ImportParser` 则把这些能力适配回 Go 侧 `parsedef.Parser`/mydump 风格接口。

保持一致的关键行为包括：列名转小写；拒绝 nested/repeated schema；logical type 优先、legacy converted type 回退；`SetPos` 通过读弃而不是随机 seek；`ScannedPos` 按已读行比例估算；读行前递增 RowID；NULL/字节串/其他值的行大小估算；跨 row group 与 EOF；DATE、TIMESTAMP、INT96 的 Spark legacy 处理；整文件预加载替换第一 row-group 内存计费。

当前实现差异必须在扩展时显式评估：

- Go `Init(nil)` 使用系统时区，Rust `new` 和空 location 使用 `UTC`；需要本地时区语义的调用者必须用 `new_with_location`。
- Go 为每列创建独立 reader，并最多并发 8 个；Rust 在同一个 `SerializedFileReader<SourceReader>` 上顺序取得列读取器。
- Go `SetColumns` 是 no-op，Rust `ImportParser::SetColumns` 会替换适配器持有的列名。
- Go row pool 预先按列数分配 `types.Datum` 槽，Rust pool 保存可变长 `Vec<mydump::Datum>` 并在复用前 `clear`。
- Go `Close` 还关闭 allocator/row-group readers 并聚合 close error；Rust `close` 无返回值，只清列并关闭共享 source。

这些差异是当前源码事实，不表示应自动改成任一侧；任何行为调整都应先用对应 Go 测试和 Rust 独立测试确认兼容目标。

## 扩展指南

新增 logical/physical 类型时，至少同时检查 `normalized_logical_type`、`validate_parquet_logical_type`、`Column::next`、`type_converter.rs`、`ImportParser::ReadRow` 和 `datum_row_size`。新增类型若只加入 schema 映射而没有列读取与 mydump 输出分支，会在构造或读行阶段出现不完整支持。测试应放在独立的 `pkg/dumpformat/parquetfile/parser_test.rs` 或更聚焦的新 `*_test.rs`，不要内嵌到本生产文件；同步参考 `parser_test.go` 的同语义用例。

改变时间语义时，要分别覆盖 DATE、TIME、TIMESTAMP（MILLIS/MICROS/NANOS）、INT96、`is_adjusted_to_utc`、负 epoch、DST 转换以及 Spark legacy metadata。主要现有锚点是 `parser_test.rs::{logical_nanos_fixture_reaches_import_parser_with_midnight_carry, logical_time_preserves_duration_and_wraps_only_utc_adjusted_wall_clock, logical_timestamp_nanos_skips_spark_rebase_and_preserves_adjustment, logical_int96_without_annotation_keeps_utc_adjustment_and_rounding, logical_timestamp_nanos_rounding_observes_the_dst_transition_offset}`。

改变 row-group、批大小或预加载策略时，应保持“最多当前 row group 加每列一个有限批次”的资源边界，复核 `SourceReader::set_ranges` 与 `reader_wrapper::row_group_range_from_meta` 的 offset 不变量，并运行 `tests/page_streaming_peak.rs` 和 `tests/whole_file_preload.rs`。改变检查点语义时，应同步验证 `Pos/SetPos/ScannedPos/ReadRow/SetRowID`，尤其是错误后 RowID 和只向前读弃行为。

兼容风险主要来自 Go/Rust 默认时区、错误字符串到 `MydumpError::Eof` 的精确映射及 `SetColumns` 差异；性能风险主要来自批大小、每列缓存、列读取器构建顺序和预加载计费。外部 parquet 依赖若升级，必须重新验证 logical type builder、页流式读取和 column reader API，而不能仅以编译通过作为行为证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/dumpformat/parquetfile` 确认目标及相邻 Rust/Go/测试文件；`node --file pkg/dumpformat/parquetfile/file_parser.rs --offset 1 --limit 500` 与 `--offset 480 --limit 160` 读取完整 588 行；`query FileParser`、`query ImportParser`、`query normalized_logical_type`、`query validate_parquet_logical_type` 核对核心符号和 importer 入口。通用 `explore` 返回大量同名噪声，未将其不相关 blast-radius 结果作为结论。
- 源码与声明：`pkg/dumpformat/parquetfile/file_parser.rs`、`lib.rs`、`Cargo.toml`、`parser.rs`、`source_reader.rs`、`reader_wrapper.rs`、`type_converter.rs`、`spark_rebase.rs`。
- 生产调用：`lightning/pkg/importer/chunk_process.rs`、`lightning/pkg/importer/get_pre_info.rs`、`pkg/executor/importer/import.rs`。
- Go 对照：`pkg/dumpformat/parquetfile/parser.go`，重点是 `Parser`、`Init`、`buildRowGroupParser`、`getBuilder`、`moveToNextRowGroup`、`readSingleRow`、`SetPos`、`ScannedPos`、`Close`、`ReadRow`、`NewParser`、`preloadBufferBytes` 与 `EstimateParquetReaderMemory`；行为测试来源为 `pkg/dumpformat/parquetfile/parser_test.go`。
- Rust 独立测试：`pkg/dumpformat/parquetfile/parser_test.rs` 覆盖字典跨批、跨 row group、检查点/进度、logical type、NULL、decimal、时间和 Spark rebase；`pkg/dumpformat/parquetfile/tests/whole_file_preload.rs` 覆盖读取策略、真实值/EOF、内存估算及关闭；`pkg/dumpformat/parquetfile/tests/page_streaming_peak.rs` 覆盖大页流式解码峰值。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。
