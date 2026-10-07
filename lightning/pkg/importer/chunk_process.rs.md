# `lightning/pkg/importer/chunk_process.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` crate；crate 入口 `lightning/pkg/importer/lib.rs` 以 `mod chunk_process` 装载它并通过 `pub use chunk_process::*` 重导出公开项。它处在 Lightning 导入流程的“源文件分块解析 → 行编码 → 交付进度”边界，同时也向重复键预检测和预信息读取提供解析器能力。

当前 Rust 接线需要分两层理解：`openParser` 已被 `lightning/pkg/importer/dup_detect.rs::dupDetector::addKeysByChunk` 用于生产路径，`parquet_source` 也被 `lightning/pkg/importer/get_pre_info.rs::ReadFirstNRowsByFileMeta` 复用；但 `newChunkProcessor`、`chunkProcessor::process`、`encodeLoop` 和 `deliverLoop` 在非测试 Rust 文件中没有调用者，现阶段主要由 `lightning/pkg/importer/chunk_process_test.rs` 验证。Go 版本的完整导入主链则从 `lightning/pkg/importer/table_import.go` 创建处理器并调用 `process`。

## 核心职责

- `openParser` 根据 checkpoint 的源类型创建 CSV、SQL 或 Parquet 解析器，并恢复到 `Chunk.Offset` / `PrevRowIDMax`；对 gzip 输入先解压，再通过 `ReadUntil` 恢复压缩流中的逻辑位置。
- `MydumpParser` 把 `astersql_lightning_mydump::Parser` 适配为本文件的 `DataParser`，包括错误类型转换、行数据类型转换、列名透传和行对象回收。
- `getColumnNames` 将“目标表列下标 → 源字段下标”的 `ColumnPermutation` 反转为源字段顺序的列名，并识别虚拟列 `_tidb_rowid`。
- `chunkProcessor::encodeLoop` 逐行调用编码器，将 KV、解析位置和 row ID 暂存在 `pending`；`deliverLoop` 以最后一项进度更新 chunk checkpoint 并调用 `saveCheckpoint`。
- `parquet_source` 把 `Storage::FileSize` 与 `Storage::OpenRange` 封装成 Parquet `SourceReader` 所需的按范围读取接口，并在每次访问前检查取消状态。

## 主要符号

- `MydumpParser`：私有适配器，持有 `Box<dyn parser_impl::Parser + Send>`、可选配置列名及 `last_row: Mutex<Option<Row>>`。`LastRow` 会克隆底层行并保存一份，`RecycleRow` 再将该行归还底层解析器。
- `parser_error`：把 `MydumpError` 转成 crate 错误；只有精确的 `MydumpError::Eof` 会设置 `class = Some("EOF")`。
- `convert_row`：映射 `I64`、`Bytes`、`Binary`；由于兼容层 `Datum` 尚无 NULL 变体，`Null` 暂表示为字节串 `\\N`，这是有损的兼容选择。
- `DataParser: Send`：处理器依赖的解析器抽象，暴露 `Pos`、`ReadRow`、`Columns`、`LastRow`、`RecycleRow` 和 `Close`。方法名保留 Go 风格。
- `ParsedRow`：编码边界的数据载体，包含 `RowID` 与 `Vec<Datum>`。
- `EofParser`：公开的最小 EOF 实现，主要用于测试；首次及后续 `ReadRow` 都返回分类为 `EOF` 的错误。
- `chunkProcessor`：持有解析器、chunk checkpoint、日志器、共享 `TableImporter` 和私有 `pending`。公开字段允许测试与兼容调用面直接检查状态。
- `newChunkProcessor`：仅组装已有 parser、checkpoint、logger 与 importer；与 Go 同名函数不同，它不调用 `openParser`。
- `openParser`、`getColumnNames`、`chunkProcessor::{process, encodeLoop, getDuplicateMessage, deliverLoop, maybeSaveCheckpoint, close}`：主要公开行为面。
- `parquet_source`：crate 内可见的 Parquet range reader 工厂。

文件没有模块级常量、条件编译项或独立 `impl Drop`；资源收尾依赖显式 `close` / `Close`。

## 执行流程

