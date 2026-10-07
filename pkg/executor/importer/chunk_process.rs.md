# `pkg/executor/importer/chunk_process.rs`

## 文件定位

本说明对应生产源码 [`chunk_process.rs`](./chunk_process.rs)。该文件属于 `astersql-executor-importer` crate；crate 根 `pkg/executor/importer/lib.rs` 以 `mod chunk_process` 挂载并 `pub use chunk_process::*` 导出其公共接口，包的 Go 来源由 `pkg/executor/importer/Cargo.toml` 中的 `package.metadata.porting.go-package = "pkg/executor/importer"` 标明。

它位于单个导入 chunk 的执行中段：上游 `pkg/executor/importer/engine_process.rs::ProcessChunkWithWriterAndLogger` 已取得表 KV 编码器、keyspace 和 data/index writer 后，按 `DataSourceType` 调用 `NewFileChunkProcessor` 或 `NewQueryChunkProcessor`；本文件负责“读取一行 -> 编码成表/索引 KV -> 分批 -> 写入两个 writer”。文件不负责打开或导入 Lightning engine，也不负责整表后处理；这些职责分别留在 `engine_process.rs` 和 `table_import.rs`。

此外，`IndexRouteWriter` 是全局排序路径的索引 writer 适配器。`pkg/dxf/importinto/encode_and_sort_operator.rs::newChunkWorker` 通过 `NewIndexRouteWriter` 为每个 `index_id` 创建独立的 `GlobalIndexSstWriter`，减少不同索引数据在后续归并排序时的范围重叠。

## 核心职责

1. 用统一的 `EncodeReader` 抽象两类输入：`ParserEncodeReader` 读取文件 parser，`QueryChunkEncodeReader` 从 SELECT 结果通道读取 `QueryChunk`。
2. `ChunkEncoder::encodeLoop` 调用 `TableKVEncoder::Encode`，按照 `min_deliver_bytes`（默认 96 KiB）或 `min_deliver_row_count`（默认 4096）聚合行，并构造 `EncodedKVGroupBatch`。
3. `EncodedKVGroupBatch::Add` 将记录键放入 `data_kvs`，将索引键按解码出的 `index_id` 放入 `index_kvs`，同时累计带 keyspace 语义的 `KVGroupChecksum`。
4. `DataDeliver::deliverLoop` 将 data KV 和按索引分组的 KV 分别交给 data/index `EngineWriter`。
5. `BaseChunkProcessor::Process` 用容量为 `maxKVQueueSize`（32）的同步通道并行执行编码与投递，统一合并校验和并关闭编码器和 writer。
6. `IndexRouteWriter` 在全局排序场景按 `index_id` 惰性创建并缓存底层 `RoutedIndexWriter`。

## 主要符号

- `RowToEncode { row, row_id, end_offset, start_pos }`：一行编码输入。文件源携带 parser 的起止偏移；查询源把两个偏移设为 `-1`。
- `EncodeReader::ReadRow`：顺序读行接口；接收可复用的 `Vec<Datum>`，以 `Ok(None)` 表示输入结束。
- `ParserEncodeReader` / `parserEncodeReader`：在 `parser.Pos() >= end_offset`、parser EOF 时结束；其他 parser 错误附加文件名和读取起点；`Drop` 最终调用 `Parser::Close`。
- `QueryChunk`、`SharedQueryChunkReceiver`、`QueryChunkEncodeReader`：SELECT 导入输入。reader 会跳过空 chunk，以 `row_id_offset + cursor`（首行为 offset + 1）生成行号，发送端全部释放后结束。
- `EncodedKVGroupBatch` / `NewEncodedKVGroupBatch` / `Add`：保存一批 data KV、`index_id -> KV` 映射、源行数和批次校验和。`row_count` 独立于记录 KV 数，不能用 `data_kvs.len()` 反推。
- `ChunkEncoder` / `newChunkEncoder`：持有 reader、`TableKVEncoder`、批次阈值、源偏移、计时、collector、keyspace 和本 chunk 累计校验和；关键方法为 `encodeLoop`、`accept_offset`、`send_batch`、`Close`。
- `DataDeliver`：持有 data/index writer；`deliverLoop` 消费批次，`Close` 尝试关闭两者并返回首个错误。
- `ChunkProcessor` / `BaseChunkProcessor`：对外的单 chunk 处理接口与默认实现。`NewFileChunkProcessor`、`NewQueryChunkProcessor` 分别装配文件源和查询源；`WithChunkLogger` 保留调用者日志上下文并追加 chunk key/size。
- `WriterFactory`、`RoutedIndexWriter`、`IndexRouteWriter`：全局排序的索引路由边界；`AppendRows` 要求输入实际类型为 `GroupedPairs`，按索引 ID 写入。
- `is_record_key`、`decode_index_id`、`formal_key_component`：KV 分类辅助函数。优先使用 `astersql_tablecodec`，同时兼容包含 `_r`/`_i` 标记的形式化测试键。

