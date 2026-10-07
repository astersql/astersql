# `pkg/dumpformat/parquetfile/reader_wrapper.rs`

## 文件定位

本文件说明对应源码 [`reader_wrapper.rs`](./reader_wrapper.rs)。它属于 `astersql-dumpformat-parquetfile` crate，由 `lib.rs` 以公开模块 `reader_wrapper` 装配。它提供两组能力：一组是基于 `Arc<Vec<u8>>` 的随机读/定位兼容层（`ReaderWrapper`、`InMemoryReaderBase`），另一组是把 Parquet footer 中的列块元数据归并为 row group 与逐列字节范围（`row_group_range_from_meta`）。生产解码路径实际使用后一组能力：`file_parser.rs::FileParser::new` 构造 `FileMeta`、计算范围，再交给 `source_reader.rs::SourceReader::set_ranges` 驱动整文件、row group 或列流式读取。

文件第 16～283 行的大段内容全部是注释，保存了 Go 对象存储 reader、并发预读和 `prepareReader` 的移植草案；它不参与 Rust 编译。当前真正可执行的实现从 `use crate::{Error, Result}` 开始。对象存储 `RangeOpener`、互斥缓存、关闭清理和实际 Parquet `ChunkReader` 接线位于 `source_reader.rs`，不能把注释草案当作本文件已经具备的运行能力。

## 核心职责

1. `ReaderWrapper` 把共享的内存文件包装成 `Read + Seek`，并提供 Go 风格的 `read_at`：小幅向前跳过时直接推进当前位置，回读或跨度超过 `DEFAULT_BUFFER_SIZE` 时显式 seek。
2. `RowGroupRange` 聚合每个列块的半开区间 `[start, end)`，同时保留 `column_starts`/`column_ends`，供下游区分 row group 缓存范围和单列流式边界。
3. `InMemoryReaderBase` 将一个经校验的 row group 切片复制到共享的 `Arc<Vec<u8>>`，之后按文件绝对 offset 提供完整读取语义。
4. `row_group_range_from_meta` 从精简的 `FileMeta` 中选择数据页或更早的字典页作为列起点，并为旧版 parquet-mr 复现 PARQUET-816 字典页头补偿规则。

`ROW_GROUP_IN_MEMORY_THRESHOLD` 声明为 128 MiB，但本文件的可执行代码并未读取该常量；生产缓存阈值实际由 `source_reader.rs::ROW_GROUP_THRESHOLD` 控制。`newReaderWrapper` 和 `rowGroupRangeFromMeta` 只是保留 Go 命名的公开别名。

## 主要符号

- `DEFAULT_BUFFER_SIZE: usize = 64 * 1024`：`ReaderWrapper::read_at` 决定“小跨度前跳”还是 seek 的上限；它不是实际分配出的 skip buffer。
- `MAX_DICT_HEADER_SIZE: i64 = 100`：旧 parquet-mr 列块尾部最多补偿的字典页头字节数。
- `ROW_GROUP_IN_MEMORY_THRESHOLD: i64 = 128 * 1024 * 1024`：与 Go 默认值对齐的公开常量，当前 Rust 生产路径不直接使用。
- `ReaderWrapper { data, position, last_offset, skip_capacity, closed }`：共享不可变字节数据，但每个 clone 拥有独立游标和关闭标志。公开入口为 `new`、`read_at`、`close`，并实现标准库 `Read`、`Seek`。
- `RowGroupRange { start, end, column_starts, column_ends }`：row group 总范围及按元数据顺序保存的列范围；`add` 同时扩展总边界并追加一列。
- `InMemoryReaderBase { buffer, row_group }`：复制并共享一个 row group 的字节；`new` 校验范围，`read_at` 使用文件绝对偏移读取。
- `ColumnChunkMeta`：仅保留数据页偏移、可选字典页偏移和压缩大小。
- `FileMeta`：保留源文件大小、是否为旧 parquet-mr，以及按 row group 分组的列元数据。
- `row_group_range_from_meta`：本文件在生产路径中的关键函数；`file_parser.rs::FileParser::new` 是直接调用者。

## 执行流程

`ReaderWrapper::read_at` 的流程是：先拒绝已关闭 reader；用请求 `offset - last_offset` 计算 gap；gap 为负或超过 64 KiB 时调用 `SeekFrom::Start`，否则在现有 `position` 上用 `checked_add` 前跳；随后通过 `Read::read` 拷贝数据。只有恰好填满调用方缓冲区才成功，并把 `last_offset` 更新为 `offset + read`。回读测试和小跨度前跳测试位于 `parser_test.rs::reader_wrapper_supports_forward_gaps_random_reads_and_close`。

`InMemoryReaderBase::new` 先要求 `0 <= range.start <= range.end <= file.len()`，再复制 `file[start..end]`。其 `read_at` 把绝对 offset 转成 `offset - row_group.start`：起点在 row group 之前时报带上下文的错误，落在或超过缓存尾部时报 `EOF`，跨越尾部时先复制可用前缀再返回 `EOF`，完整覆盖才返回字节数。

