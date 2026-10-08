# `pkg/util/chunk/row_in_disk.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-util-chunk` crate；crate 边界由 `pkg/util/chunk/Cargo.toml` 定义，`pkg/util/chunk/lib.rs` 通过 `#[path = "row_in_disk.rs"] pub mod row_in_disk` 挂载模块并用 `pub use row_in_disk::*` 重导出其公开符号。它是 Chunk 子系统的按行临时落盘层：上游 `pkg/util/chunk/row_container.rs` 在内存压力触发 spill 后，将内存 `List` 中的 Chunk 逐个交给 `DataInDiskByRows`，随后把读写切换到磁盘表示。

该文件同时包含两组职责：`DataInDiskByRows` 及其序列化辅助类型负责真正的双临时文件落盘；`ReaderAt`、`SliceReaderAt`、`ReaderWithCache` 提供“底层随机读 + 未刷盘尾缓存”的独立抽象。当前 `DataInDiskByRows` 直接使用 `tempfile::NamedTempFile`，并未调用本文件的 `ReaderWithCache`。

## 核心职责

1. `DataInDiskByRows` 把每个非空 `Chunk` 逐行编码到数据文件，并把每行在数据文件中的绝对起始偏移编码到第二个偏移文件（`DataInDiskByRows::Add`、`chunkInDisk::WriteTo`、`offsetsOfRows::WriteTo`）。
2. 它保存 Chunk 边界元数据，使调用者既能按 `RowPtr { ChkIdx, RowIdx }` 随机恢复一行，也能按原 Chunk 下标顺序恢复整块（`GetRowAndAppendToChunk`、`GetChunk`）。
3. `diskFormatRow` 在内存 `Row` 与磁盘行格式之间转换；磁盘格式先写每列 8 字节小端长度，再依次写非 NULL cell 的原始字节，长度 `-1` 表示 NULL（`convertFromRow`、`rowInDisk::{WriteTo, ReadFrom}`、`diskFormatRow::toRow`）。
4. `diskTracker` 统计本实例成功登记的数据文件字节数与偏移文件字节数；`Close` 和 `Drop` 负责释放临时文件并清零统计。
5. `ReaderWithCache::ReadAt` 在底层 `ReaderAt` 返回 EOF 且请求未读满时，从指定绝对偏移开始的尾缓存补齐数据，保留随机读的“已读字节数 + EOF”语义。

## 主要符号

- `defaultChunkDataInDiskByRowsPath`、`defaultChunkDataInDiskByRowsOffsetPath`：两个 `NamedTempFile` 的文件名前缀。
- `DataInDiskByRows`：公开的磁盘行容器。公开入口包括构造 `New`，状态查询 `Len`、`NumChunks`、`NumRowsOfChunk`、`GetDiskTracker`，写入 `Add`，读取 `GetChunk`、`GetRow`、`GetRowAndAppendToChunk`，以及终止生命周期的 `Close`。`initDiskFile`、`dataReader`、`getOffset` 是内部 I/O 辅助函数。
- `chunkInDisk`：单个 Chunk 的写入中间态，记录本次写入起点 `offWrite` 及逐行偏移；`WriteTo` 会复用上一行的 `diskFormatRow` 容量。
- `offsetsOfRows`：偏移数组包装，`WriteTo` 逐项写出小端 `i64`，每行固定占偏移文件 8 字节。
- `rowInDisk`：单行编解码器；`numCol` 只在读取时决定长度头的项数，写入时使用其 `diskFormatRow`。
- `diskFormatRow`：磁盘行的逻辑表示，`sizesOfColumns` 对所有列逐一记长度，`cells` 只保存非 NULL 列；`toRow` 将其追加到可复用 Chunk。
- `convertFromRow`：将 `Row` 浅取原始 cell 字节并生成 `diskFormatRow`；传入旧格式可复用两个 Vec 的容量。
- `ReadAtError`、`ReadAtResult`、`ReaderAt`：本文件自己的随机读结果模型与 `Send + Sync` 接口。
- `SliceReaderAt`：内存字节切片实现，用于验证偏移、短读和 EOF。
- `ReaderWithCache`：把底层 reader 与 `[cacheOff, cacheOff + cache.len())` 的尾缓存拼成一个逻辑地址空间。