1. 调用方准备 `ChunkCheckpoint`。`openParser` 对 Parquet 创建 range reader 和 `ImportParser`，直接 `SetPos`；其他格式先用 `Storage::Read` 取数据，gzip 全量解压，随后创建 CSV 或 SQL parser。无压缩输入调用 `SetPos`；压缩且 offset 大于 0 时调用 `ReadUntil` 并设置 row ID。存在列排列时调用 `getColumnNames`，并对非 Parquet parser 执行 `SetColumns`。
2. `process` 建立日志任务，按顺序执行 `encodeLoop`、`deliverLoop`，任一步失败都会终止后续步骤，并把最终错误交给日志任务的 `End`。
3. `encodeLoop` 从 `Controller::encBuilder` 创建 encoder。每次读行前先检查 `Context::Err`，保证已取消时不额外消费一行；成功读取后取 `LastRow` 与 `Pos`，调用 `Encode(row, RowID, ColumnPermutation, chunk start offset)`，把编码对和进度压入 `pending`，最后回收行。
4. 解析器返回精确分类 `EOF` 时正常结束；任何其他错误经 `errors::Trace` 返回。测试特意证明仅错误文本包含“EOF”不会被吞掉。
5. `deliverLoop` 先检查取消，再查看 `pending.last()`。无待交付数据直接成功且不保存 checkpoint；有数据时以最后一行的 offset/row ID 更新 `Offset`、`RealOffset`、`PrevRowIDMax`，调用 `saveCheckpoint`，随后清空全部 pending。
6. `close` 调用解析器 `Close`，但忽略其错误；调用者必须显式执行。Rust `process` 本身不会自动关闭 encoder 或 parser。

## 数据与状态

`ChunkCheckpoint` 是恢复状态的中心：`Timestamp`、`PrevRowIDMax` 和系统变量进入 `EncodingConfig`；`Key.Path` 标识来源；`ColumnPermutation` 控制列映射；`Offset`、`RealOffset`、`PrevRowIDMax` 在交付成功边界一起推进。当前实现将 `RealOffset` 直接设为 parser 的 `offset`，没有单独跟踪 Go parser 的扫描/真实偏移。

`pending: Vec<deliveredKVs>` 是进程内缓冲。每个元素保存 encoder 产出的 `pairs`、解析位置和 row ID；它没有字节上限、分批阈值或背压。只有 `deliverLoop` 完成保存 checkpoint 后才整体清空；编码中途出错时已经编码的项仍留在处理器中，但 `process` 不会进入交付阶段。

`getColumnNames` 先建立源字段位置到目标表列位置的反向数组，再按源字段顺序输出名称。负值表示忽略目标列；目标下标等于 `tableInfo.Columns.len()` 时输出 `_tidb_rowid`。Rust 额外用边界判断忽略超出排列数组或表列数组的下标，避免 panic；这比 Go 版本对有效 permutation 的前置假设更防御性，但也可能静默隐藏非法映射。

## 依赖与调用关系

上游关系：

- `lightning/pkg/importer/lib.rs` 声明并重导出本模块。
- `dupDetector::addKeysByChunk` 调用 `openParser`，随后基于 `DataParser` 的列名、行、位置进行重复键预编码。
- `get_pre_info.rs::ReadFirstNRowsByFileMeta` 调用 `parquet_source` 读取 Parquet 样本。
- `chunk_process_test.rs` 直接覆盖处理器构造、解析、编码、交付、列映射和取消边界；未发现非测试 Rust 调用 `chunkProcessor::process`。

下游关系：

- 解析依赖 `astersql-lightning-mydump`，Parquet 路径依赖 `astersql-dumpformat-parquetfile` 和带固定 tag 的 `parquet` Git 依赖；gzip 解压使用 `flate2`。这些边界由 `lightning/pkg/importer/Cargo.toml` 声明。
- checkpoint 类型来自兄弟 crate `astersql-lightning-pkg-checkpoints`；存储通过本 crate 的 `storeapi::Storage`；编码通过 `Controller::encBuilder` 与 `TableImporter::encTable`。
- 进度最终调用 `lightning/pkg/importer/import.rs::saveCheckpoint`；待交付项复用同文件的 `deliveredKVs`，但 Rust 当前不调用 EngineWriter。

