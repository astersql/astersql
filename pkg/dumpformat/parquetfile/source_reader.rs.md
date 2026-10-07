# `pkg/dumpformat/parquetfile/source_reader.rs`

## 文件定位

本文件是 `astersql-dumpformat-parquetfile` crate 的对象存储读取适配层。它把“按半开区间 `[start, end)` 打开独立字节流”的 `RangeOpener` 转成 parquet-rs 所需的 `Length` 与 `ChunkReader`，使 `SerializedFileReader<SourceReader>` 能用同一接口读取 footer、列块和数据页。模块由 `lib.rs` 公开为 `source_reader`；crate 边界与依赖见 `pkg/dumpformat/parquetfile/Cargo.toml`，其中 `bytes` 提供零拷贝切片，带 tag 的 AsterSQL `parquet` 分支提供流式 page reader。

生产入口主要有两处：`pkg/executor/importer/import.rs::open_parquet_file` 将 `Storage::Open` 包装为带取消检查的范围读取器；`lightning/pkg/importer/chunk_process.rs::parquet_source` 将 Lightning `Storage::OpenRange` 与 `FileSize` 包装为同一接口。两者构造 `SourceReader` 后交给 `pkg/dumpformat/parquetfile/file_parser.rs::FileParser`，后者再经 `parser.rs::open_file_reader` 进入 parquet-rs 的流式解码路径。

## 核心职责

- 在 `prepare`/`prepare_with_thresholds` 中决定是否一次性预载整个已知大小的小文件；默认上限 `WHOLE_FILE_THRESHOLD` 为 32 MiB。
- 在 metadata 已解析后，由 `set_ranges` 接收 row group 范围和各 column chunk 范围；访问小 row group 时按需共享预载，默认上限 `ROW_GROUP_THRESHOLD` 为 128 MiB。
- 对未整文件/row-group 缓存覆盖的列读取，在 `get_read` 中保留至多 `parser::DEFAULT_BUFFER_SIZE`（64 KiB）的前缀，使 page header 探测和紧随其后的 payload 读取能复用一次请求。
- 实现 parquet-rs 的 `Length` 和 `ChunkReader`，同时提供缓存占用、峰值与关闭接口，供 `FileParser::adjust_memory_estimate` 和资源回收使用。

这些策略只改变字节取得方式，不解释 Parquet 类型或行值；schema 校验、row-group 切换和 Datum 转换属于 `file_parser.rs`。

## 主要符号

- `WHOLE_FILE_THRESHOLD: u64 = 32 << 20`：整文件预载的默认闭区间上限，文件大小等于阈值时也预载。
- `ROW_GROUP_THRESHOLD: u64 = 128 << 20`：row group 共享缓存的默认闭区间上限。
- `RangeOpener`：`Arc<dyn Fn(u64, u64) -> parquet::Result<Box<dyn Read>> + Send + Sync>`。调用者必须为每个半开区间返回独立、可精确读满该区间的流。
- `Buffer`：缓存的绝对起止偏移及 `Bytes` 内容；`Bytes::slice` 允许返回共享底层分配的子视图。
- `State`：互斥保护的可变状态，包括关闭标志、整文件/当前 row group/短前缀三个缓存、已登记范围、row-group 阈值和记录到的峰值。
- `SourceReader`：持有不可变 `size`、范围打开器和 `Arc<Mutex<State>>`。`Clone` 只复制句柄，所有克隆共享缓存和关闭状态。
- `load`：校验 `end - start`、转换为 `usize`、调用 opener 并 `read_exact`，然后构造 `Buffer`；短读不会被静默接受。
- `prepare`：使用默认阈值调用 `prepare_with_thresholds`。
- `prepare_with_thresholds`：可注入阈值的真实构造器，也是独立测试切换三种策略的入口。
- `set_ranges`：登记 row-group 与列范围，并拒绝反向或超出文件大小的范围。
- `close`、`whole_file_preloaded`、`buffer_bytes`、`peak_buffer_bytes`：生命周期与观测接口。`buffer_bytes` 只报告 whole/group 二者之一，不计短前缀 `stream`；`peak_buffer_bytes` 同样只跟踪整文件或 row-group 预载。
- `cached`：检查关闭状态，按需切换当前 row-group 缓存，并从 whole、group、stream 中依次寻找完整覆盖请求的切片。
- `Length::len`、`ChunkReader::{get_read,get_bytes}`：parquet-rs 实际调用的读取协议实现。

