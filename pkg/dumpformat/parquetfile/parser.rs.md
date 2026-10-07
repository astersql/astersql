# `pkg/dumpformat/parquetfile/parser.rs`

## 文件定位

本文件属于 `astersql-dumpformat-parquetfile` crate；模块由 `pkg/dumpformat/parquetfile/lib.rs` 以 `pub mod parser` 暴露。crate 的边界由 `pkg/dumpformat/parquetfile/Cargo.toml` 定义，直接依赖 `bytes`、`chrono`、`chrono-tz`、`astersql-lightning-mydump`，以及带固定 tag `astersql-parquet-v60.0.0-streaming-pages.1` 的 AsterSQL Arrow/Parquet fork。

文件有两个需要严格区分的部分：

- 第 20—856 行是被逐行注释掉的 Go 机械翻译草稿。它记录了 column iterator、对象存储 reader、row pool、Spark rebase 等原设计，但不会参与编译，不能作为当前 Rust 运行行为。
- 第 857 行以后才是当前可执行实现。它一方面提供便于验证 Go 语义的内存模型 `ParquetFile`/`Parser`，另一方面通过 `open_file_reader` 提供真实 Parquet 解码器的公共建造入口。

真实导入路径并不把整个文件物化为这里的 `ParquetFile`：`pkg/dumpformat/parquetfile/file_parser.rs::FileParser::new_with_location` 调用 `parser::open_file_reader` 获取 `SerializedFileReader`，再负责 row group、列批次、类型转换和 mydump 接口适配。因而本文件处于“共享元模型、进度/估算语义和底层 reader 配置”的位置，而 `file_parser.rs` 承担生产导入的完整逐页解码状态机。

## 核心职责

1. 定义简化的 Parquet 文件模型：`ConvertedType`、`ColumnDescriptor`、`RowGroup`、`ParquetFile` 和输出行 `ParsedRow`。
2. 用 `Parser` 实现顺序跨 row group 读行、逻辑行号、物理消费位置、扫描字节进度、定位与关闭状态；同时保留一组 Go 风格方法别名，便于迁移调用方。
3. 提供与 Go 对齐的辅助计算：`estimate_row_size`、`ReadRowCount`、`SampleStatisticsFromParquet` 和 `EstimateParquetReaderMemory`。
4. 用 `TrackingAllocator` 模拟 Go allocator 的当前/峰值内存统计及 64 字节额外对齐成本。
5. 在 `open_file_reader` 中统一真实 Parquet reader 的配置，显式启用 page streaming，供 `FileParser`、测试和其他消费者复用。

当前实现并不在 `Parser` 内解析 Parquet 编码页，也不在 `NewParser` 中打开对象存储。它接收已经构造好的内存 `ParquetFile`；真正的 schema 校验、列 reader 和对象存储范围读取位于 `file_parser.rs`、`source_reader.rs`、`reader_wrapper.rs` 与 `type_converter.rs`。

## 主要符号

- `DEFAULT_BUFFER_SIZE: usize = 64 * 1024`：对应 Go 的云存储小跨度 skip buffer 默认值。当前本文件的可执行路径没有直接使用它，保留为模块级兼容常量。
- `READ_BATCH_SIZE: usize = 128`：默认列批读取规模。`file_parser.rs::FileParser` 将它写入实例的 `batch_size`；`EstimateParquetReaderMemory` 也用它估算每列 level 缓冲。
- `ConvertedType`：旧式 converted/logical type 摘要。`Parser::new` 明确拒绝 `List`、`Map`、`MapKeyValue`、`Interval` 和 `NA`；`type_converter.rs` 与 `file_parser.rs` 还会复用该枚举。
- `ColumnDescriptor`：保存列名、`PhysicalType`、`LogicalType`、converted type 与 `adjusted_to_utc` 标记。简化 `Parser` 本身只读取列名和 `converted`；其余字段供类型转换/测试表达 schema 事实。
- `RowGroup`：包含已物化行 `rows` 与压缩大小估算 `compressed_bytes`。
- `ParquetFile`：包含列描述、row groups、footer metadata 摘要、`created_by` 和 `source_size`。`num_rows` 对所有 row group 的行数求和。
- `ParsedRow`：最近一行的值、逻辑 `row_id` 与估算 `length`。
- `estimate_row_size`：忽略 `None`；`Bytes`/`FixedBytes` 按实际字节数，其余非空值统一按 8 字节计。
- `Parser`：单线程顺序状态机。核心字段是 `current_group`、`current_row`、`total_read_rows`、`total_rows`、`last_row` 和 `closed`。
- `Parser::new` / `NewParser`：校验 unsupported converted type、生成 ASCII 小写列名、缓存总行数并初始化游标。
- `Parser::read_single_row`：内部推进物理位置并返回克隆行；不更新逻辑 `row_id`。
- `Parser::read_row` / `ReadRow`：先递增逻辑行号，再调用 `read_single_row`，成功时替换 `last_row`。
- `Parser::set_pos` / `SetPos`：按 `pos - last_row.row_id` 消费物理行，随后独立写入逻辑行号。
- `Parser::scanned_pos` / `ScannedPos`：以“已消费行数 / 总行数 × 源文件大小”估算进度。
- `ReadRowCount`：返回内存模型中所有 row group 的总行数。
- `SampleStatisticsFromParquet`：最多读取 1024 行，返回总行数与平均估算行大小。
- `TrackingAllocator`：以分配 ID 追踪当前和峰值字节数；每次非零新分配额外计 64 字节。
- `EstimateParquetReaderMemory`：估算首个 row group 的预读缓冲、列值内存和每列 level 缓冲之和。
- `open_file_reader`：为任意实现 Parquet `ChunkReader` 的源构造 `SerializedFileReader`，并开启 page streaming。

