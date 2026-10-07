# `pkg/dxf/importinto/encode_and_sort_operator.rs`

## 文件定位

本文件属于 `astersql-dxf-importinto` crate；crate 入口 `pkg/dxf/importinto/lib.rs` 以 `pub mod encode_and_sort_operator` 声明模块并公开再导出其符号。它位于 IMPORT INTO 分布式任务的 encode-and-sort 步骤：上游 `pkg/dxf/importinto/task_executor.rs` 的子任务执行路径计算 writer 内存和 block 大小、准备本地 engine 或对象存储，然后调用 `RunEncodeSortChunks`；全局排序产物随后以 `subtaskPrefix` 作为文件前缀进入 merge/write-ingest 流程。

`pkg/dxf/importinto/Cargo.toml` 将该目录定义为 `astersql-dxf-importinto`，并直接依赖 DXF operator/workerpool、executor importer、Lightning backend/encode/KV、simple-SST、object-store、auto-ID 和 table 等 crate。`nextgen` feature 只转发给 `astersql-config-kerneltype/nextgen`，本文件没有条件编译分支。

## 核心职责

本文件承担五组相互衔接的职责：

1. `AsyncEncodeSortOperator` 用固定并发度启动 worker 线程，从零容量 `SimpleDataChannel<EncodeSortTask>` 接收 chunk，并把首个 worker 错误写入共享 `PoolContext`、触发取消和输入结束。
2. `NewConfiguredEncodeSortFactory` 在每个 worker 所属线程内重建表、LOAD DATA controller、`TableImporter` 与 `SharedVars`，避免把可能非 `Send` 的 importer/AST 状态跨线程移动；channel 只传 `Plan` 与 `Chunk`。
3. `chunkWorker` 调用 `runImportMinimalTask` 完成一个 chunk 的行到 data/index KV 编码，并确保底层 writer 跨多个 chunk 复用、只在 worker 关闭时真正 flush。
4. 全局排序时，`BuildGlobalChunkWorker`、`GlobalDataEngineWriter`、`GlobalIndexSstWriter` 和 `ObjectStoreWriterSink` 把 importer writer 接口适配到 simple-SST 与真实对象存储，并汇总文件范围、冲突、checksum、文件数和 allocator 水位；本地排序时 writer 留空，由 importer 使用预先打开的 local engines。
5. `getWriterMemorySizeLimit` 与 `subtaskPrefix` 提供与 Go 版本一致的资源分配和对象路径约定。

## 主要符号

- `EncodeSortTask { Plan, Chunk }`：线程间传输的最小任务信封；不携带 importer 或 writer 状态。
- `EncodeSortWorker` / `EncodeSortWorkerFactory`：可替换的 worker 抽象及线程内构造器，`HandleTask` 处理 chunk，`Close` 负责最终资源收束；`ConfiguredEncodeSortRuntime::WorkerFactory` 也使测试或宿主可以替换默认实现。
- `ConfiguredEncodeSortRuntime`：保存 controller/importer service 工厂、共享 importer service 的 `OnceLock`、对象存储、logger、可选 local engines、collector 和可选 worker factory。`importer_service` 保证服务只构造一次并在 worker 间共享。
- `ThreadOwnedEncodeSortWorker`：持有真实 `chunkWorker`、`Option<SharedVars>`、logger 与 `GlobalWriterSummaries`；每次 `HandleTask` 临时取出 `SharedVars` 构造 `importStepMinimalTask`，完成后放回，`Close` 合并 checksum/allocator 最大值并关闭 `TableImporter`。
- `AsyncEncodeSortOperator` / `newEncodeAndSortOperator`：实现 `WithSource<EncodeSortTask>` 和 `Operator` 的并行驱动器；`concurrency.max(1)` 保证至少一个 worker。
- `RunEncodeSortChunks`：文件的主公开入口。它创建汇总器和 operator，顺序发送 `step_meta.Chunks`，等待 worker 结束，再把 checksum、最大 ID、data/index 排序元数据与冲突计数写回 `ImportStepMeta`。
- `ObjectStoreWriterSink`：实现 simple-SST `WriterSink`，把 `write_file` 转给 `StorageRef::WriteFile`，并用监视线程把 encode context 的取消传播到对象存储 context。
- `GlobalDataEngineWriter`：把 `Rows2KvPairs` 逐项写入 data SST；关闭时保存 `WriterSummary` 并返回 `flushed: true`。
- `GlobalIndexSstWriter`：实现 `RoutedIndexWriter`，供懒创建的 `IndexRouteWriter` 按 index ID 写 SST。
- `GlobalWriterSummaries`：以 `Mutex` 保护 data/index 元数据、checksum 和 allocator 最大值，以 `AtomicI64` 统计文件数。
- `BuildGlobalChunkWorker`：构建 data writer 及按 index ID 懒创建 writer 的 factory，配置 keyspace prefix、内存、block size、重复键策略、对象路径与关闭回调。
- `chunkWorker` / `ChunkWriterProxy`：前者运行 minimal task 并最终关闭 writer；后者转发 append/sync，但把每个 chunk 的 `Close` 变为 no-op，避免首个 chunk 提前消费共享 writer。
- `maxWaitDuration`、`subtaskPrefix`、`getWriterMemorySizeLimit`：分别定义取消后 flush 的 30 秒上限、`{taskID}/{subtaskID}` 路径，以及 data/index writer 内存份额。
- 兼容结构 `encodeAndSortOperator` 仅保存任务标识、共享变量和 collector，并提供 `String`；实际可执行异步编排由 `AsyncEncodeSortOperator` 完成。

