# `pkg/executor/importer/engine_process.rs`

## 文件定位

本文件是 `astersql-executor-importer` crate 中“单个导入 chunk 到本地 Lightning 引擎”的编排层。模块由 `pkg/executor/importer/lib.rs` 声明并公开再导出；crate 边界与直接依赖见 `pkg/executor/importer/Cargo.toml`。它位于表级导入编排与逐行解析/编码/投递之间：上游 `TableImporter::ImportSelectedRows` 在打开 data/index engine 后调用 `ProcessChunk`（`pkg/executor/importer/table_import.rs`），本文件取得 writer、编码器和输入源，再把实际处理交给 `BaseChunkProcessor::Process`（`pkg/executor/importer/chunk_process.rs`）。

该文件不是引擎导入、Region 切分或 parser/KV 编码算法的实现位置。它只为一次 chunk 选择处理路径并建立资源所有权；engine 的最终关闭、导入及清理由上层 `TableImporter` 继续完成。

## 核心职责

- 用 `ImportChunk` 抽象文件路径、偏移、行号范围、格式、压缩与时间信息，使编排函数不依赖具体 `Chunk` 类型。
- 用 `TableImporterRuntime` 抽象表元数据、keyspace、KV 编码器、文件 parser 和查询结果 channel；生产实现位于 `impl TableImporterRuntime for TableImporter`。
- `ProcessChunkAndLogger` 根据表元数据配置 data writer 的 `Local.IsKVSorted`，依次从 data/index engine 打开两个本地 writer，并处理第二个 writer 打开失败时第一个 writer 的清理。
- `ProcessChunkWithWriterAndLogger` 先构造编码器和 keyspace，再按 `DataSourceType::{File, Query}` 构造对应 processor 并执行。
- `ProcessChunk` 与 `ProcessChunkWithWriter` 是使用全局默认日志器的便利入口；带 `AndLogger` 后缀的函数保留调用方结构化日志上下文。

这些职责对应 `engine_process.rs` 中四个公开处理函数，而解析、编码、异步投递和 checksum 合并发生在 `chunk_process.rs` 的 `ChunkEncoder`、`DataDeliver` 与 `BaseChunkProcessor` 中。

## 主要符号

- `trait ImportChunk: Send + Sync`：chunk 元数据契约。`Key` 用于日志身份；`Path`/`FileSize`/偏移和行号区间用于 parser 与编码器；`SourceType`、`Compression`、`Timestamp` 和可选 `ParquetLocation` 描述输入。生产 `Chunk` 在 `table_import.rs` 中实现此 trait。
- `ImportChunk::GetSize`：普通格式返回 `EndOffset - Offset`；Parquet 的 offset 表示行而不是字节，因此返回完整 `FileSize`。`ParquetLocation` 默认 `None`，允许特殊实现覆盖。
- `trait TableImporterRuntime`：本文件所需的最小表级运行时接口。`GetKVEncoder` 和 `GetParser` 都按当前 chunk 构造资源；`TakeQueryChunks` 提供查询导入的共享接收端。
- `ProcessChunk`：从两个 `OpenedEngine` 开始的默认日志入口，转调 `ProcessChunkAndLogger`。
- `ProcessChunkAndLogger`：计算 data KV 是否可声明有序、打开 writer，并转调 writer 级入口。只有目标表同时满足非 `PKIsHandle`、非 `IsCommonHandle`、无 `AutoRandomBits`、无 `ShardRowIDBits`、无分区时，data writer 的 `IsKVSorted` 才为 `true`；index writer 始终用默认配置。
- `ProcessChunkWithWriter`：供已经持有 writer 的调用方使用默认日志器的入口。
- `ProcessChunkWithWriterAndLogger`：核心分派函数。它构造 `TableKVEncoder`，取得 keyspace，并建立 `FileChunkProcessor` 或 `QueryChunkProcessor` 后调用 `ChunkProcessor::Process`。

## 执行流程