## 执行流程

内存模型的构造和逐行读取流程如下：

1. `NewParser(file)` 转发到 `Parser::new`。
2. `Parser::new` 遍历 `file.columns`，遇到不支持的容器/特殊 converted type 立即报错；随后将列名做 `to_ascii_lowercase`，用 `ParquetFile::num_rows` 记录总行数，并把 group/row 游标置零。
3. 调用 `read_row` 时，方法先将 `last_row.row_id` 加一并清零旧长度，然后调用 `read_single_row`。
4. `read_single_row` 先拒绝已关闭解析器和已越过最后 row group 的状态；当前 group 恰好读完时只前进一个 group，再从该 group 获取当前行。
5. 行列数必须与 schema 列数相同，否则返回带实际列数和 schema 列数的错误。成功后推进 `current_row` 与 `total_read_rows`，计算行大小并返回新 `ParsedRow`。
6. `read_row` 把预先递增的逻辑行号写入返回行并更新 `last_row`。

这里有一个由测试固定的细节：一次读取最多跨过一个 row group。若下一个 group 为空，本次调用会因取不到行而返回 `EOF`；再次调用才会继续推进到再下一个 group。`parser_test.rs::parquet_parser_moves_across_multiple_row_groups` 明确验证了这一 Go 对齐行为，同时也验证失败读取仍已递增逻辑 `row_id`。

定位与进度流程：

- `set_pos(pos, row_id)` 计算 `pos - last_row.row_id`。正数时反复调用 `read_single_row` 丢弃行；非正数的 Rust range 为空，不回退物理游标。最后仅把逻辑行号设为 `row_id`。
- `pos()` 返回 `(total_read_rows, last_row.row_id)`，因此物理消费计数和逻辑 checkpoint 行号可以不同。
- `scanned_pos()` 对空文件或全部读完直接返回 `source_size`；否则按行数比例估算，避免底层预读让网络/文件游标虚高。

统计与内存流程：

- `SampleStatisticsFromParquet` 若没有首 group，或首 group 为空，直接返回 `(0, 0.0)`；否则构造解析器并抽样 `min(total_rows, 1024)` 行。
- `EstimateParquetReaderMemory` 只看首 group。压缩大小在 `(0, 128 MiB]` 时计入整组预读成本；再加所有非空 `ColumnValue` 的内存估算，以及 `columns × READ_BATCH_SIZE × 4` 的 level 缓冲。
- `open_file_reader` 通过 `ReadOptionsBuilder` 注入启用了 `page_streaming` 的 `ReaderProperties`，再调用 `SerializedFileReader::new_with_options`。真实导入的 `FileParser::new_with_location` 以此为起点读取 footer/schema 并配置范围读取。

## 数据与状态

`Parser` 维护两套有意分离的进度：

- 物理状态：`current_group`、`current_row` 和 `total_read_rows`，反映内存文件中实际消费的位置。
- 逻辑状态：`last_row.row_id`，用于 checkpoint/恢复语义，可以由 `set_pos` 或 `set_row_id` 独立重置。

`last_row` 保存值的拥有型克隆，不借用 `ParquetFile`。因此这个简化实现没有注释草稿中“下一次读取会使浅拷贝失效”的生命周期限制；真实 `FileParser` 则从 Parquet 列 reader 按批获取值并转换为自有 `Datum`。