## 执行流程

1. `task_executor.rs` 从 `StepResource` 调用 `getWriterMemorySizeLimit`，计算 block size，并按 `Plan.IsLocalSort()` 决定是否预先打开 data/index local engines；随后调用 `RunEncodeSortChunks`。
2. `RunEncodeSortChunks` 创建 `GlobalWriterSummaries`，优先使用运行时注入的 `WorkerFactory`，否则调用 `NewConfiguredEncodeSortFactory`；它建立零容量 channel、设置 source 并 `Open` operator。
3. `AsyncEncodeSortOperator::Open` 为每个并发槽启动线程。线程先在自身内部调用 factory；所有 worker 都通过 ready channel 报告初始化成功后，`Open` 才返回，任何初始化错误都会结束 source 并作为错误返回。
4. 默认 factory 克隆 task meta，验证 `TableInfo`；本地排序必须存在预开 engines。它构造 table、AST 参数、controller 和 `TableImporter`，生成 UUID worker ID；全局排序调用 `BuildGlobalChunkWorker`，本地排序则建立无外部 writer 的 `chunkWorker`。每个线程拥有独立 `SharedVars`，但 importer service 可通过 `OnceLock` 共享。
5. 主线程逐个把 `Plan` 克隆和 `Chunk` 发送到无缓冲 channel。worker 收到任务后，经 `ThreadOwnedEncodeSortWorker::HandleTask`、`chunkWorker::HandleTask` 到 `runImportMinimalTask`；任一处理错误调用 `PoolContext::OnError`、结束 source，并使其他 worker 看到取消。
6. 全局排序的数据行经 `ChunkWriterProxy` 到 `GlobalDataEngineWriter::AppendRows`，索引行经 route writer 按 index ID 到 `GlobalIndexSstWriter`。simple-SST writer 关闭时通过回调把每个 writer 的 summary 合并到 `GlobalWriterSummaries`。
7. 输入发送完毕后 source 被标记完成，`AsyncEncodeSortOperator::Close` drain 并 join 所有线程。worker 的 `Close` 才真正关闭 data/index writer；若原 context 已取消，`chunkWorker::Close` 改用新的 30 秒 context，使必要的对象存储 flush 仍可进行。
8. 所有线程结束后，`RunEncodeSortChunks` 先检查 `PoolContext::OperatorErr` 和发送错误，再把聚合 checksum、每种 allocator 的最大水位、data/index meta 和全部冲突 KV 数写回 `step_meta`。

## 数据与状态

任务级输入是 `TaskMeta`，chunk 级输入是 `EncodeSortTask`。`RunEncodeSortChunks` 只在所有 worker 成功退出后更新传入的 `ImportStepMeta`，因此失败路径不会发布一组被当作成功结果的最终聚合字段。

每个 worker 的 `SharedVars` 包含独立 `TableImporter`、checksum、排序 meta、冲突计数和文件计数等可变状态。`ThreadOwnedEncodeSortWorker` 用 `Option::take` 在一次调用期间显式转移所有权，再从 minimal task 取回；缺失状态会返回 `encode sort worker state is missing`。跨 worker 只共享 `GlobalWriterSummaries`、importer service、对象存储和可选 local engines。

全局排序文件路径由 `subtaskPrefix(task_id, subtask_id)` 加 writer ID 组成：data 为 `data/{worker_uuid}`，index 为 `index/{index_id}/{worker_uuid}`。UUID 防止网络分区或重试时两个节点执行同一 subtask 而覆盖文件。simple-SST builder 同时应用 keyspace 字节前缀；summary 因而记录的是实际编码后的 key 范围。

内存预算使用 `MemoryPerCore * writerMemBudgetRatio / (index_group_count + 3)`：data writer 占三份，每个 index group 占一份。`DesiredTableInfo` 缺失时 index group 数为零；浮点结果向 `u64` 截断，测试固定了 0、1、2、4、5 组索引时的 Go 对齐值。