1. 上游调用 `ProcessChunk`；该函数取得 `astersql_lightning_log::L()` 并进入 `ProcessChunkAndLogger`。
2. `ProcessChunkAndLogger` 读取 `TableInfo`，计算 `has_ordered_auto_row_id`，据此配置 data writer；随后以默认配置打开 index writer。
3. 若 data writer 打开失败，直接返回其字符串化错误；若 index writer 打开失败，先尽力关闭已打开的 data writer，再返回 index writer 错误。
4. 两个 writer 均成功后，其所有权被移动到 `ProcessChunkWithWriterAndLogger`。该函数首先调用 `GetKVEncoder`，然后复制 keyspace。
5. 文件源分支调用 `GetParser`。成功后，将 parser、encoder、keyspace、chunk 键和偏移范围、两个 writer、可选 checksum 与 collector 交给 `NewFileChunkProcessor`，并通过 `WithChunkLogger` 叠加真实 chunk 键与 `GetSize`；随后执行 `Process`。
6. 查询源分支调用 `TakeQueryChunks`，把接收端及同一组编码/写入资源交给 `NewQueryChunkProcessor`，随后执行 `Process`。查询 processor 使用固定身份 `import-from-select`，读取到 channel 断开为止。
7. `BaseChunkProcessor::Process` 在当前线程编码、作用域线程投递，通过有界同步通道传递批次；处理成功才把 chunk checksum 合并到共享 `KVGroupChecksum`，之后无论处理结果成功或失败都会尝试关闭 encoder 和两个 writer，并返回原始处理结果。

生产 Rust 中已确认的直接上游是 `TableImporter::ImportSelectedRows`。文件导入的更广泛调度可能通过其他表级流程持有 writer 级入口；本次证据没有确认额外的生产调用边，因此不把测试或预期架构写成已接线事实。

## 数据与状态

本文件自身不保存长期状态。`ImportChunk` 提供只读元数据；`TableImporterRuntime` 借用表级运行时；writer、parser 与 encoder 则按值移交给 processor，所有权边界清楚。

`GetSize` 的单位依源类型变化：CSV/SQL 等使用偏移差，Parquet 使用源文件字节数。该值只用于 chunk 日志字段；parser 的实际范围仍由 `Offset`/`EndOffset` 和运行时 parser 配置决定。`table_import_test.rs::chunk_size_uses_file_bytes_for_parquet_and_offsets_for_other_sources` 还证明普通格式不在本层校验偏移顺序，负差值会原样返回。

共享状态有两类：`Option<Arc<Mutex<KVGroupChecksum>>>` 在成功处理后合并本 chunk 校验和；`SharedQueryChunkReceiver` 是 `Arc<Mutex<mpsc::Receiver<QueryChunk>>>`，查询读取器每次持锁接收下一批。`collector` 是可选的线程安全 trait object，由编码阶段报告 accepted offset 及 processed bytes/rows。keyspace 以 `Vec<u8>` 复制进 processor，用于编码及校验和隔离。

## 依赖与调用关系

上游关系为 `TableImporter::ImportSelectedRows -> ProcessChunk -> ProcessChunkAndLogger -> ProcessChunkWithWriterAndLogger`；默认日志的 writer 入口则为 `ProcessChunkWithWriter -> ProcessChunkWithWriterAndLogger`。`Chunk` 和 `TableImporter` 的适配实现均在 `pkg/executor/importer/table_import.rs`，模块公开面由 `pkg/executor/importer/lib.rs` 再导出。

核心下游关系是 `OpenedEngine::LocalWriter`、`TableImporterRuntime::{GetKVEncoder, GetParser, TakeQueryChunks}`、`NewFileChunkProcessor`/`NewQueryChunkProcessor`，最终到 `ChunkProcessor::Process`。RustCodeGraph 对 `ProcessChunkWithWriterAndLogger` 的 callees 查询确认了这些分派边以及 chunk 的 `Key`/`Offset`/`EndOffset` 访问；对公开包装函数的查询确认它们只转调下一层入口。

