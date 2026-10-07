# `pkg/ingestor/globalsort/merge.rs`

## 文件定位

本文件属于 `astersql-ingestor-globalsort` crate（见同目录 `Cargo.toml`），实现全局排序 V1 路径的“重叠有序文件归并”阶段。它接收若干已经各自按 key 排序的 data 文件，把文件分成多个独立组并并行执行多路归并；每组输出一对 `.data`/`.stat` 文件，遇到 `OnDuplicateKey::Record` 时还可能输出 `.dup` 文件。

模块由 `lib.rs` 的 `pub mod merge` 公开。仓库内可见的生产调用链有两条：

- `pkg/dxf/importinto/task_executor.rs` 在 IMPORT INTO 的 merge-sort 子任务中构造 `NewMergeOperator`，通过回调合并 `WriterSummary`，调用 `MergeOverlappingFiles` 后把排序元数据与冲突计数写回子任务元数据。
- `pkg/session/runtime/modify_column_cloud_executor.rs::CloudStep::run_merge` 在云存储列变更回填中构造算子，以 `OnDuplicateKey::Error` 合并 `ExternalFields.data_files`，再把输出摘要写回 `meta_groups`。

因此本文件不是通用的“对所有输入重新排序”：它依赖“每个输入文件内部已经有序”的前置条件，只负责在文件之间恢复全局顺序。V2 的按 key 区间和 stat offset 归并位于相邻的 `merge_v2.rs`，不在本文件实现。

## 核心职责

1. 用 `getTargetFileCount`、`getGroupedTargetFileCount` 和 `splitDataFiles` 按并发度与 `MaxMergingFilesPerThread` 把输入路径尽量均匀地分组。
2. 用 `MergeOperator` 保存取消令牌、存储、内存预算、输出前缀、并发度、重复键策略、回调与冲突状态，并通过 worker pool 并行执行各组。
3. 在 `merge_overlapping_files_internal` 中只保留每个输入 reader 的一个堆顶元素，以最小堆语义完成流式 k 路归并，而不是把整个对象读入内存。
4. 按 `OnDuplicateKey` 处理同 key 多值，维护可查询的 `ConflictInfo`，并让 `Collector` 统计已成功处理的输入行和原始 key/value 字节。
5. 用 `StreamSummary` 同步生成 data 文件范围、行数、字节数和 range properties，并在 writer 完成后通过 `OnWriterClose` 发布 `WriterSummary`。

当前实现还保留了若干与 Go 接口对齐但尚未实际生效的参数：`merge_overlapping_files_internal` 的 `_check_hotspot`、`_reader_memory_size` 没有参与 reader 构造；`get_merge_part_size` 的结果被计算到 `_part_size`，但没有传给 `ObjectWriter`。调用方不能据此推断 Rust 已具有 Go 路径的热点并发读取或 multipart part-size 配置能力。

## 主要符号

- `MaxMergingFilesPerThread: AtomicUsize`：单个 worker 期望承担的最大输入文件数，默认 250；测试会临时修改它，因此使用原子类型。
- `MinUploadPartSize`、`MaxMergeReaderMemoryPerCore`：分别是 5 MiB 的最小上传分片和每 CPU 256 MiB 的 reader 预算上限。
- `Collector`：线程安全的进度接口。`Accepted` 有默认空实现，归并路径实际调用 `Processed(bytes, row_count)`。
- `SubtaskSummary` / `MergeCollector`：用 relaxed 原子加法累计处理字节与行数；`MergeCollector` 还维护本地 `metric_bytes`，但该字段没有公开读取接口。
- `OnWriterClose`：`Arc<dyn Fn(&WriterSummary) + Send + Sync>`，每个成功输出组关闭后回传摘要。
- `MergeOperator`：公开算子配置及运行状态。`String` 返回 `mergeOperator`；`conflict_info` 返回锁保护状态的快照；`Tune` 调整正在运行的 worker pool 并等待退役 worker。
- `MergeTask` / `MergeWorker`：内部 worker-pool 任务与执行器。任务包含单调 writer id、文件组和活跃组数；worker 将总 reader 预算除以活跃组数后调用内部归并函数。
- `MergeRun`：RAII 清理守卫。`Drop` 清除 operator 上的活动 pool、关闭输入 channel、释放并 join pool，并通知监控线程退出。
- `MergeOverlappingFiles`：公开批量入口，负责任务拆分、worker pool 生命周期、取消协调、结果排序和错误归一。
- `merge_overlapping_files_internal`：单组实际归并入口，负责打开流、堆归并、重复键策略、data/stat/dup 写出和摘要回调。
- `MergeHead`：堆元素；反转 key 比较，使 Rust 的最大堆表现为按 key 升序弹出的最小堆，同 key 时按 source index 保持确定性。
- `write_stream_pair`：按 `RecordFormat` 写长度头与载荷。旧测试格式使用小端 `u32`，Go 生产格式使用大端 `u64`。
- `StreamSummary`：跟踪 min/max、总字节/行数、当前及已完成的 `RangeProperty`，并序列化 `.stat` 文件。