## 执行流程

1. 上游构造 `RangeOpener`，并把可信的 `known_size` 或延迟 `discover_size` 传给 `prepare`。当 `known_size > 0` 且不大于整文件阈值时，构造阶段直接读取 `[0, known_size)`；成功后不会调用 `discover_size`。大小未知、非正数或超过阈值时不做整文件预载，而是调用 `discover_size` 得到精确长度。最终长度超过 `i64::MAX` 会失败。
2. `FileParser::new_with_location` 先把 `SourceReader` 克隆给 `parser::open_file_reader`。parquet-rs 借助 `Length` 和 `ChunkReader` 读取 footer/metadata；`open_file_reader` 显式启用 page streaming。
3. `FileParser` 从 metadata 计算每个 row group 的总范围和每个 column chunk 的范围，然后调用 `SourceReader::set_ranges`。因此构造早期的 footer 请求可以工作，而后续列读取才获得范围感知缓存。
4. `get_bytes(start, length)` 先用 checked add 计算结尾，拒绝溢出和越界；零长度立即返回空 `Bytes`。非空读取先询问 `cached`，未命中才精确 `load(start, end)`。
5. `get_read(start)` 先处理越界和恰好位于 EOF 的空流，再从 `column_ranges` 推导本次流的终点（列内请求止于该列尾，否则止于文件尾）。若已有缓存从 `start` 覆盖到终点，直接返回 `Cursor<Bytes>`；仅覆盖前缀时，用 `Cursor::chain` 拼接后续范围流。
6. 未命中缓存且请求不在列范围内时，直接打开 `[start, end)`。若在列范围内，则从新流预读最多 64 KiB，保存为 `stream`，返回“缓存前缀 + 剩余原流”的链；这兼顾小探测复用和大 page 流式消费。
7. `cached` 在没有 whole 缓存时查找包含 `start` 的首个 row-group 范围。若该组不超过阈值且不是当前组，它先释放旧组，再加载新组并更新峰值。后续读取共享同一 `Bytes`，但每次 `get_read` 都拥有独立 `Cursor`。
8. `FileParser::close` 调用 `SourceReader::close`；共享状态被标为关闭，同时释放 whole、group、stream 缓存。此后普通非空缓存路径返回 `reader is closed`。

## 数据与状态

`size` 与 `open` 在构造后不变；所有会随读取变化的数据都位于共享 `State`。缓存优先级是 `whole -> group -> stream`：整文件缓存存在时不会再加载 row group；group 缓存一次只保留一个范围；stream 一次只保留最近列流的有界前缀。切换 row group 前先将旧 `group` 设为 `None`，避免新旧大缓冲在分配期间同时占用内存。

`ranges` 用于判断何时可共享预载整组，`column_ranges` 用于限定 `get_read` 的流终点并决定是否建立短前缀缓存。两者均保持调用者传入顺序，查找采用首个包含 `start` 的范围；因此扩展 metadata 接线时应继续提供合法、无歧义的半开区间。

`peak_buffer` 初值为整文件缓存大小，否则为 0，只在加载 row group 时取最大值。它不是进程总内存、解码器内存或所有 `Bytes` 切片的实时统计。`buffer_bytes` 也不计 `stream`，且调用方持有的 `Bytes`/`Cursor` 可因引用计数而在状态槽清空后继续保持底层分配。

## 依赖与调用关系

上游调用链为：

`executor/importer::open_parquet_file` 或 `lightning/importer::parquet_source` -> `SourceReader::prepare` -> `FileParser::new[_with_location]` -> `parser::open_file_reader` -> parquet-rs `SerializedFileReader` -> `ChunkReader::{get_bytes,get_read}`。

metadata 建立后的局部接线为：

`FileParser::new_with_location` -> `reader_wrapper::row_group_range_from_meta` -> `SourceReader::set_ranges`。随后 `FileParser::build_group` 取得各列 reader，列 reader 通过 parquet-rs 间接回调本文件的 `ChunkReader` 实现。