## 依赖与调用关系

直接上游是 `pkg/dxf/importinto/task_executor.rs`：它在 encode-sort 子任务执行中调用 `getWriterMemorySizeLimit` 和 `RunEncodeSortChunks`，并在后续 merge/write-ingest 请求中复用 `subtaskPrefix`。`pkg/dxf/importinto/lib.rs` 将本模块公开给 crate 内外使用。

主要下游为：

- `crate::subtask_executor::runImportMinimalTask`：真正执行单 chunk 的 importer 编码逻辑。
- `astersql_executor_importer`：解析语句参数、创建 controller/table importer、计算索引组、选择重复键策略并提供 index route writer。
- `astersql_dxf_operator` 与 `astersql_resourcemanager_pool_workerpool`：提供 channel、operator trait、取消和首错状态。
- `astersql_ingestor_simplesst`：提供有内存/block/重复键配置的 SST writer 和关闭 summary。
- `astersql_lightning_backend*`：提供 importer 所需的 `EngineWriter`、编码 context、rows 与 KV 转换。
- `astersql_objstore_storeapi`：持久化全局排序文件，并承接取消信号。
- `crate::proto`：承载 `TaskMeta`、`ImportStepMeta`、`SharedVars`、`SortedKVMeta` 与 summary 协议结构。

RustCodeGraph 已索引目标文件及 `newEncodeAndSortOperator`、`RunEncodeSortChunks` 等符号；其宽泛 `explore` 结果存在同名噪声，精确 `callers` 命令在本次环境超时。因此，上述直接上游边以 `rg` 在 `task_executor.rs` 的真实调用点补证，而不是推测图关系。

## 错误处理与边界

worker 初始化会显式拒绝 poisoned task-meta mutex、缺少 table info、缺少 table factory、AST/controller/importer 初始化失败，以及 local sort 未提供预开 engines。`AsyncEncodeSortOperator::Open` 等待全部 worker 初始化；失败 worker 调用 `OnError` 并结束输入，主线程返回首个初始化错误。

执行期错误遵循首错取消：`HandleTask` 失败后记录 operator error、`Finish` 输入并退出；其他 worker 仍执行各自的 `Close`，最终都被 join。发送端若 source 提前关闭会记录独立发送错误，但 operator error 的检查优先。线程 panic 不直接从 `Close` 返回，而被转为 `encode sort worker panicked` 写入 context；调用方必须像 `RunEncodeSortChunks` 一样读取 `OperatorErr`。

writer 关闭严格有序：先 data、后 index；data 关闭失败立即返回，避免忽略对象存储错误造成数据丢失。取消场景不能直接用已取消 context flush，因此改用 `Context::with_timeout(maxWaitDuration)`。`ObjectStoreWriterSink` 在写前检查取消，并在阻塞写期间以 10 ms 轮询把 encode 取消传给 store context。其监视线程总会在写调用返回后通过原子标志停止并 join。

锁中毒统一转换为字符串错误的路径包括 factory meta、对象 sink context、chunk writer 和 worker 关闭；`GlobalWriterSummaries` 的内部合并方法使用 `unwrap`，所以这些内部 mutex 若因持锁 panic 被毒化会继续 panic，这是当前实现边界。冲突总数使用 `wrapping_add`，极端溢出会按整数环绕，而非报错。

## 并发与资源生命周期

并发单位是 OS 线程，每个线程拥有一个 `EncodeSortWorker`；channel 为 bounded(0)，发送方与接收方逐项 rendezvous，形成天然背压。`Open` 的 ready barrier 避免部分 worker 尚未初始化时调用方就开始发送。`Close` 先结束 source，再 drain `JoinHandle`，保证 worker 析构和 summary 回调完成后才读取聚合结果。

非 `Send` 风险通过“在线程内构造并常驻”规避：channel 仅携带测试确认可 `Send` 的 `Plan` 和 `Chunk`，`SharedVars` 在同一个 `ThreadOwnedEncodeSortWorker` 内循环移动。跨线程聚合字段用 mutex/atomic；allocator 合并取各 worker 最大值而非求和，checksum 用 `KVGroupChecksum::Add` 合并。