## 执行流程

1. `NewMergeOperator` 把 `concurrency` 至少提升为 1，调用 `get_merge_reader_memory` 计算总预算，并初始化 writer id、冲突状态和空的 `running_pool`。
2. `MergeOverlappingFiles` 先检查取消；随后 `splitDataFiles` 计算目标组数。空输入直接成功返回空列表。
3. 函数创建容量为 1 的任务 channel、worker pool、结果 channel和共享首错槽位。每个 `MergeWorker` 克隆只读配置，共享取消令牌与冲突锁。
4. scoped producer 逐组分配 `next_writer_id` 并发送 `MergeTask`；monitor 轮询外部 token 与 pool context，任一取消即同时取消两者并关闭输入。
5. worker 的 `HandleTask` 调用 `merge_overlapping_files_internal`。成功时发送 `(id, data_path)`；失败时只保存第一个 typed `Error`、取消 token，再向 worker pool 返回字符串错误。
6. 主线程按预期组数收集结果到 `BTreeMap`。`MergeRun` 离开作用域时关闭 channel、释放 worker pool并等待工作线程结束；producer 和 monitor 也被 join。
7. 收尾优先返回共享的原始错误，其次返回 pool context 错误；结果不全或 token 已取消时返回 `Error::Cancelled`。成功时按 writer id 顺序返回 data 路径，而不是按 worker 完成顺序返回。
8. 单组内部先汇总 `file_size` 并计算 part size，再通过 `Storage::open` 打开每个输入。每个 reader 只读一个 head 放入 `BinaryHeap`。
9. 循环弹出最小 key，执行重复键策略和 `collect_processed`，再从同一 source 读取下一条。若下一 key 小于刚弹出的 key，立即返回 `InvalidData("merge input is not sorted")`；否则重新入堆。
10. 循环结束后刷新 `Remove` 模式下待定的唯一行，依次完成 data writer、写 stat、完成可选 dup writer，再构造 `WriterSummary`、调用回调并返回 data 路径。

## 数据与状态

- 分组不复制文件内容，只复制路径字符串。`getTargetFileCount` 先按每 worker 最大文件数求 shares，至少取并发度；当文件数少于 `2 * concurrency` 时改为至少两文件一组（单文件输入仍形成一组）。`splitDataFiles` 让组大小差最多 1。
- `next_writer_id` 是跨多次调用共享的 relaxed 原子计数器，所以同一 operator 重复运行时不会复用 `{prefix}/{id}`。结果使用 `BTreeMap<id, path>` 恢复确定性顺序。
- `conflict_info` 在 operator 生命周期内累积，不会在每次 `MergeOverlappingFiles` 开始时清零；`conflict_info()` 返回 clone 快照。
- `MergeHead` 同时持有一个 `KvPair` 和 reader 下标。主堆最多保留每个输入一个 KV，因此 KV 载荷内存随输入文件数增长，而非随总行数增长；reader 自身的缓冲行为由 `Storage::open` 实现决定。
- `previous` 保存上一 key；`ordinal` 记录当前 key 组内次序；`pending_single` 让 `Remove` 延迟写第一条，只有确认 key 组没有第二条时才输出。
- `StreamSummary.size` 只累计 `KvPair::encoded_size()`（key + value，不含长度头），`count` 是实际写入 data 的行数。`Collector` 则在重复策略处理成功之后、读取下一行之前按每个输入行累计；因此 `Remove` 丢掉的重复组仍计入 collector，而 `Error` 模式触发错误的第二条不会计入。
- range property 在 Go wire 格式下按 8192 keys 或 1 MiB payload+16-byte length header 达到任一阈值后截断；旧格式固定每 4 keys 一段。旧格式的 `.stat` 是空对象，Go 格式则写大端字段。