`column_names` 在构造时固定为 ASCII 小写。`columns()` 返回借用切片，不允许调用者修改内部 schema 顺序。`total_rows` 同样在构造时缓存；`file` 被 `Parser` 独占后没有公开修改接口，所以该缓存保持一致。

`closed` 是单向标记。`close` 幂等地将其设为 `true`；`read_single_row` 在任何数据访问前检查它。其他只读方法（例如 `columns`、`pos`、`scanned_pos`）在关闭后仍可调用。

`TrackingAllocator` 用单调递增的 `next_id` 标识分配，`allocations` 保存 ID 到计费大小的映射。`current` 是尚未释放的总计费大小，`peak` 只增不减。它是测试/估算模型，并不实现 Rust 全局 allocator，也没有实际对返回 `Vec<u8>` 地址做 64 字节对齐；“+64”只是与 Go 统计口径对齐的记账成本。

## 依赖与调用关系

上游关系：

- `pkg/dumpformat/parquetfile/lib.rs` 公开本模块。
- `pkg/dumpformat/parquetfile/file_parser.rs::FileParser::new_with_location` 调用 `open_file_reader`；这是当前真实 Parquet 导入解码主链中的直接生产调用边。
- `lightning/pkg/importer/get_pre_info.rs` 直接调用 `open_file_reader(bytes::Bytes)` 读取真实 Parquet 元数据。
- `pkg/dumpformat/parquetfile/parser_test.rs` 调用 `NewParser`、`ReadRowCount`、`SampleStatisticsFromParquet`、`EstimateParquetReaderMemory` 和 `open_file_reader`，覆盖内存语义和真实 reader 配置。
- `pkg/dumpformat/parquetfile/tests/page_streaming_peak.rs` 与 `tests/whole_file_preload.rs` 直接调用 `open_file_reader`，分别验证大页流式读取和不同预读策略。

下游关系：

- `ColumnDescriptor` 使用 `column_type.rs::{PhysicalType, LogicalType}`。
- 行数据和行/内存估算使用 `column_value.rs::{ColumnValue, account_column_value_memory_bytes}`。
- 所有本地错误通过 crate 根的字符串包装 `Error`/`Result` 返回。
- `open_file_reader` 依赖上游 tag 固定的 `parquet` crate 的 `ChunkReader`、`ReadOptionsBuilder`、`ReaderProperties` 和 `SerializedFileReader`。

RustCodeGraph 对目标文件的文件级索引显示它被 `file_parser.rs`、`parser_test.rs`、`type_converter.rs`、`pkg/dumpformat/testutils/parquet_writer.rs`、`pkg/dxf/importinto/task_executor.rs` 等文件使用。对 `parser.rs::NewParser`、`read_single_row`、`open_file_reader`、`SampleStatisticsFromParquet`、`EstimateParquetReaderMemory` 分别执行了精确符号查询；其中同名 Go/Rust 符号必须结合 `filePath` 区分。调用边查询对这些 Rust 符号没有返回完整边，因此上述生产调用关系又以直接源码引用核验，没有把缺失图边推断成“无调用者”。

## 错误处理与边界

- `Parser::new` 只按 `ConvertedType` 拒绝 `List`、`Map`、`MapKeyValue`、`Interval`、`NA`。完整的逻辑类型与物理编码适配性校验属于 `file_parser.rs::normalized_logical_type`，不能把简化构造器视为真实文件的全部 schema 验证。
- `read_single_row` 对关闭状态返回 `parser is closed`；耗尽 group 列表、当前/空 group 无行时返回字符串 `EOF`。crate 的统一 `Error` 没有结构化错误类别。
- 读取到行后会检查 `row.len() == file.columns.len()`，防止 schema 与物化行错位。该错误发生前不推进物理计数。
- `read_row` 在实际读取之前先递增逻辑行号；因此 EOF、关闭或行宽错误也会消耗一个逻辑 `row_id`。这是当前代码事实，并由跨空 group 测试间接固定，调用方不能假定失败时行号不变。
- `set_pos` 只能向前消费物理行，负差值不会回退。若中途读取失败，已经消费的物理行不会回滚，且最终逻辑 `row_id` 尚未写入。
- `scanned_pos` 假设 `source_size` 非负且 `total_read_rows <= total_rows`。构造模型正常使用时成立；字段私有保证外部无法破坏计数。
- `SampleStatisticsFromParquet` 特意把“首 group 为空”视为整个样本为空，即使后续 group 有行也返回 `(0, 0.0)`；这是 Go 规则对齐，不应擅自改成跳过空 group。
- `TrackingAllocator::free` 对未知或已释放 ID 静默忽略；`reallocate` 对未知 ID 采用容量零。扩容顺序是先分配、复制、再释放旧块，所以峰值会同时包含新旧块。
- `open_file_reader` 将上游错误包装为 `open parquet source: ...`，保留底层文本；它不负责关闭源，资源所有权随返回的 `SerializedFileReader` 传递。

