# `pkg/ingestor/globalsort/reader.rs`

## 文件定位

该文件属于 `astersql-ingestor-globalsort` crate（见 `pkg/ingestor/globalsort/Cargo.toml` 与 `lib.rs::pub mod reader`），位于全局排序结果从对象存储重新进入内存、归并和冲突处理的边界。它读取 globalsort/simplesst 约定的有序 KV data 文件及 stat 属性文件，提供两类入口：按半开键区间 `[start_key, end_key)` 批量装载，以及按文件顺序异步流出所有 KV。

直接生产调用链有三组：`engine.rs::Engine::load_range_batch_data` 为导入批次装载 KV，`merge_v2.rs::mergeOverlappingFilesV2` 为外部归并的每个范围窗口装载 KV；`dxf/importinto/collect_conflicts.rs::CollectConflictGroup` 和 `conflict_resolution.rs::ResolveConflictGroup` 使用 `ReadKVFilesAsync` 消费冲突文件。除此之外，`merge.rs::merge_overlapping_files_internal` 复用底层 `read_stream_pair`，`split.rs::PropertyMerge` 复用 `StreamStatsReader`。

## 核心职责

1. `read_all_data` 校验 data/stat/offset 切片形状，逐个 data 文件从给定偏移开始读取，只保留 `[start_key, end_key)` 内 KV，并保证失败时清空输出。
2. `MemKvsAndBuffers` 在读取阶段保留逐文件分组，随后由 `build` 展平；同时区分保留数据字节数 `size` 与起始键之前被跳过的 `dropped_size`。
3. `ReadKVFilesAsync` 在后台线程中按输入文件顺序解码，通过容量为 1 的同步通道向迭代器施加背压，并把读取、格式或取消错误作为迭代项返回。
4. `read_stream_pair` 同时解码 Rust 旧格式 `LegacyLittleEndian32` 和 Go 兼容格式 `GoBigEndian64`，是范围读取、异步读取及归并读取共享的记录边界。
5. `StreamStatsReader` 与 `get_read_ranges_from_props` 流式解析 Go stat 文件，为每个升序 job key、每个 stat 文件求“不晚于目标键”的最大 data 偏移；最多并发扫描 64 个 stat 文件。

## 主要符号

- `readAllDataConcThreshold: u64 = 4` 与 `ConcurrentReaderBufferSizePerConc = 64 MiB`：根据估算区间大小计算单文件期望并发度。当前结果传入 `read_one_file` 的 `_concurrency`，但尚未驱动实际并发 I/O。
- `CancellationToken(Arc<AtomicBool>)`：可克隆的协作式取消标志；`cancel` 用 Release 写，`is_cancelled` 用 Acquire 读，`check` 将状态转换为 `Error::Cancelled`。`from_cancellation_flag` 可与对象存储/上层循环共享同一原子标志。
- `MemKvsAndBuffers`：公开字段 `kvs`、`size`、`dropped_size` 加内部逐文件缓冲。`build` 按缓冲加入顺序展平，`clear` 重置所有数据和计数。
- `read_all_data(...) -> Result<()>`：多文件范围读取的事务式门面；要求 `data_files`、`stats_files`、`start_offsets`、`estimated_end_offsets` 等长。
- `read_one_file(...) -> Result<()>`：从 `Storage::open_at` 返回的流解码单文件，执行半开区间过滤和 `memory_limit` 检查。
- `AsyncKvReader: Iterator<Item = Result<KvPair>>`：同步迭代器外观；其 `Drop` 先断开接收端再 `join` 生产线程，防止提前停止消费时生产者永久阻塞。
- `ReadKVFilesAsync(...) -> AsyncKvReader`：按文件顺序启动后台生产者；函数名保留 Go API 风格。
- `io_error`、`read_stream_pair`：将嵌套的 `Error::Cancelled` 保真，其余 I/O 错误归一为 `Error::InvalidData`；仅在读取记录头之前遇到 EOF 才视为正常结束。
- `StreamStatsReader`、`FileProperty`：逐条读取 stat 属性体，产出 `RangeProperty` 与对应 data 偏移，不把整个 stat 文件载入内存。
- `get_read_ranges_from_props(...)`：验证 keys 升序，按文件并行扫描 stat 数据并转置为 `[key][file]` 偏移矩阵。