## 依赖与调用关系

上游生产调用者是 `pkg/dxf/importinto/task_executor.rs` 与 `pkg/session/runtime/modify_column_cloud_executor.rs`；两者都通过 `NewMergeOperator -> MergeOverlappingFiles` 使用本模块。`lib.rs` 将 `merge` 暴露为 public module，并把 `merge_test.rs` 作为独立的 `#[cfg(test)]` 模块接入，符合测试不与生产源混放的仓库约束。

关键下游依赖如下：

- `crate::reader::{CancellationToken, read_stream_pair}`：取消传播和逐记录解码。
- crate 根的 `Storage` / `ObjectWriter`：对象大小、流式打开、创建与最终发布；`RecordFormat` 决定 wire 编码。
- crate 根的 `KvPair`、`OnDuplicateKey`、`ConflictInfo`、`WriterSummary`、`RangeProperty`：数据、策略和元数据契约。
- `astersql-resourcemanager-pool-workerpool`：任务 channel、worker pool、动态 `Tune`、panic 上下文和 operator error。
- `astersql-ingestor-simplesst::onefile_writer::MaxUploadPartCount`：仅用于 `get_merge_part_size` 的上限计算。

`Cargo.toml` 还声明 engineapi、lightning-membuf 等 crate 级依赖，但本文件没有直接引用它们。相反，本文件直接依赖 workerpool 与 simplesst。RustCodeGraph 将目标文件标为被 `merge_test.rs` 和 `pkg/session/runtime/modify_column_cloud_store_test.rs` 使用；精确限定名的 callers/callees 命令未返回边，所以生产调用关系另由仓库文本检索及对应调用点源码核验。

## 错误处理与边界

- 已取消的 token 在公开入口和内部入口都会立即返回 `Error::Cancelled`；归并循环每次弹堆后也再次检查。
- `concurrency == 0` 被正规化为 1；`Tune(0)`、超过 `i32` 或未启动时调用 `Tune` 则返回 `InvalidArgument`。
- 任一 `Mutex` poison 在公开可恢复路径中映射到 `Error::Poisoned`。少数生命周期代码（worker 首错锁、`Drop`、线程 join）使用 `unwrap`，panic 会继续传播；worker task 本身通过 `TaskMayPanic::RecoverArgs` 为 pool 提供诊断上下文。
- 存储的 `file_size/open/create/write/finish`、reader 解码及 stat 编码错误都用 `?` 保留 crate 的 typed error。worker 层保存第一个原始错误，避免最终只剩 workerpool 字符串。
- 每个输入必须单调非降序；只有从同一 reader 取得下一条时会验证该约束。跨文件次序由堆自然合并。
- `OnDuplicateKey::Ignore` 名称表示“不报错”，实际保留所有重复行；`Error` 在第二条同 key 处返回携带该条 key/value 的 `DuplicateKey`；`Remove` 删除整个多行 key 组；`Record` 在 data 中保留前两条，仅把第三条及之后写入 `.dup` 并增加冲突计数。
- key/value 在旧 `u32` 格式中过大、stat property/key 超过 `u32`、stat property 大小计算溢出都会返回 `InvalidData`。Go 格式的长度使用 `as u64`，在当前平台上不会比 `usize` 更窄。
- 函数没有失败回滚：如果 data/stat/dup 中途失败，已经 `create` 或 `finish` 的对象可能留在存储中；调用者或更高层清理流程需负责处理孤儿文件。只有全部写出成功才调用 `OnWriterClose`。
- `getGroupedTargetFileCount(total, groups, ...)` 假设 `groups > 0`；传入 0 会除零 panic，它是供已知正分组数的计算路径使用，不是防御式公共 API。