RustCodeGraph 的 `callees` 确认 `openParser → {getColumnNames, parquet_source, parser_error}`、`process → {encodeLoop, deliverLoop}`、`encodeLoop → {DataParser 方法, NewEncoder, Encode}`。图索引能定位本文件符号，但 `callers` 对这些 Rust 符号未返回边，因此上游接线又以 `rg` 和相邻源码核验。

## 错误处理与边界

- 未配置 `encBuilder` 时，`encodeLoop` 返回 `encoding builder is not configured`；encoder 构造或单行编码错误立即传播。
- EOF 判断依赖错误分类而非字符串。`parser_error` 只给真正的 `MydumpError::Eof` 设置分类，避免误把“malformed ... EOF ...”当正常结束。
- `openParser` 明确拒绝未知源类型和除 none/gzip（数值 1）之外的压缩类型；存储读取、gzip 解压、parser 定位和 Parquet 构造错误均转换为 crate 错误。
- 取消在 `encodeLoop` 每次 `ReadRow` 前、`deliverLoop` 修改 checkpoint 前以及 `parquet_source` 的文件大小/range 打开闭包中检查。取消不会消费下一行，也不会写 checkpoint。
- `deliverLoop` 空 pending 是幂等成功；非空时只依据最后一个元素推进进度。`saveCheckpoint` 没有返回值，因此本层无法确认持久化成功后再清空。
- `MydumpParser::last_row.lock().unwrap()` 在互斥锁中毒时会 panic；`getColumnNames` 对非法下标选择忽略；`close` 忽略关闭失败。这三点是扩展时需明确决定是否保持的兼容行为。
- `file_size = 0` 的 Parquet 路径由 `SourceReader::prepare` 通过 `Storage::FileSize` 补足；对应 Rust 测试覆盖 0 和实际大小两种情况。

## 并发与资源生命周期

`DataParser: Send` 允许解析器随处理任务在线程间移动；`TableImporter` 用 `Arc` 共享。`MydumpParser` 的 `last_row` 使用 `Mutex` 是为了让 `LastRow(&self)` 能记录待回收底层行，但整个 parser 仍由 `&mut self` 串行驱动，并不表示同一 parser 支持并发读。

当前 Rust `process` 是完全串行的：先把整个 chunk 编码进 `pending`，再一次性交付检查点。它没有 Go 版本中的有界 KV channel、独立 delivery goroutine、`minDeliverBytes` 聚合、磁盘配额读锁、EngineWriter 同步状态或数据/索引 writer 并行关系。因此大 chunk 的内存占用随编码结果线性增长，也没有生产者—消费者背压。

`parquet_source` 的闭包捕获克隆后的 `Context`、`Storage` 和路径，可由 Parquet reader 多次发起 range 请求。parser 的底层 reader、encoder 及 parser 都没有 RAII 收尾约束；调用路径应在所有退出分支显式 `Close`。`dupDetector::addKeysByChunk` 已在结果路径末尾调用 parser `Close`，而本文件 `process` 不负责 parser 关闭。

## 与 Go 版本的对应关系

直接对照文件为 `lightning/pkg/importer/chunk_process.go`，测试对照为同目录 `chunk_process_test.go`。

已对齐的语义包括：按 CSV/SQL/Parquet 选择 parser；从 checkpoint 恢复 offset 与 row ID；按 permutation 生成源列名并支持 `_tidb_rowid`；编码配置使用 SQL mode、timestamp、sys vars 和稳定的 auto-random seed；EOF 正常结束、其他解析错误传播；取消优先于继续读行；仅有进度时保存 checkpoint；关闭 parser。

Rust 当前的关键差异/缺口：

- Go `newChunkProcessor` 自行 `openParser` 并记录 file index；Rust 接收已经创建的 parser，且没有 index。
- Go `openParser` 使用完整 CSV 配置、字符集转换、ReadBlockSize、IO worker 和 zstd 解压；Rust CSV 只设置 header，`_ioWorkers` 未使用，只支持 gzip，并将非 Parquet 文件全量读入内存。
- Go 对压缩流总会 `ReadUntil(offset)`；Rust 仅 offset 大于 0 时执行，offset 为 0 时也不会显式恢复 `PrevRowIDMax`。
- Go `process` 同时运行 encode/deliver，并写 data/index EngineWriter；Rust 串行缓存，只更新 checkpoint。Go 的 checksum、columns、realOffset、指标、重复键详细报错、AddIndexBySQL 分支、ignore/extend columns、交付阈值、writer 同步和磁盘配额行为尚未移植到此 Rust 文件。
- Rust `getDuplicateMessage` 只格式化原始 key；Go 会重新定位冲突行并从 record/index KV 构造用户可读的键冲突错误。
- Rust 将 parser NULL 映射为 `Bytes("\\N")`，源码注释已明确这是因为兼容 `Datum` 缺少 NULL 变体，而非完整等价表示。