## 并发与资源生命周期

`Parser` 的方法需要 `&mut self` 才能推进游标，本身没有锁、原子或内部并发设计；文档注释也明确它是顺序状态机。不要把同一个实例跨线程并发读取。`ParquetFile`、`RowGroup`、`ParsedRow` 等值类型可克隆，但克隆数据不共享解析进度。

`TrackingAllocator` 使用普通 `i64` 和 `BTreeMap`，同样不是线程安全统计器；它与 Go 版使用 `atomic.Int64`/`sync.Map` 的并发能力不同，只适合单线程测试与静态估算。

内存模型的 `close` 不持有外部文件句柄，只改变 `closed`。`recycle_row` 是空操作，因为 `ParsedRow` 持有普通 `Vec`，当前实现没有 row pool。注释草稿中的 allocator close、column reader close 和 row pool 归还逻辑均未编译。

真实资源生命周期由下游完成：`open_file_reader` 返回拥有 source 的 `SerializedFileReader`；`FileParser` 持有 reader 与 `SourceReader`，其 `close` 清空列 reader、关闭 source 并标记关闭。`tests/whole_file_preload.rs::real_rows_and_eof_match_across_whole_group_and_column_strategies` 验证关闭后继续读会报 closed，并覆盖 whole-file、row-group 与 column streaming 策略。`tests/page_streaming_peak.rs::large_plain_page_reads_values_and_eof_with_bounded_peak` 验证大 plain page 在 page streaming 下保持有界峰值。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dumpformat/parquetfile/parser.go`。

- 常量：`DEFAULT_BUFFER_SIZE` 对应 `defaultBufSize`，`READ_BATCH_SIZE` 对应 `readBatchSize`。
- 行大小：Rust `estimate_row_size` 与 Go `estimateRowSize` 都忽略 NULL、字符串/字节按长度、其他值按 8 字节。
- 解析进度：Rust `pos`、`set_pos`、`scanned_pos`、`set_row_id` 保留 Go 的“物理消费位置与逻辑 row ID 分离”规则；Rust 测试 `parquet_parser_set_pos_skips_forward_and_allows_go_style_row_id_reset` 明确覆盖负差值不回退。
- 列名：两端都在构造时转小写。
- unsupported 类型：简化 Rust `Parser::new` 对齐旧 converted type 黑名单；当前 Go `validateParquetLogicalType` 还会拒绝 List、Map、Interval、Unknown、Float16、Variant，并验证 logical type 与 physical type/type length 的适配性。真实 Rust `FileParser` 才承担更接近这一规则的校验。
- row group：Go `readSingleRow` 在当前 group 完成时调用 `moveToNextRowGroup`；简化 Rust 每次也只推进一个 group，因而保留中间空 group 产生一次 EOF 的测试语义。
- 资源模型：Go `Parser` 持有 metadata、对象存储、每列 reader、row pool 和 allocator；当前 Rust `Parser` 只持有内存 `ParquetFile`。生产等价能力被拆到 `FileParser`、`SourceReader`、`reader_wrapper` 和类型转换模块，不能只按类型同名认定一一完成移植。
- 采样：两端最多抽样 1024 行，且首 row group 为空时返回零统计。
- 内存估算：Go 用真实 tracking allocator 读取首 group，再加 allocator 外的 preload buffer；本文件的 Rust 函数是结构化静态估算，按首 group 已物化值与 level buffer 求和。它保持测试口径，但不等价于测量真实 decoder 的动态峰值。
- allocator：Go 返回实际 64 字节对齐切片并用地址追踪分配；Rust `TrackingAllocator` 以 ID 与额外 64 字节记账模拟，返回的 `Vec` 不保证该对齐。
- page streaming：当前 Go `NewParser` 设置 `PageStreamingEnabled = true`；Rust `open_file_reader` 通过固定 tag 的 Parquet fork 设置 `set_page_streaming_enabled(true)`，这是生产路径上的直接对应。

文件顶部的大段注释草稿更接近 Go `Parser` 的完整形态，但没有编译。后续评估移植完成度时，应以第 857 行后的实现及 `file_parser.rs` 的真实代码为准，而不是以草稿中的函数名为准。

## 扩展指南

- 新增 converted/logical type 时，先确定改动面属于简化模型还是真实文件路径。内存模型需更新 `ConvertedType` 和 `Parser::new`；生产行为还必须同步 `file_parser.rs::normalized_logical_type`、`type_converter.rs`，以及对应的独立测试，不能只让 `NewParser(ParquetFile)` 通过。
- 改动 row group、EOF、行号或 checkpoint 语义时，优先修改 `Parser::read_single_row`、`read_row`、`set_pos`，并同步 `parser_test.rs` 中读取、跨 group、定位和 scanned position 用例；生产接口还要核对 `file_parser.rs::ImportParser` 的同名方法。
- 改动行大小规则时，同时更新 `estimate_row_size`、`SampleStatisticsFromParquet`、内存估算调用者与 `row_size_row_count_and_sampling_match_go_rules`；还要对照 Go `estimateRowSize`，避免改变导入配额/进度口径。
- 改动 reader 属性时只在 `open_file_reader` 汇总配置，并运行真实文件测试。尤其不能关闭 page streaming 而只依赖小型单元测试，因为 `tests/page_streaming_peak.rs` 专门覆盖大页峰值。
- 改动内存估算时区分三部分：整组预读、值拥有内存和 definition/repetition level 缓冲；同步 `reader_memory_estimate_includes_preload_values_and_level_buffers` 以及 whole-file preload 替换测试。若目标是生产精确峰值，应扩展真实 `FileParser` 路径，不能把静态 `ParquetFile` 估算包装成实测值。
- 若引入共享并发读取，需要重新设计 `Parser` 与 `TrackingAllocator`；简单添加 `Send`/`Sync` 约束不足以保护游标和映射状态。
- Rust 测试继续放在独立的 `parser_test.rs` 或 `tests/*.rs`，不要嵌入生产源文件。任何 Go 对齐变更都应同时阅读 `parser_test.go` 的相应用例，保留真实行为而非简化到仅能编译。

兼容风险集中在逻辑类型接受范围、失败时 row ID 推进、空 row group 的 EOF 次序和 scanned position 口径；性能风险集中在 `READ_BATCH_SIZE`、page streaming、128 MiB 预读阈值及估算是否重复计算 buffer。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/dumpformat/parquetfile/parser.rs`，RustCodeGraph `node --file` 完整读取 1—1217 行；确认第 857 行之前为注释草稿，之后为编译实现。
- crate 边界：`pkg/dumpformat/parquetfile/Cargo.toml`、`pkg/dumpformat/parquetfile/lib.rs`；确认模块公开方式、直接依赖和 Parquet fork tag。
- Go 对照：`pkg/dumpformat/parquetfile/parser.go`；核对常量、schema 校验、row group 状态机、定位/进度、采样、allocator 与内存估算。
- Rust 独立测试：`pkg/dumpformat/parquetfile/parser_test.rs`；核对列名、读取/EOF、跨 group、定位、unsupported 类型、采样、allocator、内存估算与 scanned position。
- 真实解码入口：`pkg/dumpformat/parquetfile/file_parser.rs`；核对 `FileParser::new_with_location -> parser::open_file_reader`、schema 校验、row group 状态与 `ImportParser` 适配。
- 真实 reader 测试：`pkg/dumpformat/parquetfile/tests/page_streaming_peak.rs`、`pkg/dumpformat/parquetfile/tests/whole_file_preload.rs`；核对 page streaming、大页峰值、预读策略、EOF 与关闭生命周期。
- 其他直接生产调用：`lightning/pkg/importer/get_pre_info.rs` 对 `open_file_reader` 的元数据读取调用。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dumpformat/parquetfile` 确认目标与相关 Rust/Go 文件均在索引内；对 `NewParser`、`read_single_row`、`open_file_reader`、`SampleStatisticsFromParquet`、`EstimateParquetReaderMemory` 做了精确 query，并对 Rust qualified symbol 执行 callers/callees。图未返回这些符号的完整调用边，因此又用上述直接源码引用补证。

人工复核结论：本文能够区分注释草稿、内存兼容模型和真实逐页导入入口；说明了文件存在目的、主要状态机、错误/生命周期边界、Go 差异及安全扩展位置，没有把未编译草稿写成当前已支持行为。