## 并发与资源生命周期

`MergeOverlappingFiles` 的并行单位是“文件组”，而非单个 KV。pool 初始容量为 `op.concurrency`，但实际同时有意义的任务数最多是 `min(group_count, concurrency)`；该值也用于把总 reader 预算平均分给每个 task。`merge_operator_runs_file_groups_on_actual_parallel_workers` 通过记录 `Storage::open` 的线程 id 验证至少两个真实 worker 并发运行。

取消是双向协调的：monitor 把 token/context 任一取消传播给另一方并关闭输入；worker 失败也取消 token；主结果循环观察两种取消后退出。scoped threads、`MergeRun::Drop` 与 worker pool `Release` 保证公开函数返回前 producer、monitor 和 workers 已完成 join，不把后台读写留给调用者。

`running_pool` 只在一次活动运行期间保存 pool。`Tune` clone 该 `Arc` 后调用 pool 的 `Tune(concurrency, true)`，其中 `true` 要求返回前 join 退役 worker；`MergeRun::Drop` 仅在字段仍指向本次 pool 时清空它，避免误清除未来运行状态。不过同一 operator 并发启动多次没有显式拒绝机制，后启动者会覆盖 `running_pool`，所以调用方应把一个 operator 视为单次活动运行资源。

共享计数使用 relaxed 原子，因为它们只要求数值累加，不承载其他内存的发布关系。`stopped` 使用 Release/Acquire 配对控制 monitor 退出。冲突文件列表与首错使用 `Mutex`，writer id 使用 `AtomicUsize`。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ingestor/globalsort/merge.go`，测试对照为 `merge_test.go` 与 `merge_test.rs`。

保持一致的语义包括：并发度至少为 1；每核 reader 预算为 `min(256 MiB, memory_per_core / 5) * concurrency`；分组算法与 250 文件上限；part size 按“输入大小 + 每文件一个 block padding”向上除以最大 part 数并至少 5 MiB；collector 统计输入 KV 的 key/value 字节；每个 group 独立归并并在 writer 关闭时上报摘要；重复键策略由 `OnDuplicateKey` 控制。

实现形态存在以下差异：

- Go 用 `operator.AsyncOperator`/pipeline、UUID writer id、`simplesst.NewMergeKVIter` 和 `OneFileWriter`；Rust直接管理 worker pool，以递增整数作 writer id，并在本文件实现 `BinaryHeap` 归并、wire 写出与 `StreamSummary`。
- Go 的 reader 使用 `checkHotspot` 与 `memorySizePerGroup` 配置并发读取；Rust保留参数和预算计算，但内部当前使用 `Storage::open`，两项参数未生效。
- Go 把计算出的 part size传给 one-file writer；Rust当前只计算 `_part_size`，没有可配置 part size 的 `ObjectWriter` 调用。
- Go collector 可接 Prometheus common metric；Rust `MergeCollector` 只累加内部 `metric_bytes`，没有注册/导出 Prometheus counter。
- Go 测试用 failpoint覆盖内部错误、panic 与超时；Rust没有等价 failpoint，`merge_test.rs` 改为走真实取消、重复键错误及存储/格式边界，并显式测试真实 worker 并行。
- Rust额外支持 `LegacyLittleEndian32` 内存 fixture 格式；生产互操作由 `GoBigEndian64` 覆盖，测试 `merge_reads_and_writes_go_simplesst_wire_format` 验证读写一致。

这些差异说明接口对齐不等于全部性能机制已移植。修改时应优先保持 Go 的可观察行为，并把 hotspot、内存预算或 multipart 接线作为明确、可测试的独立工作，而不能仅移除下划线就宣称完成。

## 扩展指南

- 修改分组策略时，集中调整 `getTargetFileCount` / `splitDataFiles`，同步 `merge_test.rs::test_split_data_files` 及 Go 的 `TestSplitDataFiles` 用例；必须维持全覆盖、无重复、组大小差至多 1，以及每组文件数上限的性质。
- 增加重复键策略时，应修改 `merge_overlapping_files_internal` 中以 `previous`/`ordinal`/`pending_single` 为核心的状态机，同时明确 collector 是统计输入还是输出、冲突文件从第几条开始记录，并扩展 `test_merge_duplicate_modes_match_one_file_writer` 和 collector 测试。
- 改 wire 或 stat 格式时，应从 `write_stream_pair`、`StreamSummary::write/write_stats/finish` 接入，并同步 reader 侧解码、Go wire 互操作测试和 range property/offset 测试。不能只改变 data 而不更新 stat offset 语义。
- 真正接入热点 reader、reader 内存预算或 multipart part size 时，修改 `merge_overlapping_files_internal` 的相应下游抽象，并新增能观察实际 range 并发、内存上限或上传 part 配置的独立测试；当前下划线参数是明确的迁移缺口。
- 调整 worker 生命周期或动态并发时，围绕 `MergeOverlappingFiles`、`MergeRun::Drop`、`MergeWorker::HandleTask` 和 `Tune` 修改，验证首错类型不丢失、取消后全部 join、调小并发会等待退役 worker、输出顺序不依赖完成顺序。
- 增加失败清理时，应定义 data/stat/dup 哪些对象在各失败点可见，避免删除已由回调发布的成功对象，并用故障存储在独立 `merge_test.rs` 中覆盖部分 `finish` 失败。
- 上层元数据契约变化还需同步两个生产调用点：IMPORT INTO 的 `task_executor.rs` 摘要转换，以及 `CloudStep::run_merge` 的 `SortedMeta` 聚合。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/ingestor/globalsort/merge.rs`；`files --filter pkg/ingestor/globalsort` 确认源、Go 对照与独立测试；`node --file ... --offset 1/430` 阅读完整 732 行；`query` 精确定位 `merge.rs::NewMergeOperator`、`merge.rs::MergeOverlappingFiles` 与 `merge.rs::merge_overlapping_files_internal`。限定名 `callers/callees` 未输出调用边，故未把空图结果当作“无调用者”。
- 源码与模块证据：`pkg/ingestor/globalsort/merge.rs`、`lib.rs`、`reader.rs` 所暴露的取消/读取接口，以及 crate 根的 `Error`、`Storage`、`RecordFormat`、`WriterSummary` 定义。
- crate 证据：`pkg/ingestor/globalsort/Cargo.toml` 的 package 名、`lib.rs` 入口、Go package 元数据，以及 workerpool/simplesst 路径依赖。
- 上游证据：`pkg/dxf/importinto/task_executor.rs` 和 `pkg/session/runtime/modify_column_cloud_executor.rs` 中对 `NewMergeOperator` / `MergeOverlappingFiles` 的生产调用。
- Go 对照：`pkg/ingestor/globalsort/merge.go` 的 collector、内存规划、pipeline、merge iterator、one-file writer 和 part-size 逻辑；`merge_test.go` 的分组、内存、failpoint、统计及 writer summary 断言。
- Rust 测试：`pkg/ingestor/globalsort/merge_test.rs` 覆盖均匀分组、零并发、内存/part-size 计划、取消、重复键、collector、流式对象读取、Go wire 格式、排序往返及真实并行 worker；`sort_test.rs` 还从本地 global-sort 流程调用生产入口并检查摘要与读回。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 11 标题结构命令验证，并人工复核文档只描述当前源码可证实的行为和明确标注的未接线能力。