## 执行流程

文件导入路径如下：

1. `engine_process.rs::ProcessChunkWithWriterAndLogger` 从 `TableImporterRuntime` 获取 encoder、keyspace 和 parser，调用 `NewFileChunkProcessor`，再用 `WithChunkLogger` 补充日志字段并执行 `ChunkProcessor::Process`。
2. `BaseChunkProcessor::Process` 创建 `sync_channel(32)`，在 scoped delivery 线程运行 `DataDeliver::deliverLoop`，当前线程运行 `ChunkEncoder::encodeLoop`。
3. `ParserEncodeReader::ReadRow` 记录当前 `Pos`，读取 parser 行，以 `ScannedPos` 作为进度终点，把 parser datum 转为 encoder datum，并在复制完成后回收 parser 行。
4. `ChunkEncoder::encodeLoop` 在每行前检查 `Context::is_cancelled`，读取一行并调用 `TableKVEncoder::Encode(row, row_id)`；达到字节或行阈值时先接受文件偏移，再调用 `send_batch`。输入结束时也刷新非空尾批。
5. `send_batch` 对每行 `Pairs` 调用 `EncodedKVGroupBatch::Add`，更新 chunk 累计校验和，经有界通道发送批次，成功后调用 collector 的 `Processed(total_kv_bytes, source_row_count)`。
6. delivery 线程依次把 `data_kvs` 转为 `MakeRowsFromKvPairs` 写 data writer，再把 `index_kvs` 转成按 ID 有序的 `BTreeMap`/`GroupedPairs` 写 index writer。
7. 编码循环结束后主线程释放 sender，使 delivery 线程在队列耗尽后退出；两个结果以 `encode_result.and(deliver_result)` 合并。只有整体成功时才把 chunk 校验和并入共享校验和；无论成功失败都会尝试关闭 encoder 和两个 writer。

查询导入复用步骤 2、4 至 7，只把 reader 换成 `QueryChunkEncodeReader`。它持有 `Arc<Mutex<Receiver<QueryChunk>>>`，当前 chunk 用尽时阻塞接收下一批，空 chunk 会继续接收，通道断开即结束；查询路径没有文件偏移进度。

全局排序索引路径独立使用 `IndexRouteWriter::AppendRows`：对每个分组，首次遇到某 `index_id` 时调用 `WriterFactory`，随后逐 KV 调用对应 `RoutedIndexWriter::WriteRow`；`Close` 遍历所有已创建 writer，尝试全部关闭并返回首个错误。

## 数据与状态