因此本文件是“部分可用的解析与兼容处理层”，不能仅凭同名符号认为完整 Lightning chunk restore 已迁移。

## 扩展指南

- 扩展格式或压缩方式时，从 `openParser` 分支接入，并同步 `chunk_process_test.rs` 的定位恢复、EOF、列名与压缩中点恢复测试；同时比较 Go 的 reader opener、字符集及压缩配置，避免只让样例通过。
- 接入真实导入主链时，优先设计 `chunkProcessor::process/encodeLoop/deliverLoop` 的有界流式传输与 EngineWriter 契约，而不是继续扩大 `pending`。必须保留“checkpoint 不领先于持久化 KV”的不变量，并补取消、writer 失败、部分批次、checksum、data/index 同步和恢复测试。
- 增强列处理时修改 `getColumnNames` 或调用前 permutation 构造逻辑，并同步 `test_get_columns_names`、`test_encode_loop_columns_mismatch`、`test_encode_loop_ignore_columns`；应决定非法 permutation 是报错还是继续保持静默忽略。
- 改动 parser 适配时重点检查 `MydumpParser::{LastRow, RecycleRow}` 的一取一还关系、NULL 的无损表示和锁中毒策略；测试必须放在独立的 `chunk_process_test.rs`，不要嵌入生产文件。
- 增加资源管理时可为处理器引入显式返回错误的关闭路径或受控 guard，但需与 Go 的 defer 顺序核对，确保解析/编码/交付错误优先级不被关闭错误意外覆盖。
- 若补齐 Rust/Go 等价实现，需逐项对照 `chunk_process.go` 的 encode/deliver 主循环，不应删除 AddIndexBySQL、扩展列、忽略列、重复键信息、指标、配额锁和 checkpoint 同步逻辑来做简化版本。

兼容性风险主要在 checkpoint 恢复语义、列排列和 EOF 分类；性能风险主要在全文件读取/解压和无上限 `pending`；正确性风险主要在当前交付未写 EngineWriter、`RealOffset` 简化以及关闭错误被忽略。

## 验证依据

- 生产源码：`lightning/pkg/importer/chunk_process.rs`（全部 509 行）；模块入口：`lightning/pkg/importer/lib.rs`；crate 声明：`lightning/pkg/importer/Cargo.toml`。
- 直接 Rust 调用证据：`lightning/pkg/importer/dup_detect.rs::addKeysByChunk` 调用 `openParser`；`lightning/pkg/importer/get_pre_info.rs::ReadFirstNRowsByFileMeta` 调用 `parquet_source`；`lightning/pkg/importer/import.rs` 定义 `saveCheckpoint`、`deliveredKVs`、`deliverResult`。
- Rust 独立测试：`lightning/pkg/importer/chunk_process_test.rs`，覆盖取消发生在读行前、精确 EOF 分类、错误传播、空交付不写 checkpoint、三行处理后的 offset/row ID/checkpoint、列排列/忽略列、gzip CSV、Parquet 从 offset 恢复及 `file_size` 为 0 的探测路径。
- Go 对照：`lightning/pkg/importer/chunk_process.go`、`lightning/pkg/importer/chunk_process_test.go`，以及主链调用 `lightning/pkg/importer/table_import.go::{newChunkProcessor, process}`。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点、1,848,419 边；`query` 定位 `newChunkProcessor`、`openParser`、`getColumnNames`、`parquet_source` 及 Rust/Go 同名符号；`callees` 验证上述主要内部调用边。路径过滤和 `callers` 未能返回本文件的完整上游边，故调用者以源码搜索复核。
- 本任务为纯文档分析，按计划不运行 Cargo 或代码测试；最终使用任务指定命令验证文档存在且恰有 11 个固定二级章节，并人工复查没有把 Go 的完整行为写成 Rust 已支持能力。