crate 直接依赖由 `Cargo.toml` 声明：本文件实际使用 `astersql-lightning-backend` 的 engine/writer、`astersql-lightning-backend-encode::Context`、`astersql-lightning-mydump` 的 parser 与源类型、`astersql-lightning-verification` 的 checksum、`astersql-meta-model::TableInfo`、`astersql-dxf-framework-taskexecutor-execute::Collector` 及 `astersql-lightning-log`。本文件不直接访问 TiKV Region；注释中的 Region 仅说明本地 engine 后续会由上层导入。

## 错误处理与边界

所有公开处理入口返回 `Result<(), String>`。backend writer 错误在本层转为字符串；运行时的 encoder/parser/channel 错误直接传播；processor 的解析、编码、投递或取消错误也原样返回。创建 encoder 失败时尚未创建 processor，传入 writer 会随函数返回被丢弃，但本文件没有显式调用其 `Close`；其具体 Drop 行为不在本次证据中确认。

文件分支中，若 parser 创建失败，本文件显式尝试 `encoder.Close()` 后返回原错误；查询分支中，若 channel 未配置，同样关闭 encoder 后返回 `TakeQueryChunks` 的错误。关闭错误被忽略，不能覆盖首要错误。index writer 打开失败时，data writer 的关闭错误也被忽略。

一旦 processor 建立，`BaseChunkProcessor::Process` 保证处理成功、编码失败、投递失败时都尝试关闭 encoder 与两个 writer，并保持处理错误优先；关闭失败不改变成功结果或原处理错误。共享 checksum 只在编码和投递整体成功时合并，避免把失败 chunk 的部分结果计入最终校验。`DataSourceType` 是只有 `File`、`Query` 两项的 Rust enum，因此 `match` 穷尽；增加新变体会在编译期迫使此处补分支。

## 并发与资源生命周期

`ImportChunk: Send + Sync` 允许 chunk 元数据跨线程安全引用；collector 也要求 `Send + Sync`。本文件本身不创建线程，但它构造的 `BaseChunkProcessor` 会用 `std::thread::scope` 启动投递线程，同时在调用线程执行编码。两者通过固定容量的同步 channel 施加背压；发送端结束后投递线程排空并 join，线程 panic 会转换为错误。

writer 的所有权从 `OpenedEngine::LocalWriter` 移到 processor 的 `DataDeliver`。第二个 writer 打开失败时，本文件立即关闭第一个；正常进入 processor 后，`BaseChunkProcessor::Process` 在合并结果前后管理 encoder、data writer 和 index writer 的清理。`engine_process_test.rs` 用原子计数证明写入失败仍各关闭一次，并证明 writer close 错误不使成功处理失败。

共享 checksum 的互斥锁即使中毒也通过 `into_inner` 继续取得数据；查询 channel 的接收锁在 `QueryChunkEncodeReader::ReadRow` 中采用相同策略。调用方必须保证查询生产者最终关闭 channel或让接收结束，否则查询 processor 会继续阻塞等待；取消只有在编码/投递循环取得执行机会时被检查。

## 与 Go 版本的对应关系

同路径 `engine_process.go` 提供基准语义：两版都先判断 auto row ID 是否有序，分别打开 data/index writer，然后按文件或查询源构造 chunk processor，最后执行 `Process`；两版都让处理错误优先于延迟清理错误。Go 的 `TestProcessChunkWith`（`importer_testkit_test.go`）验证文件源会跳过首行并生成 checksum，查询源会消费多个 query chunk、生成连续 row ID 并得到一致 checksum。

Rust 为便于复用和测试，把 Go 的具体 `*Chunk`/`*TableImporter` 参数拆成 `ImportChunk` 与 `TableImporterRuntime` trait，并把默认日志入口与显式日志入口分开。Go 的 `ProcessChunk` 直接接收 logger，Rust `ProcessChunk` 使用全局 logger，`ProcessChunkAndLogger` 才接收调用方 logger。Rust 的 `DataSourceType` 是封闭 enum，Go 则以整数别名和 switch 表达。