每个 global worker 共享一个 `ObjectStoreWriterSink`，但 data/index writer 各自拥有 SST writer；index writer 按首次出现的 index ID 懒创建。`ChunkWriterProxy::Close` 是 no-op，真实 writer 生命周期覆盖该 worker 的全部 chunk。worker 关闭后再合并 checksum/allocator 并调用 `TableImporter::Close`。对象存储本身由外层 `task_executor.rs` 的 guard 管理：factory 创建的 per-subtask store 在 `RunEncodeSortChunks` 和后续处理结束后关闭，本文件不擅自关闭共享 store。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dxf/importinto/encode_and_sort_operator.go`，测试是同目录 `encode_and_sort_operator_test.go`。Rust 保留了 Go 的核心语义：每个 subtask 内并行处理多个 chunk；global sort 为每个 worker 生成 UUID；data/index 使用相同路径层次；index writer 懒创建；重复键策略分 data/index 计算；取消后用新 30 秒 context 关闭 writer；data 关闭错误优先；writer 内存按 `3 : 1` 份额分配。

实现结构有所不同。Go 用通用 `AsyncOperator`/`WorkerPool` 和指向共享 `SharedVars` 的 worker；Rust 以 `AsyncEncodeSortOperator`、显式 thread/channel 和 factory 实现，并把 importer 状态限制在线程内。Go 的 `simplesst.EngineWriter` 直接连接 global store；Rust 增加 `ObjectStoreWriterSink`、`GlobalDataEngineWriter` 和 `GlobalIndexSstWriter` 适配现有 Rust trait。Rust 还在 `RunEncodeSortChunks` 中显式把多 worker checksum、allocator 最大值和 writer summary 汇总回 step meta。

`encode_and_sort_operator_test.go::TestEncodeAndSortOperator` 验证取消与并发错误，`TestGetWriterMemorySizeLimit` 固定资源分配样例。Rust 的独立 `encode_and_sort_operator_test.rs` 覆盖这些意图，并额外覆盖共享 writer 跨 chunk 生命周期、对象存储真实文件、按 index 路由、keyspace 前缀、取消传播、初始化错误和聚合规则。

## 扩展指南

- 改变 chunk 调度、错误优先级或 worker 并发时，应修改 `AsyncEncodeSortOperator::{Open,Close}`、`RunEncodeSortChunks`，并同步 `async_encode_operator_cancels_on_worker_error` 与 `async_encode_operator_keeps_first_error_when_two_workers_fail`；必须保留 ready barrier、输入结束和完整 join。
- 新增 worker 级 importer 状态时，应放入线程内的 `SharedVars`/`ThreadOwnedEncodeSortWorker`，不要把非 `Send` 对象加入 `EncodeSortTask`。同步测试应继续放在独立的 `encode_and_sort_operator_test.rs`，不能内嵌到生产文件。
- 改变 global-sort 文件布局或 SST 参数时，应从 `BuildGlobalChunkWorker`、`subtaskPrefix` 和两个 writer adapter 接入，并同步检查 `task_executor.rs` 的 merge/write-ingest 消费方；路径变化具有重试兼容和历史文件读取风险。
- 新增 summary 字段时，应同时更新 `protocol_writer_summary`、`GlobalWriterSummaries` 的合并规则和 `RunEncodeSortChunks` 的发布逻辑，并明确字段是求和、取最大值还是按 index 分组。
- 新增重复键模式时，应同步 `duplicate_mode`、`getOnDupForConflictedKV`/`getOnDupForIndex` 的映射及 Go 对照；未知值当前回退为 `Ignore`，改变默认值会影响兼容性与数据安全。
- 调整内存公式时，应同步 Go 的 `getWriterMemorySizeLimit` 与两端测试样例，并评估并发数乘以单 worker data/index 总预算后的峰值；浮点截断也是兼容行为的一部分。
- 改变取消/关闭逻辑时，应同时验证阻塞对象存储写、已取消 context 下的 flush、data-first 失败短路和 store 所有权；不能仅验证内存存储的快速成功路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`node --file pkg/dxf/importinto/encode_and_sort_operator.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500` 完整读取 972 行；`query EncodeAndSortOperator`、`query newEncodeAndSortOperator`、`query RunEncodeSortChunks` 定位 Rust 与 Go 符号。精确 `callers` 查询超时，调用点改由 `rg` 核验。
- 生产源码：`pkg/dxf/importinto/encode_and_sort_operator.rs`、`pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/lib.rs`。
- crate/包契约：`pkg/dxf/importinto/Cargo.toml`、`pkg/dxf/importinto/job_doc.go`。后者描述 IMPORT INTO/DXF 的作业事务边界；本文件处理的是已进入分布式子任务后的编码排序阶段，不负责提交或取消作业记录。
- Go 对照：`pkg/dxf/importinto/encode_and_sort_operator.go`、`pkg/dxf/importinto/task_executor.go`。
- 独立测试：`pkg/dxf/importinto/encode_and_sort_operator_test.rs`、`pkg/dxf/importinto/encode_and_sort_operator_test.go`。已人工核对关闭顺序、跨 chunk writer 生命周期、对象存储文件、index 路由、取消传播、首错、初始化失败、checksum/allocator 汇总、路径与内存份额。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；结构验证要求本文恰有 11 个固定二级标题，并在交付前以任务文件给定命令执行。