`row_group_range_from_meta` 先按 `index` 取得列集合，并以 `start = i64::MAX, end = 0` 初始化范围。每列优先选择正数且早于数据页的字典页 offset；长度从 `total_compressed_size` 开始。若 `old_parquet_mr` 为真，函数先校验 offset、length 非负且各自不大于源大小，再按 Go 语义计算剩余字节和最多 100 字节的补偿，最后调用 `RowGroupRange::add(start, start + length)`。空 row group 保留哨兵范围，调用方 `FileParser::new` 通过空 `column_starts` 跳过它。

生产链为：`FileParser::new` 从 `parquet-rs` metadata 提取 `created_by`、列页偏移和压缩大小 → 调用 `row_group_range_from_meta` → 校验总范围 → 整理 `(row-group range, column ranges)` → `SourceReader::set_ranges`。真正读取时，`SourceReader` 根据这些范围选择共享 row group 缓存或受列末端限制的流式 reader。

## 数据与状态

所有范围均按半开区间 `[start, end)` 理解。`RowGroupRange::add` 不排序、不去重，也不验证 `start <= end`；它信任上游元数据，并保持列的原始顺序。空列集合的 `start = i64::MAX, end = 0` 是与 Go 一致的可观察哨兵，不代表有效可读范围。

`ReaderWrapper` 的正常不变量是成功 `read_at` 后 `position == last_offset == offset + read`。标准 `Read::read` 只推进 `position`，标准 `Seek::seek` 同时更新两者；因此混用裸 `Read` 与 `read_at` 时，调用方需要理解 `last_offset` 记录的是定位优化基准。短读错误发生前 `Read::read` 已经推进 `position`，但 `read_at` 不更新 `last_offset`，失败后两者可能暂时分离。

`InMemoryReaderBase` 在构造时复制目标字节，因此不借用原文件；clone 只克隆 `Arc`，共享只读 buffer，而 `row_group` 作为普通值克隆。`ColumnChunkMeta` 和 `FileMeta` 是本 crate 的精简镜像，并非 `parquet-rs` 原始 metadata 类型。

## 依赖与调用关系

本文件直接依赖 crate 根部的字符串错误包装 `Error`/`Result`，以及标准库的 `Read`、`Seek`、`SeekFrom`、`Arc`；可执行实现没有直接调用 `parquet` crate。`Cargo.toml` 将该模块归入 `astersql-dumpformat-parquetfile`，而 `parquet` 依赖固定到 `astersql-parquet-v60.0.0-streaming-pages.1` tag；该依赖由 `file_parser.rs` 读取真实 footer metadata 后转换成本文件的 `FileMeta`。

已验证的直接上游是 `file_parser.rs::FileParser::new` 对 `row_group_range_from_meta` 的调用。已验证的直接下游是 `RowGroupRange::add`；计算结果随后传入 `source_reader.rs::SourceReader::set_ranges`。`ReaderWrapper`、`InMemoryReaderBase` 和 Go 风格别名没有在当前非测试 Rust 生产代码中形成对象存储读取主链；`parser_test.rs` 与 `reader_wrapper_test.rs` 是它们的主要调用者。注释中的 `store.Open`、failpoint、error group、`objstore::ReadDataInRange` 和 per-column wrapper 都不是当前可执行依赖。

## 错误处理与边界

- `ReaderWrapper::new` 拒绝超过文件长度的起始 offset；等于文件尾允许构造。
- `read_at` 在关闭后立即失败；短读会返回包含期望长度和实际长度的 crate 错误，而标准 `Read` 到 EOF 只返回 `Ok(0)`。
- `Seek` 拒绝负位置，但允许定位到文件尾之后；后续标准读取返回零字节，`read_at` 则转为短读错误。
- `close` 只设置布尔标志，只有 `read_at` 检查它；直接调用实现的 `Read` 或 `Seek` 仍可操作。这是当前接口边界，不应描述为底层资源已释放。
- `InMemoryReaderBase::new` 拒绝负起点、反向区间和超过文件的终点；`read_at` 区分“起点早于 row group”的非法 offset 与“到达/跨越末尾”的 `EOF`。
- `row_group_range_from_meta` 拒绝不存在的 row group index。旧 parquet-mr 分支只检查 start 和 length 各自不超源大小；当二者之和越过文件尾时，`wrapping_sub`/`wrapping_add` 保留 Go 的负 padding 效果，使尾端收缩到文件边界，`reader_wrapper_test.rs::old_parquet_range_matches_go_when_chunk_crosses_source_end` 固定了该行为。
- 新格式分支不在本函数内验证负 offset、负 length 或加法溢出，而是使用 wrapping 加法；生产调用方随后只校验总范围，逐列范围由 `SourceReader::set_ranges` 再检查。扩展时不能未经兼容性评估把这些行为悄然改成另一套错误规则。

## 并发与资源生命周期