## 执行流程

同步范围读取从 `get_read_ranges_from_props` 开始。它为空 keys 直接返回空矩阵；非 Go 格式直接返回全零矩阵；Go 格式则建立最多 64 个 scoped worker。每个 worker 原子领取一个 stat 文件，`StreamStatsReader::next` 顺序解码属性。若当前属性首键大于当前目标键，目标索引前进且沿用上一偏移；否则更新当前目标偏移。属性耗尽后，剩余目标键沿用最后偏移。最后主线程把每文件列写入 `[key][file]` 结果。

`Engine::load_ingest_data_with` 或 `merge_v2` 取相邻 key 行作为开始/估算结束偏移，调用 `read_all_data`。该函数先 `output.clear()`，逐文件检查取消，使用 `estimated_end_offset - start_offset` 算出期望并发度，然后调用 `read_one_file`。单文件读取从 `start_offset` 打开流：键小于 `start_key` 时累计 dropped bytes；键大于等于 `end_key` 时立即停止；窗口内记录在加入前做累计大小溢出和内存上限检查。全部文件成功后，调用方执行 `MemKvsAndBuffers::build`，再在 engine/merge 层排序及去重或写出。

异步路径由 `ReadKVFilesAsync` 创建容量为 1 的 `sync_channel` 与后台线程。线程依次打开每个文件，`read_one_kv_file_to_channel` 解码一条、检查取消、发送一条；任一文件失败时先发送一个错误项再终止，所有文件完成或失败后 sender 被释放，迭代器收到通道关闭。消费者提前丢弃 reader 时，`Drop` 先释放 receiver，使阻塞的 `send` 以取消错误退出，再等待线程结束。

## 数据与状态

data 文件记录由长度头、key、value 顺序组成。`LegacyLittleEndian32` 使用两个小端 `u32`（8 字节头）；`GoBigEndian64` 使用两个大端 `u64`（16 字节头）。长度先转换为 `usize`，再用 `try_reserve_exact` 申请，避免转换溢出或分配 panic。`KvPair::encoded_size` 仅计算 key/value 载荷，不包含记录头，因此 `size`、`dropped_size` 与 `memory_limit` 都是载荷口径。

`MemKvsAndBuffers::kvs_per_file` 的顺序等于 `read_all_data` 的文件遍历顺序，但它不声明跨文件全局有序；`engine.rs` 与 `merge_v2.rs` 在 `build` 后显式按 key 排序。`size` 在每个文件成功结束时累计，单文件局部失败不会提交该文件；多文件任一失败由外层统一 `clear`，所以调用者不会看到半成品。

stat 文件每条记录是大端 `u32` 长度加属性体；体内依次为两段“大端 `u32` 长度 + key 字节”和三个大端 `u64`（size、keys、offset）。`get_read_ranges_from_props` 的结果维度为 `keys.len() × files.len()`，偏移只是安全的较早起点，真正键边界仍由 `read_one_file` 过滤。

## 依赖与调用关系

本文件只直接依赖标准库 I/O、原子、`Arc`、线程与 `mpsc`，以及 crate 根定义的 `Storage`、`RecordFormat`、`KvPair`、`RangeProperty`、`Error`、`Result`。crate manifest 的非 Windows 通用依赖不为本文件提供额外运行时组件；对象存储差异被 `Storage::{open, open_at, record_format}` 隔离。

上游关系（由 RustCodeGraph 文件索引及 `rg` 直接调用点核对）：`engine.rs` 调用 `get_read_ranges_from_props` 和 `read_all_data`；`merge_v2.rs` 同样按范围使用二者；`collect_conflicts.rs`、`conflict_resolution.rs` 调用 `ReadKVFilesAsync`；`merge.rs` 调用 `read_stream_pair` 实现多路归并；`split.rs::PropertyMerge` 调用 `StreamStatsReader::{open,next}` 合并属性流。