- `ChunkEncoder::offset` 是最后接受的文件扫描偏移。`accept_offset` 以 `saturating_sub` 计算增量、更新 offset，并仅在偏移非负时调用 `Collector::Accepted`；查询源的 `-1` 因而不会产生读字节进度。
- `read_total_duration`、`encode_total_duration`、`DataDeliver::deliver_total_duration` 分别累计读取、编码、投递耗时；当前文件只维护这些状态，没有把它们作为返回值或结构化摘要导出。
- `batch_rows` 保存按源行生成的 `Pairs`，`batch_bytes` 使用 `Pairs::Size()` 决定刷新时机；`EncodedKVGroupBatch::row_count` 保存源行数。一个源行可能产生多个记录/索引 KV，因此两个计数语义不同。
- 批次校验和随 `Add` 分别调用 `UpdateOneDataKV`、`UpdateOneIndexKV`；`ChunkEncoder::group_checksum` 聚合已成功组批的校验和，共享 `group_checksum` 只在整个处理成功后更新。
- `index_kvs` 在批次内是 `HashMap`，投递前转为 `BTreeMap`，使交给 `GroupedPairs` 的索引组按 `index_id` 稳定排序。
- `IndexRouteWriter::writers` 的生命周期覆盖整个 writer；每个索引 ID 最多调用一次 factory，之后复用相同 writer。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 文件引用与直接引用搜索核对）：

- `engine_process.rs::ProcessChunkWithWriterAndLogger -> NewFileChunkProcessor/NewQueryChunkProcessor -> BaseChunkProcessor::Process`，是本地 engine 的正式主链。
- `table_import.rs::TableImporter::ImportSelectedRows -> ProcessChunk`，把 IMPORT FROM SELECT 送入上述查询分支。
- `dxf/importinto/encode_and_sort_operator.rs::newChunkWorker -> NewIndexRouteWriter`，是全局排序索引写入链。
- `chunk_process_testkit_test.rs` 和 `engine_process_test.rs` 直接构造处理器验证边界。

主要下游依赖：

- `astersql-lightning-mydump::{Parser, Datum}`：文件解析、位置和行缓冲生命周期。
- `crate::TableKVEncoder` 与 `astersql-lightning-backend-encode::{Context, Datum}`：行到 KV 的编码及取消状态。
- `astersql-lightning-backend-kv::{Pairs, GroupedPairs, MakeRowsFromKvPairs}` 和 `EngineWriter`：批次表示与本地/全局 writer 接口。
- `astersql-lightning-verification::{KVGroupChecksum, KvPair}`：分 data/index 的数量、大小及校验聚合。
- `astersql-tablecodec`：真实 TiDB 表键的记录/索引判别及索引 ID 解码。
- `Collector`：接受源字节增量和已处理 KV 字节/源行数。

RustCodeGraph 的 `query` 能精确区分同名 Go/Rust 符号，并确认目标文件含 62 个符号；本次索引上的精确 `callers/callees` 命令未返回边，因此调用关系又由索引的 `used by` 文件列表和上述直接引用位置交叉确认，不能把空图结果解释成符号未接线。

## 错误处理与边界

- 文件 reader 在达到 chunk `end_offset` 或 parser EOF 时正常结束；parser 其他错误格式化为 `encode <filename> at offset <read_position>: ...`，`ScannedPos` 错误直接转字符串。
- 编码错误包含 chunk 名与该行 `start_pos`；取消在下一轮编码前返回 `chunk encoding was cancelled`。
- `send_batch` 对空积压无操作；索引键无法解码时失败；接收端已停止时返回 `encoded KV delivery loop stopped`。collector 的 `Processed` 仅在发送成功后执行。
- delivery 在每个批次开始时检查取消，data writer 失败会阻止该批的 index writer；任一 writer 错误直接传播。通道正常断开表示成功完成。
- `BaseChunkProcessor::Process` 中编码结果优先：`encode_result.and(deliver_result)` 在两边均失败时保留编码错误。delivery 线程 panic 被转换为 `KV delivery thread panicked`。
- encoder/writer 的关闭错误在 `Process` 清理阶段被忽略，不覆盖处理结果；`DataDeliver::Close` 自身仍会尝试关闭两个 writer并能返回首个关闭错误，便于独立调用时观察。
- 查询 receiver 的 poisoned mutex 会恢复内部值；sender 断开被视为输入结束。reader 阻塞在 `recv()` 时不轮询 `Context`，取消只能在获得下一行后由 `encodeLoop` 观察，这是当前实现的资源边界，而非可立即中断的接收。
- `IndexRouteWriter::AppendRows` 对非 `GroupedPairs` 返回 `invalid grouped pairs`；factory/写入错误立即停止。`Close` 不因首个错误中断，仍关闭其余 writer，最后返回首错。