`ReaderWrapper` 没有内部锁；其改变游标的方法要求 `&mut self`，不能把同一实例作为并发随机读器使用。多个 clone 共享不可变 `data`，但复制 `position`、`last_offset` 和 `closed`，因此各自推进和关闭互不影响。`close` 不释放共享字节，仅改变当前 clone 的状态。

`InMemoryReaderBase` 的 buffer 通过 `Arc` 共享且只读，多个 clone 可并发调用只需 `&self` 的 `read_at`；最后一个 `Arc` 被释放时内存才回收。本文件的可执行代码不创建线程、任务、通道或锁，也不执行对象存储 I/O。生产链中的并发/缓存生命周期属于 `SourceReader`：其 `Arc<Mutex<State>>` 管理 whole/group/stream buffer，`close` 清空缓存；注释草案中的并发度 8 分块预读没有在本文件落地。

## 与 Go 版本的对应关系

Go 对照文件是 `reader_wrapper.go`。Rust 保留了 64 KiB skip 思路、100 字节旧字典页头补偿、128 MiB row group 阈值、空组的 `MaxInt64` 起点，以及“字典页早于数据页时从字典页起算”的范围语义。`reader_wrapper_test.rs` 专门锁定空组哨兵、跨源文件尾的旧版 padding 和短读 EOF；`parser_test.rs` 还覆盖小跨度前跳、回读、关闭、字典页起点及正常旧版补偿。

当前实现并非 Go 的完整等价移植。Go `readerWrapper` 包装 `storeapi.Storage.Open` 返回的 `ReadSeekCloser`，可借 failpoint 替换 reader；Rust `ReaderWrapper` 只包装内存字节且 `close` 不传播底层错误。Go `inMemoryReaderBase.loadRowGroup` 以 error group 并发执行对象存储 range GET；Rust `InMemoryReaderBase::new` 只从已有 slice 同步复制。Go 的 `prepareReader`、`inMemoryReaderWrapper`、whole-file 策略和真实 close 生命周期在本文件仅存于注释；Rust 的相应生产实现主要集中在 `source_reader.rs`。因此本文件属于“部分语义对齐 + 元数据范围已接线”，不是完整对象存储 reader 移植。

## 扩展指南

若修改 Parquet 列范围算法，优先改 `row_group_range_from_meta`，并同步 `reader_wrapper_test.rs` 中空组、越界旧文件用例和 `parser_test.rs` 中字典页/旧版本用例；同时检查 `file_parser.rs::FileParser::new` 对负范围、空范围和 `SourceReader::set_ranges` 的假设。任何溢出处理、旧 parquet-mr padding 或空组哨兵的变化都可能改变 Go 兼容性。

若扩展随机读兼容层，应在 `ReaderWrapper` 中明确裸 `Read`、`Seek`、`read_at` 和失败后游标的共同契约，并在独立的 `reader_wrapper_test.rs` 增加短读后重试、seek 到文件外、clone/close 隔离等回归测试；不要把测试嵌入生产源文件。

若目标是真实对象存储性能或资源管理，应从 `source_reader.rs` 的 `RangeOpener`、`SourceReader::cached/get_read/get_bytes/close` 接入，而不是激活本文件的注释草案。阈值调整应同步评估 `source_reader.rs::ROW_GROUP_THRESHOLD`，因为只改 `ROW_GROUP_IN_MEMORY_THRESHOLD` 不会改变当前生产行为。将 Go 并发分块预读移植到 Rust 时还需处理取消、首错传播、切片并发写安全、峰值内存和底层 reader 关闭，并新增独立测试验证请求数与缓存上限。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/dumpformat/parquetfile` 确认相邻实现与测试；`explore "pkg/dumpformat/parquetfile/reader_wrapper.rs ReaderWrapper"`、`node --file ...reader_wrapper.rs` 和符号查询核对了可执行边界、主要符号及使用文件。
- 生产源码：`reader_wrapper.rs`；`file_parser.rs::FileParser::new` 第 262～305 行验证 metadata 转换、直接调用与 `SourceReader::set_ranges` 接线；`source_reader.rs` 验证实际缓存、流式读取、锁和关闭生命周期；`lib.rs` 验证模块公开性和测试装配。
- crate 配置：`pkg/dumpformat/parquetfile/Cargo.toml` 验证 crate 名、`lib.rs` 入口及带 tag 的 `parquet` Git 依赖。
- Go 对照：`reader_wrapper.go` 的 `readerWrapper`、`newReaderWrapper`、`inMemoryReaderBase`、`prepareReader`、`rowGroupRangeFromMeta`；`parser_test.go` 验证 Go 的阈值与预读/按需读取语义。
- Rust 测试：`reader_wrapper_test.rs` 的三个边界回归；`parser_test.rs::reader_wrapper_supports_forward_gaps_random_reads_and_close`、`in_memory_reader_and_row_group_ranges_cover_boundaries`、`old_parquet_mr_range_adds_dictionary_header_padding`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务规定的 `rg` 命令确认恰有 11 个固定二级章节，并人工复核现行实现与注释草案已明确分离。