本文件没有条件编译项，也没有在源文件内嵌测试；独立测试由 `pkg/util/chunk/lib.rs` 的 `#[cfg(test)] #[path = "row_in_disk_test.rs"]` 挂载。

## 执行流程

写入流程从 `DataInDiskByRows::Add` 开始。它先拒绝空 Chunk 和列数与 `fieldTypes` 不同的 Chunk，再由 `initDiskFile` 惰性创建两个临时文件。`chunkInDisk::WriteTo` 遍历每行，在写行之前记录 `offWrite + written`，调用 `convertFromRow` 提取 NULL 标记和原始 cell，再由 `rowInDisk::WriteTo` 写出所有列长度及非 NULL 载荷。数据文件 flush 成功后，`offsetsOfRows::WriteTo` 把对应绝对偏移追加到偏移文件并 flush。两个写入都成功后才追加 Chunk 行数、Chunk 首行全局序号并增加总行数与 Tracker。

单行读取由 `GetRow` 转发到 `GetRowAndAppendToChunk`。后者先验证 `RowPtr` 的 Chunk 和行下标，`getOffset` 用 `rowNumOfEachChunkFirstRow[ChkIdx] + RowIdx` 算出全局行序号，在偏移文件的 `ordinal * 8` 处读取绝对数据偏移；`dataReader` 重开数据文件并 seek；`rowInDisk::ReadFrom` 读取一行；最后 `diskFormatRow::toRow` 追加到调用者提供且未满的 Chunk，或新建容量 1024 的 Chunk。

整块读取 `GetChunk` 以原 Chunk 行数创建恰好容量的目标 Chunk，从该 Chunk 第一行偏移开始顺序读取 `rowCount` 行。这里不为每行再次查询偏移文件，因为数据文件中的行连续排列；每次反序列化后都把行追加到同一个目标 Chunk。

`ReaderWithCache::ReadAt` 先调用底层 `reader.read_at`。只有底层返回 `Eof` 且目标缓冲仍未填满时才进入缓存路径；它根据绝对请求位置、底层已读数及 `cacheOff` 定位缓存起点，复制可用尾字节，并按最终是否填满决定清除或保留 `Eof`。

## 数据与状态

`DataInDiskByRows` 的持久状态分三层：`fieldTypes` 是解码契约；`numRowsOfEachChunk`、`rowNumOfEachChunkFirstRow`、`totalNumRows` 是内存索引；`dataFile`、`offsetFile`、`dataOffWrite`、`offsetOffWrite` 是文件与追加游标。对第 `c` 个 Chunk，`rowNumOfEachChunkFirstRow[c]` 指向其在所有已落盘行中的首个序号，偏移文件的对应槽位再指向数据文件字节位置。

每行数据布局为 `num_columns * 8` 字节长度头，之后紧跟所有非 NULL cell。NULL 只消耗值为 `-1` 的长度槽，不占 cell 载荷。零长度非 NULL cell 的长度为 `0`，仍在 `cells` 中占一个元素；`toRow` 通过独立的 `cellOffset` 保持它与列序对应。字段类型不写入磁盘，因此读取必须使用构造实例时的同一 `fieldTypes`；`Add` 只验证列数，不比较逐列类型。

`dataOffWrite` 和 `offsetOffWrite` 独立于文件句柄当前 cursor，所有追加前都会显式 seek，因而随机读使用 `reopen` 的独立句柄不会改变后续追加位置。`Close` 会取走两个临时文件并把两个写偏移清零，但不会清空 Chunk 边界数组或 `totalNumRows`；因此它是终止操作，不应把同一实例在 `Close` 后继续读取或再次追加。