下游关系：所有记录读取最终落到 `Storage` 返回的 `Read` 流；取消传播到 crate 的 `Error::Cancelled`，格式/截断错误落到 `Error::InvalidData`，内存限制落到 `Error::OutOfMemory`。该文件不负责 KV 全局排序、重复键策略、SST 写出或对象存储重试，这些分别由 engine/merge、writer 和存储实现承担。

## 错误处理与边界

- 四组文件/偏移切片不等长时，`read_all_data` 返回 `InvalidArgument`；`stats_files` 当前只参与长度契约与并发估算循环，不在该函数内读取。
- `estimated_end_offset < start_offset` 使用 `checked_sub` 检出并返回 `InvalidData`，不会产生无符号回绕。
- `[start_key, end_key)` 是严格半开区间；空 `end_key` 不是“无上界”，对非空键会立即结束并得到空范围，`reader_test.rs::test_read_one_file_empty_end_key_is_empty_range` 固化了该契约。
- EOF 只有发生在下一条记录头的第一个字节之前才正常；不完整头、key、value 或 stat 属性体均是 `InvalidData`。stat key 声明长度越过属性体剩余空间时有显式截断检查。
- KV 长度不能转为 `usize` 时返回 `InvalidData("KV length overflow")`；预留失败及载荷累计溢出返回 `OutOfMemory`。后者的累计上限覆盖此前成功文件、当前文件已保留记录和待加入记录。
- 任意多文件读取错误都会清空 `output`。异步读取则向消费者交付首个错误后终止，不继续后续文件。
- `get_read_ranges_from_props` 拒绝非升序 keys；互斥锁 poison 转换为 `Error::Poisoned`，首个 worker 错误被保留。空文件产生全零偏移。非 Go 记录格式不解析 stat 文件，返回全零以从文件头读取。
- `ReadKVFilesAsync` 的 token 不会在 reader 被丢弃时自动 `cancel`；安全退出依靠 receiver 断开使生产线程的发送失败。

## 并发与资源生命周期

`CancellationToken` 可跨线程共享，范围循环、stat 扫描和异步文件循环在 I/O/记录边界检查取消；正在执行的阻塞 `Read` 本身不能被该令牌强制中断。

`get_read_ranges_from_props` 使用 `std::thread::scope`，worker 数为 `min(files.len(), 64)`，作用域结束前全部 join。文件索引通过 `AtomicUsize` 分配；每文件结果先写入 `Mutex<Vec<Option<Vec<u64>>>>`，首错写入独立 mutex。发生错误后其他 worker 会在领取下一文件前观察 failure，但已开始的阻塞读取仍需自然返回。

`ReadKVFilesAsync` 拥有一个 `JoinHandle`；容量 1 通道把生产者领先量限制为一条 KV。正常消费到 EOF 时 sender 随线程退出关闭；提前 drop 时先断 receiver、再 join，顺序不可颠倒，否则生产者可能阻塞在满通道上。线程 panic 的 join 错误当前被忽略，消费者只观察到通道关闭。