资源语义基本对齐但可观察性不同：Go defer 关闭 writer、parser 和 encoder，并对关闭错误记 warning；Rust processor 对 encoder/writer 做无条件清理但忽略关闭错误，parser 由其 Rust 所有权/reader 生命周期释放，本文件没有对应的 close-warning。Rust 独立测试只直接覆盖 writer 清理；因此不能据现有测试声称 parser 关闭告警完全等价。Go 将 `diskQuotaLock` 显式传给 processor；当前 Rust 构造函数没有该参数，磁盘配额检查在 `TableImporter` 的独立后台流程中实现，这是结构差异而非本文件内的锁。

## 扩展指南

- 新增数据源类型时，先扩展 `DataSourceType`，再在 `ProcessChunkWithWriterAndLogger` 添加明确分支，并在 `TableImporterRuntime` 增加所需资源接口；不要把源特有逻辑塞入共同 writer 打开流程。同步新增独立的 `engine_process_test.rs` 分支测试及对应 Go 语义对照。
- 修改“可声明 KV 有序”的条件时，应改 `ProcessChunkAndLogger` 的 `has_ordered_auto_row_id`，并用能观察 `LocalWriterConfig.Local.IsKVSorted` 的独立 mock engine 回归 PK handle、common handle、AutoRandom、ShardRowID 与 partition 五类边界。错误地声明有序会带来正确性风险，过度保守只损失写入优化。
- 增加 chunk 元数据时，扩展 `ImportChunk`，同步 `impl ImportChunk for Chunk` 以及 `TableImporterRuntime::{GetKVEncoder, GetParser}` 中的适配复制。若字段影响日志大小，保留 Parquet offset 不是字节这一不变量，并扩展 `table_import_test.rs` 的 `GetSize` 用例。
- 改动资源关闭或错误优先级时，应在 `engine_process_test.rs` 增加 parser/encoder/writer 分别于“创建失败、处理失败、关闭失败”的回归矩阵；测试逻辑保持在独立测试文件，不内嵌到生产源。兼容风险在于改变首要返回错误，性能风险在于提前/重复 flush 或遗漏 close。
- 修改 checksum/collector 传递时，同时审查 `BaseChunkProcessor::Process`、`ChunkEncoder::accept_offset/send_batch` 和 Go `TestProcessChunkWith` 的 checksum/row ID 断言；失败 chunk 不得合并共享 checksum。

## 验证依据

- RustCodeGraph：`status` 确认项目索引可用且含 11,467 个文件；`node --file pkg/executor/importer/engine_process.rs` 成功读取目标文件全貌；对 `ImportChunk`、`TableImporterRuntime`、四个处理入口的 `query`，以及对四个入口的 `callers`/`callees` 查询用于核对符号和调用边。图未返回 Rust caller 时，使用 `table_import.rs` 中的真实调用点补证；图把同名 `L` 归到无关文件，因此日志器归属以目标源码和 Cargo 依赖为准。
- Rust 源与装配：`pkg/executor/importer/engine_process.rs`、`chunk_process.rs`、`table_import.rs`、`import.rs`、`lib.rs`；分别核对编排、处理器生命周期、生产适配/上游入口、封闭数据源 enum 和公开再导出。
- crate 声明：`pkg/executor/importer/Cargo.toml`，核对 crate 名、lib 根及本文件使用的直接依赖。
- Go 对照：`pkg/executor/importer/engine_process.go` 与 `pkg/executor/importer/importer_testkit_test.go::TestProcessChunkWith`，核对两条数据源路径、checksum、row ID 和 defer 清理意图。
- Rust 测试：`pkg/executor/importer/engine_process_test.rs` 覆盖处理失败仍关闭两个 writer、close 错误非致命；`pkg/executor/importer/table_import_test.rs::chunk_size_uses_file_bytes_for_parquet_and_offsets_for_other_sources` 覆盖 `GetSize` 的格式边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核唯一新增产物、源码路径和未验证边界。