直接下游依赖包括 `bytes::Bytes`、`std::io::{Read, Cursor}`、`std::sync::{Arc, Mutex}`，以及 parquet-rs 的 `ParquetError`、`Length`、`ChunkReader`。`SourceError` 是 `ParquetError` 的公开别名，方便上游 opener 不直接重复依赖错误路径。RustCodeGraph 将本文件标记为被 `file_parser.rs`、`parser.rs`、`parser_test.rs`、两个独立集成测试等 9 个文件引用；精确 callers/callees 查询未生成 trait 间接调用边，以上调用关系由这些已索引源码接线复核。

## 错误处理与边界

- `load` 对反向范围返回 `invalid range`，对无法装入 `usize` 的区间返回 `range too large`；opener 错误与 `read_exact` 的短读/IO 错误原样进入 `parquet::Result`。
- 构造后的文件大小必须不超过 `i64::MAX`。正的 `known_size` 被视为精确值：若启用 whole preload，读取失败会直接失败，不会回退到 `discover_size` 或流式策略。
- `set_ranges` 拒绝 `start > end` 和 `end > size`；它不主动拒绝空范围、重叠或乱序，正确性由 metadata 生产方保证。
- `get_read(start > size)` 与 `get_bytes(end > size)` 返回 `ParquetError::EOF`；`get_bytes` 还防止偏移加法溢出。`start == size` 的 `get_read` 和边界上的零长度 `get_bytes` 返回空内容。
- `cached` 只在请求完全落入某个缓存时返回切片；跨缓存尾部的 `get_bytes` 会走一次精确范围读取，`get_read` 则可把缓存前缀与后续流串联。
- `Mutex::lock().unwrap()` 表明互斥量中毒会 panic，而不是转成 `ParquetError`。调用 `RangeOpener` 和整段 `read_exact` 时部分路径仍持有状态锁，慢速对象存储会串行化共享 reader 的这些操作。
- `close` 不是底层流的显式取消协议：已返回给调用者的独立流/`Bytes` 仍由其自身生命周期管理。并发 `close` 与正在进行的 `get_read` 未由测试声明强一致顺序；安全扩展时不能假设它会中止已经打开的 IO。

## 并发与资源生命周期

`RangeOpener` 要求 `Send + Sync`，`SourceReader` 的克隆通过 `Arc<Mutex<State>>` 共享状态，因此 parquet-rs 可从多个句柄获得各自的 `Box<dyn Read>`。独立 cursor/stream 避免共享可变读取位置；缓存元数据与槽位由 mutex 防止数据竞争。不过 mutex 覆盖了 row-group 加载以及 stream 槽更新，不应把该实现理解为并行下载器。

整文件缓冲从构造成功持续到 `close` 或最后一个共享状态被释放；row-group 缓冲在首次访问合格范围时创建，并在切组前释放；stream 前缀在下一次列流建立时替换。`Bytes` 切片使用引用计数，返回给外部的切片可能延长底层 allocation 生命周期。上游对象存储 reader 的关闭职责由具体 `Box<dyn Read>` 实现承担，例如执行器入口使用 `ClosingObjectReader`，Lightning 入口依赖 `OpenRange` 返回流自身的生命周期。

`tests/page_streaming_peak.rs::importer_decoder_streams_large_pages_with_one_row_batches_and_bounded_peak` 强制关闭整文件/row-group 预载，验证 32 MiB 级未压缩 page 在逐行解码时峰值低于 4 MiB；这证明有界前缀与 parquet-rs page streaming 的组合，而不是 `peak_buffer_bytes` 单独统计的结果。

## 与 Go 版本的对应关系

Go 没有同名 `source_reader.go`；对应行为分散在 `reader_wrapper.go` 和 `parser.go`。Rust 常量分别对应 Go 的 `wholeFileInMemoryThreshold`（32 MiB）与 `rowGroupInMemoryThreshold`（128 MiB）。Rust 的 `prepare_with_thresholds` 对应 Go `prepareReader` 的 whole-file 选择，`Buffer`/whole-group 状态对应 `inMemoryReaderBase`，`set_ranges` 所用范围来自与 Go `rowGroupRangeFromMeta` 对齐的 Rust `reader_wrapper::row_group_range_from_meta`。

Go `Parser::getBuilder` 对当前 row group 选择 whole-file、row-group preload 或 per-column streaming；Rust 把策略集中到一个实现 `ChunkReader` 的共享 `SourceReader` 中，由 parquet-rs 按需回调。Go 小 row group 的 `newInMemoryReaderBase` 会用最多 8 个任务分段并行读取，Rust `load` 当前为一次范围流加同步 `read_exact`，并不复刻该内部并行方式。Go per-column 路径由 `readerWrapper` 的 skip buffer 减少 seek/reopen；Rust 则保存最近列流至多 64 KiB 的 `stream` 前缀，目标相同但机制不同。