同步 `read_all_data` 当前逐文件串行执行。它虽计算 Go 对照中的单文件并发度，但 `read_one_file` 参数名为 `_concurrency` 且未使用；因此 `readAllDataConcThreshold` 与 64 MiB 预算目前只保留决策语义，不带来实际并行读取或额外缓冲。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ingestor/globalsort/reader.go`，偏移查找语义对照 `pkg/ingestor/simplesst/util.go::GetReadRangeFromProps`。Rust 保留了：期望并发度公式及阈值、偏移指向目标键之前所以仍需过滤、严格 `[start,end)`、错误时清空输出、按文件顺序异步流出、上下文/令牌取消、stat 文件最多 64 路扫描，以及目标键超过后续属性时沿用最后偏移。

已知差异必须视为当前实现事实：Go `readAllData` 最多启动 1000 个文件读取 worker，并在大文件上启用 `simplesst.KVReader` 并发模式；Rust 串行读文件且忽略 `_concurrency`。Go 使用 `membuf.Pool` 管理小/大块并在 GCS 成功后 reset；Rust 使用拥有所有权的 `Vec`，没有 GCS 特例、读取指标或日志任务。Go `ReadKVFilesAsync` 将错误交给外部 error group、返回纯 KV channel；Rust 把错误放进 `Iterator<Item=Result<KvPair>>` 并由 reader 自身 join 线程。

Rust 还显式支持两种记录格式；Go 生产路径使用 simplesst 的 Go 格式。Rust `get_read_ranges_from_props` 位于本文件并直接解析 stat 流，而 Go 通过 simplesst 包实现。Rust 的 `reader_test.rs` 对照 Go 的 `reader_test.go`，覆盖多文件、单文件、大范围窗口与异步顺序，但 Rust fixture 直接 `encode_kvs`，不复现 Go writer 的自动分文件和 membuf 行为。偏移算法更完整的边界用例位于 `pkg/ingestor/simplesst/util_test.rs`/`.go`，不是本文件的直接单元测试。

## 扩展指南

若实现真正的并发 data 读取，应从 `read_all_data`/`read_one_file` 接入，保留输出全有或全无、文件结果可确定汇总、总内存限制原子/集中核算和取消后 join 的不变量；不能只启用 `_concurrency` 而让多个 worker 基于过期的 `output.size` 分别通过配额检查。测试应继续放在独立的 `reader_test.rs`，增加并发峰值、跨文件 OOM 回滚、取消和确定性汇总用例，并与 Go 的文件级/单文件内部并发语义核对。

若扩展编码格式，应修改 `RecordFormat`、`read_stream_pair` 及对应写出函数 `merge.rs::write_stream_pair`，同时验证完整 EOF、截断头、超大长度和跨格式读取；不能仅修改 reader。若改变 stat 编码或 seek 算法，应同步 `StreamStatsReader`、stat writer、`split.rs::PropertyMerge` 和 `get_read_ranges_from_props`，并增加本 crate 的独立 reader 测试，而不是把测试内嵌进生产源文件。

若改变异步接口，应保持“消费者提前退出不会死锁”这一 Drop 顺序，并检查 `CollectConflictGroup`、`ResolveConflictGroup` 的错误传播与取消联动。若需要在 drop 时主动取消，可在 `AsyncKvReader` 保存 token，但需区分用户取消与接收端正常结束，避免把成功读取误报为取消。

兼容风险主要是 Go/Rust 文件格式和空 `end_key` 语义；正确性风险集中在 offset 选择、截断输入及并发内存核算；性能风险集中在当前串行 data 读取、每条 KV 独立分配，以及容量 1 通道造成的高频线程同步。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go；`node --file pkg/ingestor/globalsort/reader.rs --offset 1 --limit 500` 返回完整 491 行源码及该文件被 21 个文件使用的信息；`query` 精确定位 `reader.rs::read_all_data`、`read_one_file`、`ReadKVFilesAsync`、`get_read_ranges_from_props`、`read_stream_pair`。这些符号的 `callers/callees` 未返回边，因此直接调用关系另用 `rg` 核验，未把空图结果推断为“无调用者”。
- 源码与 crate 边界：`pkg/ingestor/globalsort/reader.rs`、`lib.rs`、`Cargo.toml`；包级职责参考 `pkg/ingestor/doc.go`。
- 直接 Rust 调用证据：`pkg/ingestor/globalsort/engine.rs`、`merge_v2.rs`、`merge.rs`、`split.rs`、`pkg/dxf/importinto/collect_conflicts.rs`、`conflict_resolution.rs`。
- Go 对照：`pkg/ingestor/globalsort/reader.go`、`engine.go`、`merge_v2.go`，以及 `pkg/ingestor/simplesst/util.go::GetReadRangeFromProps`。
- 独立测试：`pkg/ingestor/globalsort/reader_test.rs` 与 `reader_test.go`；stat seek 的对照边界证据来自 `pkg/ingestor/simplesst/util_test.rs` 与 `util_test.go`。按任务约束未运行 Cargo。
- 人工复核结论：该文件存在于“外部排序文件 → 内存导入/归并/冲突处理”边界；主要执行、资源释放、错误回滚及当前并发缺口均能反向定位到上述符号和调用点。交付前另运行任务指定的 11 章节结构命令。