## 依赖与调用关系

上游真实生产调用者是 `pkg/util/chunk/row_container.rs`。`RowContainer::spillToDisk` 用内存 List 的字段类型构造 `DataInDiskByRows`，遍历所有内存 Chunk 调用 `Add`，然后保存为 `rowContainerRecord::inDisk`；spill 后，`RowContainer` 的 `Add`、`GetChunk`、`GetRow`、`GetRowAndAppendToChunkIfInDisk`、`NumRow`、`NumChunks`、`NumRowsOfChunk`、`Close` 和 `Reset` 分别委托或管理本文件的对应能力。更上层的聚合、连接等算子通过 `RowContainer` 使用它，而不是直接操作两个临时文件。

下游 crate 内依赖来自 `crate::{Chunk, Row, RowPtr, types, memory, disk, ChunkError, Result}`。`Chunk`/`Row` 提供原始 cell 的提取与追加，`RowPtr` 是两级位置键；`memory::Tracker`/`disk::NewTracker` 提供字节统计。外部直接依赖只有标准库文件 I/O 与 `tempfile::NamedTempFile`；`tempfile = "3"`、`memory-crate`、`disk-crate` 由 `pkg/util/chunk/Cargo.toml` 声明。

RustCodeGraph 的文件节点确认 `pkg/util/chunk/row_in_disk.rs` 已索引（506 行、47 个符号）并被多个文件引用；精确 `query` 找到 Rust 与 Go 的 `DataInDiskByRows`、`ReaderWithCache` 和 `convertFromRow`。图工具对这些重名符号的 `callers/callees` 查询未返回可用边，因此直接生产边进一步由 `rg` 与 `pkg/util/chunk/row_container.rs` 源码核对；没有发现除 `RowContainer` 之外直接构造 Rust `DataInDiskByRows` 的非测试代码。

## 错误处理与边界

公开 I/O 操作返回 crate 的 `Result<T, ChunkError>`；标准库 I/O 错误经 `From<std::io::Error>` 转成只保留文本的 `ChunkError::Io`。`Add` 显式拒绝零行 Chunk 和列数不匹配；`GetRowAndAppendToChunk` 显式拒绝越界 `RowPtr`；未初始化或已关闭文件分别产生 `disk data is empty` / `offset file is empty`。偏移文件短读导致 `UnexpectedEof` 时，`getOffset` 将其转换为“spilled file is broken”的稳定消息，其它 I/O 错误原样归入 `ChunkError`。

`rowInDisk::ReadFrom` 接受 `-1` 作为 NULL，拒绝小于 `-1` 的长度；正长度若超过剩余数据则由 `read_exact` 报错。不过它没有独立的最大 cell 长度限制，损坏文件中的巨大正数可能触发大内存分配。`diskFormatRow::toRow` 假设长度项数与 `fields`/Chunk 列数一致、非 NULL 长度项数与 `cells` 一致；这些内部不变量若被破坏会索引越界而 panic。

`NumRowsOfChunk` 和 `GetChunk` 没有显式 Chunk 下标检查，非法下标会在内存数组索引处 panic；只有按行入口主动返回范围错误。`Add` 也不是事务写入：数据写入或 flush 成功、随后偏移写入失败时不会回滚文件或写游标，且 Tracker/Chunk 元数据仅在完整成功后更新。调用者应把此类错误视为该实例不可继续安全使用。

`ReaderWithCache` 仅在底层明确返回 `Eof` 时拼接缓存；`Other` 会直接传播。负 offset 在 `SliceReaderAt` 中表现为 EOF。若不可信的 `ReaderAt` 声称读取字节数大于目标长度，包装器返回 `Other`，避免对目标切片做越界切分。

## 并发与资源生命周期

