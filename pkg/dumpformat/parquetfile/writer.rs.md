# `pkg/dumpformat/parquetfile/writer.rs`

## 文件定位

本文件位于 `astersql-dumpformat-parquetfile` crate 的写出层。crate 根模块 [`lib.rs`](./lib.rs) 通过 `pub mod writer` 暴露它，并把 [`column_type.rs`](./column_type.rs)、[`schema_builder.rs`](./schema_builder.rs)、[`column_buffer.rs`](./column_buffer.rs) 和 [`column_value.rs`](./column_value.rs) 的结果汇合为标准 Parquet 文件：输入是一行 `&[Option<Vec<u8>>]` SQL 原始字节，输出是实现 `std::io::Write + Send` 的 sink。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，库入口为 `lib.rs`，直接依赖固定 tag `astersql-parquet-v60.0.0-streaming-pages.1` 的 AsterSQL `arrow-rs` Parquet 分支。`chrono`、`chrono-tz` 和 `astersql-lightning-mydump` 由同 crate 的其他模块消费；本文件的外部核心依赖是 `parquet`。

RustCodeGraph 与全仓库引用搜索表明，当前 Rust 生产代码没有直接构造本文件的 `ParquetWriter`/`NewWriter`：crate 外的 Rust Dumpling 写出逻辑 [`../../../dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs) 只复用本 crate 的列 schema 和原始值解析能力，自行完成 Parquet 序列化。本文件目前由独立 Rust 测试直接覆盖。Go 对照文件 [`writer.go`](./writer.go) 则已由 Dumpling 的 `WriteInsertInParquet` 生产链实际调用。因此不能把 Go 的生产接线误写成 Rust 当前已接线事实。

## 核心职责

1. 将压缩、data page 大小和 row group 内存阈值折叠为 `WriterOptions`，默认使用 Snappy 和 120 MiB 阈值。
2. 用 `build_parquet_schema_from_columns` 规范化 `ColumnInfo`，再由私有 `parquet_schema` 构造 parquet-rs 的真实 `Type` 树与 `SerializedFileWriter`。
3. 逐列解析一行 SQL 原始字节，维护 definition level、类型化列缓冲、缓冲行数和估算内存。
4. 达到内存阈值或关闭时，将全部列缓冲写成一个 row group，并在最终关闭时写完 Parquet footer。
5. 通过 `CountingWriter` 统计底层实际接受的字节，结合未刷新的估算内存提供文件大小估计。
6. 提供 Go 风格的公开名称 `WithCompression`、`WithDataPageSize`、`WithRowGroupMemoryLimit`、`CompressionCodec`、`NewWriter`、`Write`、`Close`、`EstimateFileSize`，便于逐步移植调用方。

## 主要符号

- `DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES`：默认 row group 缓冲阈值，值为 `120 * 1024 * 1024`。
- `DEFINITION_LEVEL_MEMORY_BYTES`：每个可空列 definition level 的估算成本，固定为 2 字节。
- `CompressionType` 与 `compression_codec`/`CompressionCodec`：把上层压缩类别映射为本文件的 `Compression`；未知类型回退到 Snappy。
- `Compression`：`Uncompressed`、`Gzip`、`Snappy`、`Zstd` 四种 writer codec，构造时再映射为 parquet-rs codec。
- `WriterOptions`、`WriterOption` 与 `writer_options`：保存和折叠运行时选项。选项按切片顺序应用，后出现的同类选项覆盖前者；非正 row group 限额被忽略，data page 大小则在构造属性时钳制到至少 1。
- `parquet_schema(&[Column]) -> Result<Arc<Type>>`：将内部物理类型、逻辑类型、长度、precision、scale 和 `allows_null_encoding` 转成 parquet-rs 根 schema。
- `CountingWriter<W>`：转发 `Write`，用共享 `Arc<AtomicI64>` 累加底层每次实际返回的写入字节；`written_bytes` 读取计数，`into_inner` 取回 sink。
- `ParsedColumnValue`：单列转换的中间态，分离可选 `ColumnValue` 与最终应否按 NULL 编码。
- `ParquetWriter<W>`：核心状态机，持有 `SerializedFileWriter`、列描述、逐列缓冲、选项、缓冲行数/内存和关闭标记。`Writer<W>` 是它的类型别名。
- `ParquetWriter::new`/`NewWriter`：构造入口。
- `write`/`Write`、`parse_and_append_row`、`parse_column_value`、`append_parsed`：逐行写入链。
- `flush_rows`：把当前列缓冲写成一个 parquet-rs row group。
- `close`/`Close`：幂等关闭入口；先标记关闭，再尝试 flush 与 footer finish，并合并两处错误。
- `estimate_file_size`/`EstimateFileSize` 与 `total_written_bytes`：分别报告“已落 sink + 未刷缓冲估算”和实际落 sink 字节。
- `encode_buffer`：私有的简化二进制编码与一致性检查辅助函数。当前没有任何调用者，不参与标准 Parquet 写出路径；真实 flush 使用 parquet-rs 的 `ColumnWriter::write_batch`。

## 执行流程

构造阶段从 `ParquetWriter::new` 开始：

1. `build_parquet_schema_from_columns` 校验并规范化 `ColumnInfo`；其轻量 `Schema` 被丢弃，`Vec<Column>` 继续使用。
2. `new_column_buffers` 按物理类型建立一列一个缓冲；随后 `writer_options` 折叠选项，`parquet_schema` 创建真实 parquet-rs schema。
3. 压缩枚举映射到 parquet-rs；若指定 data page 大小，则设置 `WriterProperties::set_data_page_size_limit`。
4. sink 被包装为 `CountingWriter`，并交给 `SerializedFileWriter::new`；共享原子计数同时保存在 `ParquetWriter` 中。

每次 `write` 的流程是：

1. 若 `closed` 已置位，立即返回 `parquet writer is closed`。
2. `parse_and_append_row` 先检查行宽必须等于列数，再从左到右调用 `parse_column_value` 和 `append_parsed`。
3. `None` 只允许用于 `allows_null_encoding` 的列；非空字节交给 `column_value::parse_raw_column_value`。错误统一增加 `convert parquet column <name>` 上下文。
4. 可空编码列每行追加 definition level：NULL 为 0，非空为 1，并把 2 字节计入估算；非 NULL 值追加到对应类型缓冲，并由 `account_column_value_memory_bytes` 累加估算。
5. 全行成功后 `buffered_rows += 1`。若估算内存达到正阈值，立即调用 `flush_rows`。

`flush_rows` 在无缓冲行时为空操作；否则从 `SerializedFileWriter` 创建 row group，按 schema 顺序取得列 writer，并针对 Boolean、Int32、Int64、Float、Double、ByteArray、FixedLenByteArray 调用对应的 `write_batch`。可空列传入 definition levels，必填列传 `None`。每列写完立即关闭列 writer，全部列完成后关闭 row group。只有整次成功后才清零行数、内存并 `reset` 全部缓冲，因此失败时状态不会伪装成已提交。

`close` 首次调用先把 `closed` 置为 true，再分别执行 `flush_rows` 与 `SerializedFileWriter::finish`。两者都失败时用分号拼接消息；任一失败则返回该错误。再次调用直接成功。成功 finish 后，文件包含 parquet-rs 写出的 header、row groups、metadata/footer 和尾部 `PAR1`。

## 数据与状态

核心不变量是 `columns.len() == buffers.len()`，且两者始终按输入列顺序对应。`buffered_rows` 只在整行逐列追加成功后增加；每次成功 flush 后归零。`buffered_memory_bytes` 是触发 flush 和估算文件大小的近似值，不是分配器的精确占用：可空列每行固定加 2，值部分由 `account_column_value_memory_bytes` 估算。

可空性由 `Column::allows_null_encoding` 决定，而不等同于 SQL 元数据的 `nullable`：schema builder 会让 TIMESTAMP/DATETIME 的兼容路径也允许 definition level 0，用于把非法 MySQL 时间编码为 NULL。NULL 行不向物理值数组追加元素，因此每列的物理值数量应等于非零 definition level 数；必填列则应等于 `buffered_rows`。

输入的 `Vec<u8>` 经解析后转成拥有所有权的 `ColumnValue` 并写入列缓冲，调用方之后修改自己的缓冲不会影响已排队数据。ByteArray 与 FixedLenByteArray 在 flush 时还会克隆为 parquet-rs 值对象，带来与当前 row group 数据量成正比的临时分配。

`written_bytes` 使用 `Arc<AtomicI64>` 和 `Ordering::Relaxed`，只保证计数的原子可见性，不建立业务同步顺序。计数按底层 `write` 实际返回的 `usize` 增加，所以短写后报错时也保留已成功写出的字节。`writer: Option<_>` 构造后始终为 `Some`，当前 `close` 使用 `finish` 但不 `take` 它；关闭状态由独立布尔值控制。

## 依赖与调用关系

RustCodeGraph 对目标符号和源码的查询，以及全仓库精确引用搜索，得到以下直接关系：

- 内部构造链：`ParquetWriter::new` → `schema_builder::build_parquet_schema_from_columns`、`column_buffer::new_column_buffers`、私有 `parquet_schema`、`SerializedFileWriter::new`。
- 行写入链：`write` → `parse_and_append_row` → `parse_column_value`/`column_value::parse_raw_column_value` → `append_parsed` → `column_value::append_column_value`；达到阈值后 `write` → `flush_rows`。
- flush 链：`flush_rows` → parquet-rs `next_row_group` → 每列 `next_column`/`write_batch`/`close` → row group `close`。
- 关闭链：`close` → `flush_rows` 与 `SerializedFileWriter::finish`。
- 统计链：`CountingWriter::write` 更新共享原子；`total_written_bytes` 读取它；`estimate_file_size` 再加 `buffered_memory_bytes`。
- 兼容入口 `CompressionCodec`、`Write`、`Close`、`EstimateFileSize`、`NewWriter` 都只是转发相应 snake_case 实现。
- Rust 直接调用者目前是 [`writer_behavior_test.rs`](./writer_behavior_test.rs)、[`writer_core_test.rs`](./writer_core_test.rs) 和 [`writer_test.rs`](./writer_test.rs)。全仓库 Rust 引用没有发现生产代码构造 writer；Dumpling Rust 仅在 `writer_util.rs` 使用该 crate 的 schema/value 辅助。
- Go 生产上游是 [`../../../dumpling/export/writer_util.go`](../../../dumpling/export/writer_util.go) 的 `WriteInsertInParquet`：它把配置转为三个 writer option，构造 `parquetfile.NewWriter`，逐行写入、估算切分大小并关闭文件。

## 错误处理与边界

构造错误包括列元数据/schema 不合法、无法为某物理类型建立缓冲、parquet-rs schema 构建失败或底层 writer 初始化失败，统一转换为 crate 的字符串 `Error`。泛型 `output: W` 不能表达 Go 的 nil writer，因此 Rust 没有 `parquet output buffer is nil` 分支。

写入前校验关闭状态和行宽。必填列收到 `None` 会失败；原始值解析或追加失败时，错误带列名上下文。这里刻意保持 Go 的“逐列提交”语义而非事务式整行追加：如果一行的前几列已经追加、后续列转换失败，前面列的缓冲不会回滚，`buffered_rows` 又不会增加，writer 因列长度失配而不再安全可用。调用方应停止写入；现有行为测试证明随后 `close` 会报错，而不是静默生成不一致文件。

`flush_rows` 的错误可能来自创建 row group、schema 列数不足、物理 writer 不支持、列批写入、列关闭或 row group 关闭。Int96 没有实现写出分支，落入 `unsupported parquet physical writer type`。发生错误时缓冲不会 reset；但底层 parquet writer 可能已经部分写入，不能重试同一 writer 来承诺原子性。

`close` 在执行任何 I/O 前就设置 `closed = true`，因此首次关闭失败后第二次关闭仍返回成功，不会重试未完成的 flush/footer。这与 Go 幂等状态语义一致，也意味着调用方必须保留并处理第一次错误。`CountingWriter` 的 `Write` 实现记录返回的部分字节，之后才传播底层错误；它只转发 `flush`，不会像 Go 的 `countingWriter.Close` 那样探测并关闭底层 `io.Closer`。Rust sink 只能依靠所有权析构或其自身封装处理额外关闭协议。

私有 `encode_buffer` 会检查 definition level 数、实际物理值数以及定长字节宽度，但当前无调用者，不能把这些额外检查视为标准 flush 前置保障。实际路径依赖列缓冲逻辑和 parquet-rs `write_batch` 的校验。

## 并发与资源生命周期

所有 API 都是同步的，不创建线程、异步任务、通道或事务。`ParquetWriter` 的变更操作要求 `&mut self`，同一个实例不能在安全 Rust 中被多个线程并发写入；泛型的 `W: Write + Send` 只允许 writer 随所有权跨线程移动，不表示可共享并发调用。

一个实例的资源生命周期是“构造 schema/writer → 多次写行并按阈值产生若干 row group → `close` flush 尾组并 finish footer”。成功 flush 后列向量通过 `reset` 复用容量，避免每个 row group 重新分配基础缓冲。ByteArray flush 的临时转换向量在函数返回时释放。

调用者必须显式调用 `close` 才能保证剩余行和 footer 被写出；本类型没有 `Drop` 自动 finish。底层 sink 的生命周期归 `SerializedFileWriter<CountingWriter<W>>` 所有，当前公开 API也没有从 `ParquetWriter` 取回 sink 的方法；`CountingWriter::into_inner` 仅在单独持有包装器时可用。

## 与 Go 版本的对应关系

直接对照文件是 [`writer.go`](./writer.go)。Rust 的 `ParquetWriter`/`Writer`、`WriterOption`、压缩映射、`NewWriter`、逐列追加、按 120 MiB 默认阈值 flush、`EstimateFileSize` 和幂等 `Close` 均对应 Go 同名结构与方法。两侧都保留这些关键契约：未知压缩回退 Snappy；非正 row group 限额保留默认值；达到而非超过阈值即 flush；转换失败不会回滚前列；关闭同时尝试 flush 与 footer；实际写出计数包含短写成功的部分。

主要实现差异如下：

- Go 使用 Arrow Go `file.Writer`，Rust 使用固定 AsterSQL tag 的 parquet-rs `SerializedFileWriter`；两者生成标准 Parquet，但 metadata API 对现代 logical type 派生 legacy converted type 的表现可能不同，`writer_core_test.rs` 已记录 TIMESTAMP_MICROS 差异。
- Go 的 `WriterOption` 是函数，允许 nil option 并忽略它；Rust 是枚举切片，没有 nil 状态。
- Go 的 `NewWriter` 接受 `io.Writer` 并显式拒绝 nil；Rust 泛型参数在类型层面排除 nil。
- Go `countingWriter.Close` 会在 sink 实现 `io.Closer` 时向下关闭；Rust 只满足 `Write`，`close` 的 finish 不显式调用 sink 专有 close。
- Go 直接用 schema builder 返回的 Arrow schema；Rust schema builder 返回轻量 schema 和 `Column`，本文件的 `parquet_schema` 再物化一次 parquet-rs schema。
- Rust `close` 用字符串拼接两个错误来模拟 Go `errors.Join`，不保留可供 `errors.Is/As` 式遍历的结构化错误链。
- Go writer 已接入 Dumpling `WriteInsertInParquet`；当前 Rust Dumpling 有独立的 parquet schema/行写出实现，尚未调用本 writer。

对照测试包括 Go 的 [`writer_behavior_test.go`](./writer_behavior_test.go)、[`writer_core_test.go`](./writer_core_test.go) 和 Rust 的 [`writer_behavior_test.rs`](./writer_behavior_test.rs)、[`writer_core_test.rs`](./writer_core_test.rs)、[`writer_test.rs`](./writer_test.rs)。Rust 测试覆盖输入复制、压缩、阈值 row group、大小估算、非法行宽/NULL、部分列污染、sink 失败、真实文件 magic/footer、schema/值回读、无符号 BIGINT 和部分写计数。

## 扩展指南

- 新增压缩算法时，应同步修改 `CompressionType`、`Compression`、`compression_codec` 和 `ParquetWriter::new` 的 parquet-rs 映射，并扩展 `writer_behavior_test.rs`；同时核对 Go 的 `CompressionCodec`，避免默认回退分叉。
- 新增物理类型时，必须同步 `parquet_schema`、列缓冲创建/追加、`flush_rows` 的 `ColumnWriter` 分派和相关独立测试；若保留 `encode_buffer`，还需同步其计数与编码分支。只改 schema 会得到可构造但不可写出的类型。
- 修改 NULL 或时间兼容语义时，应从 `schema_builder::allows_null_encoding`、`parse_raw_column_value`、definition level 和真实 reader 回读四处一起验证。
- 修改内存阈值时要明确它是估算值而非 allocator RSS。值类型的估算由 `column_value::account_column_value_memory_bytes` 决定，可空列还固定增加 2 字节；改变任一规则都会影响 row group 边界和 Dumpling 文件切分估算。
- 若希望写入失败后恢复，必须先设计整行预解析或回滚机制；不能只捕获错误继续写，因为当前逐列追加会留下部分状态，Go 兼容测试也锁定了这一事实。
- 若把本 writer 接入 Rust Dumpling，应先消除或明确处理 `writer_util.rs` 的并行实现，验证 object-store sink 的关闭语义、错误传播、配置映射和文件大小切分，不应假定 Cargo 依赖已等同于生产接线。
- 测试逻辑必须继续放在独立文件。核心格式/回读场景扩展 `writer_core_test.rs`，状态与失败边界扩展 `writer_behavior_test.rs`，短写计数扩展 `writer_test.rs`，并尽量与对应 Go 测试保持同一意图。
- 兼容性风险集中在 physical/logical type、definition level、row group 边界、压缩和关闭错误；性能风险集中在大 row group 的内存估算偏差，以及 ByteArray/FixedLenByteArray flush 时的克隆和临时向量。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，目标文件在索引范围内。
- RustCodeGraph `explore` 与 `query`：定位 `ParquetWriter`、`flush_rows`、`parse_and_append_row`、`estimate_file_size` 等目标符号，核对 `write → flush_rows`、`close → flush_rows`、`estimate_file_size → total_written_bytes` 及测试调用；对常见名称产生的无关候选通过文件路径限定排除。
- 全仓库引用核对：`rg` 仅发现 Rust 独立测试直接构造本 writer；[`../../../dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs) 只调用本 crate 的 schema/value API。Go 的 [`../../../dumpling/export/writer_util.go`](../../../dumpling/export/writer_util.go) 则明确调用 `parquetfile.NewWriter`、三个 option、`Write`、`EstimateFileSize` 和 `Close`。
- 已读生产源码：[`writer.rs`](./writer.rs)、[`lib.rs`](./lib.rs)、[`schema_builder.rs`](./schema_builder.rs)、[`column_buffer.rs`](./column_buffer.rs)、[`column_value.rs`](./column_value.rs)、[`../../../dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs)。
- 已读 crate 配置：[`Cargo.toml`](./Cargo.toml)，并通过 workspace/Cargo 引用确认 Dumpling 等 crate 对本 crate 的依赖边界。
- 已读 Go 对照：[`writer.go`](./writer.go) 与 [`../../../dumpling/export/writer_util.go`](../../../dumpling/export/writer_util.go)。
- 已读 Rust 独立测试：[`writer_behavior_test.rs`](./writer_behavior_test.rs)、[`writer_core_test.rs`](./writer_core_test.rs)、[`writer_test.rs`](./writer_test.rs)、[`writer_test_helpers_test.rs`](./writer_test_helpers_test.rs)；并核对对应 Go 测试 `writer_behavior_test.go`、`writer_core_test.go`、`writer_test_helpers_test.go`。
- 本任务仅新增文档，按计划不运行 Cargo。验收执行固定 11 章节结构命令，并人工确认本文覆盖文件存在原因、执行链、状态不变量、错误/资源边界、Go 差异和安全扩展入口。