## 并发与资源生命周期

`BaseChunkProcessor::Process` 使用 `std::thread::scope`，所以 delivery 线程只能借用当前 processor 和 `Context`，并在 `Process` 返回前完成 join，不会遗留后台线程。容量 32 的 `sync_channel` 提供背压：writer 较慢时编码线程最多领先 32 个批次，避免无界内存增长。编码端退出后显式 `drop(sender)` 是 delivery 端退出条件。

`ParserEncodeReader` 独占 `Box<dyn Parser + Send>`，析构时尝试关闭 parser；`ChunkEncoder::Close` 关闭 `TableKVEncoder`。`BaseChunkProcessor::Process` 在成功、普通错误和 delivery panic 路径都会走关闭尝试。writer 由 `DataDeliver` 独占，关闭顺序为 data 后 index。

查询通道接收端通过 `Arc<Mutex<_>>` 共享；一次 `ReadRow` 在等待 `recv` 时持锁，因此多个 reader 即使共享同一接收端也会串行领取 chunk。当前 chunk 的行数据被克隆到复用缓冲后再编码，不跨通道借用。

共享 `KVGroupChecksum` 也以 `Arc<Mutex<_>>` 保护；代码对 poisoned mutex 选择恢复内部值。校验和仅在处理成功后合并，避免把失败 chunk 的部分结果计入整表状态。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/importer/chunk_process.go`。Rust 保留了 Go 版的核心结构：32 容量队列、96 KiB/4096 行双阈值、文件/查询 reader、data/index 分流、分组校验和、编码与投递并行、两个 processor 构造器，以及按索引 ID 路由的 `IndexRouteWriter`。

需要注意的当前差异：

- Go `rowToEncode` 用 `resetFn` 在编码后回收 parser 行；Rust 在 `ParserEncodeReader::ReadRow` 中先复制 datum，再立即 `RecycleRow`，所有权模型不同但目标相同。
- Go 查询 reader 用 `select` 同时等待 channel 与 `ctx.Done()`；Rust reader 的阻塞 `recv()` 不接收 `Context`，只能由外层循环在读行前检查取消。Rust 额外用 `while` 跳过空 `QueryChunk`，并有独立回归测试。
- Go batch 持有 `BytesBuf/MemBuf` 并在投递后回收；Rust `Pairs`/`KvPair` 使用拥有所有权的容器，没有对应显式 buffer recycle 字段。
- Go delivery 在磁盘配额读锁下写入并记录 Prometheus 指标/错误日志；Rust `DataDeliver` 没有磁盘配额锁和这些指标，只保留耗时、collector 与错误传播。因此不能从本文件宣称这部分 Go 行为已等价移植。
- Go error group 的派生 context 能让一侧错误取消另一侧；Rust 共享原 `Context`，单纯的 writer 错误不会在本文件内主动标记 context 取消。不过 receiver 被丢弃后，阻塞发送会失败并使编码端结束。
- Go 在 `group.Wait()` 后无条件把 chunk checksum 合入共享 checksum；Rust 仅在编码与投递整体成功时合并。Rust 的语义更明确地排除失败 chunk 的部分校验和。
- Go 的 processor task 日志含结束状态、耗时/校验和摘要和成功计数指标；Rust 当前只在开始时写 `process chunk start`，并保留 chunk/caller 日志字段。
- Go `IndexRouteWriter` 在每个 KV 内查找/创建 writer；Rust 每个索引组先查找/创建一次，再循环写该组。两者都按索引复用 writer；Rust 的 `Close` 返回 `flushed: true`，Go 返回空 flush status。

这些差异是当前源码事实，不应在只改文档的任务中补实现或判定为缺陷。

## 扩展指南

- 新增输入源时，优先实现独立的 `EncodeReader`，并在 `engine_process.rs::ProcessChunkWithWriterAndLogger` 的数据源分支装配；不要把解析逻辑塞入 `ChunkEncoder`。同步在独立测试文件（通常为 `chunk_process_testkit_test.rs` 或 `engine_process_test.rs`）覆盖 EOF、取消、空输入、行号和错误上下文。
- 修改批次策略时，入口是 `ChunkEncoder::{min_deliver_bytes,min_deliver_row_count,encodeLoop,send_batch}`。必须分别验证字节阈值、行阈值、尾批、单行多 KV，以及 `row_count` 不等于 data KV 数的情况；后者已有 `chunk_process_test.rs::encoded_batch_preserves_source_row_count_independently_of_record_kv_count`。
- 修改 KV 分类或 key 编码时，同步检查 `EncodedKVGroupBatch::Add`、`is_record_key`、`decode_index_id` 和 `KVGroupChecksum`。真实 tablecodec key 与形式化测试 key 都需要覆盖，错误发生前已累计的 `kv_bytes` 语义也应保持。
- 修改并发或取消行为时，要保持有界背压、sender 释放、scoped thread join、错误优先级和所有退出路径清理；特别需要增加“查询通道无数据时取消”和“delivery 先失败且 encoder 被背压”测试。
- 为全局排序增加索引 writer 能力时，在 `WriterFactory`/`RoutedIndexWriter` 边界扩展，并同步 `pkg/dxf/importinto/encode_and_sort_operator.rs` 的构造逻辑与其独立测试；需关注大量索引 writer 的资源占用、部分创建失败和多 writer 关闭错误。
- Rust 单元测试必须继续放在独立 `*_test.rs` 文件，由 `lib.rs` 的 `#[cfg(test)] mod ...` 挂载，不应内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/executor/importer/chunk_process.rs` 确认目标已索引，`node --file` 完整读取 635 行及其 62 个符号。
- 主要符号查询：`query BaseChunkProcessor`、`query NewFileChunkProcessor`、`query encodeLoop`、`query IndexRouteWriter`，用于区分 `pkg/executor/importer` 的 Go/Rust 同名实现；精确 `callers/callees` 无输出的限制已在“依赖与调用关系”记录。
- 生产源码：`pkg/executor/importer/chunk_process.rs`、`engine_process.rs::ProcessChunkWithWriterAndLogger`、`table_import.rs::ImportSelectedRows`、`lib.rs`、`pkg/dxf/importinto/encode_and_sort_operator.rs::newChunkWorker`。
- crate/移植边界：`pkg/executor/importer/Cargo.toml`；直接依赖包括 Lightning backend/encode/kv/mydump/verification、DXF collector 与 tablecodec。
- Go 对照：`pkg/executor/importer/chunk_process.go` 的 `parserEncodeReader`、`queryChunkEncodeReader.readRow`、`chunkEncoder.encodeLoop`、`baseChunkProcessor.Process`、`dataDeliver.deliverLoop`、`newQueryChunkProcessor` 和 `IndexRouteWriter`。
- Rust 测试：`chunk_process_test.rs` 验证源行数独立于记录 KV 数；`chunk_process_testkit_test.rs` 验证文件主流程、两类解析/编码错误、data/index writer 错误、空查询 chunk、索引 writer factory 错误、校验和/collector 与日志字段；`engine_process_test.rs` 覆盖上游装配调用。
- Go 测试：`chunk_process_testkit_test.go` 直接覆盖 `NewFileChunkProcessor` 与 `NewIndexRouteWriter`，用于核对移植测试意图。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务文件指定命令验证恰有 11 个固定二级标题，并人工复核文档能回答文件存在原因、主流程及安全扩展位置。