`Add` 与 `Close` 需要 `&mut self`，Rust 借用规则禁止通过同一安全引用并发修改；它们也没有内部锁。读取方法使用 `&self`，每次通过 `NamedTempFile::reopen` 得到独立文件句柄并独立 seek，因此完成写入后可以共享实例并发随机读。`pkg/util/chunk/row_in_disk_test.rs::testDataInDiskByRows` 使用 1、2、8 个线程共享 `Arc<DataInDiskByRows>` 并校验 10,000 行，提供了并发读证据。写入与读取同时发生没有由本类型声明同步协议；生产端由 `RowContainer` 的 `RwLock` 串行化访问。

临时文件惰性创建；`NamedTempFile` 在被 `take` 或结构析构时关闭并删除文件。显式 `Close` 同时清零 Tracker 和写游标，`Drop` 再调用一次 `Close`，因此资源释放是幂等的。`GetDiskTracker` 返回共享 `Arc`，外部可以在容器存活期间观察统计。成功 `Add` 以数据字节加偏移字节记账，`Close` 以当前 `BytesConsumed` 的相反数归零。

`ReaderAt: Send + Sync` 允许 `ReaderWithCache` 跨线程共享；其 `ReadAt` 只读取不可变的底层对象和 `Vec<u8>` 缓存，不维护 cursor。该缓存由构造者一次性交付，之后不会增长或 flush。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/util/chunk/row_in_disk.go`，核心双文件布局、绝对行偏移、`-1` NULL 哨兵、格式缓冲复用、按行/按块读回及尾缓存随机读算法均保持一致。Rust 的 `DataInDiskByRows::New` 对应 Go `NewDataInDiskByRows`；Rust 用所有权值与 `Option<Chunk>` 代替 Go 指针和 nil，用 `Result` 代替具名 error 返回值。

已验证的差异如下：

- Go 的 `diskFileReaderWriter` 在真实路径中负责临时目录初始化、缓冲写、checksum 及可选 AES-CTR 加密，并由其 reader 使用 `ReaderWithCache`；Rust `DataInDiskByRows` 当前直接读写 `NamedTempFile`，未接入 `checksum-crate`、`encrypt-crate` 或本文件的 `ReaderWithCache`。Rust 测试沿用 checksum/encrypt 测试名，但两组测试调用相同的纯文件/内存实现，因此不能作为加密已接线的证据。
- Go `initDiskFile` 先调用 `disk.CheckAndInitTempDir` 并把 Tracker label 拼入文件名；Rust 依赖 `tempfile` 默认目录和固定前缀，不调用 `CheckAndInitTempDir`。
- Go `GetChunk` 用 goroutine、channel 和 `bufio.Reader` 预读/解码；Rust 同步顺序读取，没有额外线程和显式缓冲层，语义相同但性能特征不同。
- Rust `Add` 比 Go 多做列数检查；Rust `GetRowAndAppendToChunk` 也显式检查 `RowPtr` 范围。两边的 `NumRowsOfChunk`/`GetChunk` 都把合法下标视为调用者前置条件。
- Go `Close` 显式关闭并删除路径；Rust 依靠 `NamedTempFile` 的 RAII 删除。Rust 还实现 `Drop`，即使调用者遗漏显式关闭也会清理。
- Go `ReadFrom` 会尝试按任意非 `-1` 长度分配；Rust 明确拒绝小于 `-1` 的损坏长度。

Go 行为测试位于 `pkg/util/chunk/row_in_disk_test.go`；Rust 对应独立测试位于 `pkg/util/chunk/row_in_disk_test.rs`。后者覆盖了字符串、整数、JSON、NULL、Chunk/Row 往返、范围错误、Tracker 清零、并发读以及跨底层/缓存边界的 EOF 行为，但没有验证真实 checksum/encryption 管线或 Go 的临时目录配置。

## 扩展指南

若扩展磁盘编码，优先同时修改 `convertFromRow`、`rowInDisk::{WriteTo, ReadFrom}` 和 `diskFormatRow::toRow`，并保持“每列一个小端 i64 长度、非 NULL cell 顺序紧随”的兼容性；任何格式变化都应先决定是否需要版本头，否则旧临时文件与新读取器混用会不可识别。新增字段校验或损坏输入防护时，应在 `pkg/util/chunk/row_in_disk_test.rs` 增加截断长度头、截断 cell、负长度和超大长度的独立用例。

若要实现 Go 的 checksum/encryption 等价能力，不应只改测试名；需要把 `DataInDiskByRows::{initDiskFile, Add, dataReader, getOffset}` 的具体 `NamedTempFile` 抽象为带缓存、校验和及加密的读写层，并让 `ReaderWithCache` 进入实际读取链。同步核对 `pkg/util/chunk/lib.rs` 已有 checksum/encrypt adapter、`pkg/util/chunk/internal/group1/lib.rs` 的相邻 I/O 抽象、`pkg/util/chunk/Cargo.toml` 依赖，以及 Go `diskFileReaderWriter` 的错误和 flush 语义。兼容风险集中在绝对偏移是在加密/校验层上方还是下方计算，性能风险集中在每行长度头、flush 频率和 `GetChunk` 缺少缓冲读取。

若要支持 `Close` 后复用，必须一起重置 `numRowsOfEachChunk`、`rowNumOfEachChunkFirstRow`、`totalNumRows` 和两个文件/写游标，并新增“关闭后重新 Add/读回”测试；当前契约应把 `Close` 当终止操作。若要支持并发追加或读写并行，应在本类型内增加清晰的锁与快照边界，不能只依赖 `reopen`，还需保证数据写入、offset 写入和元数据提交对读者原子可见。

常规功能扩展应同步检查 `pkg/util/chunk/row_container.rs` 的转发与 Tracker 差量统计，并扩展 `pkg/util/chunk/row_in_disk_test.rs`；若改变 Go 对齐语义，还应对照更新 `pkg/util/chunk/row_in_disk_test.go` 所表达的原测试意图。避免把测试放回生产源文件，保持当前独立测试模块结构。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含 `pkg/util/chunk/row_in_disk.rs`；`files --filter pkg/util/chunk/row_in_disk.rs` 与两次 `node --file ...` 完整读取了 506 行和 47 个符号；`query` 分别确认 `DataInDiskByRows`、`ReaderWithCache`、`convertFromRow` 的 Rust/Go 定义。精确 `callers/callees` 与后续 `explore` 未产出可用结果，调用边改由源码检索核实，未把空图结果解释为“无调用者”。
- 生产源码：`pkg/util/chunk/row_in_disk.rs`（全部实现）、`pkg/util/chunk/lib.rs`（模块挂载与重导出）、`pkg/util/chunk/row_container.rs`（spill、读写转发、锁和生命周期）。目标目录没有 `doc.go`，因此无额外 package contract 可读取。
- crate 声明：`pkg/util/chunk/Cargo.toml`（crate 名、`lib.rs` 入口、`tempfile`、磁盘/内存、checksum/encrypt 等依赖及 Go package 映射）。
- Go 对照：`pkg/util/chunk/row_in_disk.go`（布局、I/O 抽象、checksum/encryption/缓存路径、并发警告与关闭语义）。
- 测试证据：`pkg/util/chunk/row_in_disk_test.rs`（Rust 往返、范围、Tracker、并发读、cache/EOF 与基准入口），`pkg/util/chunk/row_in_disk_test.go`（Go 临时目录、真实 checksum/encrypt 组合及相同边界矩阵）。
- 直接调用检索：对非测试 Rust 文件检索 `DataInDiskByRows`，实际构造和方法调用集中在 `pkg/util/chunk/row_container.rs`；`pkg/planner/cardinality/row_size.rs` 仅按这种行格式估算宽度，`pkg/executor/aggregate/agg_hash_executor.rs` 仅在注释中描述 spill。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核文档没有把未接线的 checksum/encryption 写成现状。