Go `parser_test.go::TestParquetParserWholeFileInMemory` 验证相等阈值启用、未知大小禁用、超阈值禁用以及大 row group 流式分支，并检查对象存储 GET 次数。Rust `tests/whole_file_preload.rs` 用真实 decoder 复现相同四策略、1 次/4 次请求、内存估算替换和关闭/越界行为；`tests/page_streaming_peak.rs` 补充真实大 page 峰值验证。因此这里应视为行为对齐而非逐类型机械翻译。

## 扩展指南

- 调整缓存阈值或新增策略时，优先修改 `prepare_with_thresholds`、`cached` 和 `get_read`，同时保留默认 `prepare` 的稳定语义；同步更新 Go 的两个阈值/策略，或明确记录有意差异。
- 改变 row-group/column 范围语义时，要联合检查 `file_parser.rs::new_with_location` 和 `reader_wrapper.rs::row_group_range_from_meta`。范围必须继续使用绝对半开区间，且不得越过 `Length::len()`。
- 改变短前缀大小或复用方式时，要评估对象存储请求数、锁持有时间和大 page 峰值；不能为了减少请求而重新物化整个大列/page。
- 新增观测指标时应明确是否包含 `stream`、外部持有的 `Bytes` 切片和 parquet 解码器分配，避免把 `buffer_bytes`/`peak_buffer_bytes` 错当作总内存。
- 若增强关闭/取消语义，需要同时修改 `RangeOpener` 或其返回流契约，并覆盖“打开中取消、已有流、clone 共享关闭、关闭与加载竞态”；当前 `close` 只清状态缓存。
- Rust 测试必须保持独立文件。策略与边界回归优先放入 `tests/whole_file_preload.rs`，大页/峰值回归放入 `tests/page_streaming_peak.rs`；与行解析组合相关的用例可放入现有 `parser_test.rs`。Go 对齐需同步考虑 `parser_test.go::TestParquetParserWholeFileInMemory` 及 row-group streaming/内存估算用例。

主要兼容风险是改变范围终点或 EOF 语义导致 parquet-rs 读取失败；正确性风险是缓存返回不完整/错误偏移的字节；性能风险是对象存储请求放大、锁内慢 IO 和多份大缓冲重叠。

## 验证依据

- 目标源码：`pkg/dumpformat/parquetfile/source_reader.rs`，核对了全部 252 行以及 `RangeOpener`、`SourceReader`、`load`、`prepare[_with_thresholds]`、`set_ranges`、`cached`、`Length`、`ChunkReader`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/dumpformat/parquetfile` 确认模块与测试集合；`node --file .../source_reader.rs` 返回完整源码及 9 个引用文件；`query`/`node` 核对 `SourceReader`、`prepare`、`prepare_with_thresholds`、`set_ranges`、`get_read`、`get_bytes`、`parser::DEFAULT_BUFFER_SIZE` 和 `parser::open_file_reader`。精确 callers/callees 对 trait 间接调用未返回边，故没有据此虚构调用者。
- crate 与模块：`pkg/dumpformat/parquetfile/Cargo.toml`、`pkg/dumpformat/parquetfile/lib.rs`。
- 直接 Rust 接线：`pkg/dumpformat/parquetfile/file_parser.rs`、`pkg/dumpformat/parquetfile/parser.rs`、`pkg/executor/importer/import.rs`、`lightning/pkg/importer/chunk_process.rs`。
- Rust 测试：`pkg/dumpformat/parquetfile/tests/whole_file_preload.rs`、`pkg/dumpformat/parquetfile/tests/page_streaming_peak.rs`、`pkg/dumpformat/parquetfile/parser_test.rs`，另由 `pkg/executor/importer/production_storage_test.rs` 检查生产存储入口的 whole preload 状态。
- Go 对照：`pkg/dumpformat/parquetfile/reader_wrapper.go`、`pkg/dumpformat/parquetfile/parser.go`、`pkg/dumpformat/parquetfile/parser_test.go`。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收采用任务指定的固定章节结构命令，并人工复核上述符号、调用链、边界与扩展点均有直接源码或测试依据。
