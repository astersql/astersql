# `pkg/dxf/importinto/task_executor.rs`

源文件：[task_executor.rs](task_executor.rs)。本文只描述当前 Rust 实现；同目录 Go 文件用于语义对照，不代表两者内部类型必须一一同构。

## 文件定位

该文件是 `astersql-dxf-importinto` crate 的 IMPORT INTO **节点任务执行与步骤分派中心**。`lib.rs` 以 `pub mod task_executor` 声明并重新导出本模块；生产接线位于 `write_ingest_backend.rs::RegisterConcreteWriteIngestExecutor`，它最终调用本文件的 `RegisterImportExecutor`，把 `ImportInto` 任务类型注册到分布式任务节点框架。

本文件不负责规划子任务，也不实现具体的编码算子或物理 SST backend。它负责把节点框架的 `Task`/`Subtask` 转换为旧 `execute::StepExecutor` 接口，按步骤选择执行器，安排资源与取消信号，调用 `encode_and_sort_operator`、global sort 和 `WriteIngestBackend`，并把更新后的元数据交还框架。

## 核心职责

1. `GetImportStepExecutor` 解码 `TaskMeta`，覆盖七个 IMPORT INTO 阶段：编码排序、合并排序、写入摄取由本文件构造；import、post-process、collect-conflicts、conflict-resolution 通过 `ImportStepHost::NewStepExecutor` 交给宿主实现。
2. `EncodeSortStepExecutor` 计算 writer 内存、块大小和 Parquet reader 并发，运行 chunk 编码排序；local sort 管理 data/index 本地引擎，global sort 将完整 meta 外置到对象存储。
3. `MergeSortStepExecutor` 读取内联/外置 meta，按 KV 组选择重复键策略，执行重叠文件合并，汇总排序与冲突统计，再写回外置 meta。
4. `WriteAndIngestStepExecutor` 把排序结果组装成 `WriteIngestRequest`，依序关闭外部引擎、导入、读取冲突信息和清理；只有存在冲突时才重写外置 meta。
5. `EncodeSortNodeStepExecutor`、`ConflictNodeStepExecutor` 和各 `Extension`/注册函数衔接两套任务执行接口，并保证非 `Send` 的步骤执行器始终由单一 worker 线程持有。
6. 重复键错误统一转换为 SQL 执行器错误码，进度、对象存储请求数与写入字节计量写入 `SubtaskSummary`/meter recorder。

## 主要符号

- `SubtaskRequestRecorder`：构造时取得对象存储 `(get, put)` 快照，`Drop` 时用饱和减法累加 `GetReqCnt`、`PutReqCnt`；仅对 factory 创建的子任务专用 store 启用。
- `EncodeSortImporterHost`：为 `TableImporter` 抽出内存估算、本地引擎 open/import/cleanup/close 边界，测试可注入等价 host。`EncodeSortStepExecutor::{Init,run,Cleanup}` 是编码排序生命周期主体。
- `ImportStepHost`：提供已经绑定到任务运行时的 KV store、write-ingest backend 以及其余阶段执行器；`GetImportStepExecutor` 不按 task keyspace 再查 store。
- `MergeStoreAdapter`：把对象存储 `StorageRef` 适配成 `globalsort::Storage`；record format 固定为 `GoBigEndian64`，并实现 offset open、size/read/write/delete/list。
- `mergeExternalMeta`、`readMergeSortMeta`、`readWriteIngestMeta`：合并 planner 的内联 JSON 与对象存储中的大字段。内联字段先解析，随后 external JSON 覆盖同名字段，最后恢复内联 `ExternalPath`；write-ingest 还保留“是否显式存在 `range-job-keys`”这一 wire-level 信息。
- `MergeSortStepExecutor`：global sort merge 步骤实现；`on_close` 将 writer summary 转为本 crate 的 `SortedKVMeta`，并以互斥锁汇总。
- `WriteIngestRequest` / `WriteIngestBackend` / `WriteAndIngestStepExecutor`：执行层与具体 Lightning 外部引擎实现之间的稳定边界。
- `NodeFrameworkTask`、`RunWithNodeCancellation`：分别构造旧框架可消费的 `Task`，以及将 node context 的完成状态桥接为 `execute::Context` 取消信号。
- `EncodeSortNodeStepExecutor`：mpsc 命令代理。`new_import` 可分派全部七个阶段；`new` 只构造 encode-and-sort，供组合扩展使用。
- `ImportTaskExtension` / `ImportNodeTaskExecutor` / `RegisterImportExecutor`：统一生产注册入口；包装器确保 task metrics 恰好注销一次。
- `getAdjustedBlockSize`、`parquet_reader_concurrency`：前者在向上对齐浪费不超过 10% 时采用默认块，否则使用实际预算；零预算保留 Go 默认块。后者只对 Parquet 取最大文件估算峰值，以总内存的 `readerMemBudgetRatio`（0.3）限制 CPU 并发，估算失败或非正值时回退 CPU 并发。
- `importStepExecutor`：保留的较低层状态/`Collector` 实现，持有具体 `TableImporter`、索引策略、writer 内存和摘要；主 node 分派当前使用上面的三个独立 step executor，而非用该类型覆盖整个步骤链。
- `getOnDupForConflictedKV`、`getOnDupForKVGroup`、`getOnDupForIndex`：将导入模式及 data/unique/non-unique index 组映射为 Record、Error 或 Remove。
- `normalizeSubtaskErr`：识别 Lightning `CommonError` 因果链、规范化 shared error 或 global-sort duplicate error，并转换为 `ErrLoadDataDuplicateKeyConflict`；其他错误原样返回。
- `ingestCollector`：累计 ingest 字节；只有 data KV 组累计行数，所有非负字节都上报集群写入 meter。

## 执行流程

生产主链如下：

1. `write_ingest_backend.rs` 调用 `RegisterImportExecutor(runtime, host, retry_policy)`；注册闭包创建 metrics、安装 `ImportTaskExtension`，再用 `BaseTaskExecutor` 构造 `ImportNodeTaskExecutor`。
2. 节点框架向 `ImportTaskExtension::GetStepExecutor` 请求步骤执行器。`EncodeSortNodeStepExecutor::new_import` 启动专属线程，在该线程内用 `NodeFrameworkTask` 和 `GetImportStepExecutor` 构造真正的 `execute::StepExecutor`，并用 node CPU/memory 设置 `FrameworkInfo`。
3. `Init`、`RunSubtask`、summary、reset、cleanup 都经 `EncodeNodeCommand` 串行发送。运行子任务时会复制 node subtask，补齐旧 proto 字段，执行后把变化后的 `Meta` 返回原 subtask。

编码排序分支：

1. `EncodeSortStepExecutor::Init` 检查取消；没有测试 worker 且未注入 importer 时，`build_parent_importer` 从表元数据、SQL 语句和 controller services 构造 `TableImporter`。
2. `run` 取得共享或专用对象存储；专用 store 由 guard 关闭并记录请求差值。`read_meta` 在 `ExternalPath` 非空时读回完整 meta。
3. 从步骤资源计算 data/index writer 内存与块大小。首次子任务以 CPU 为初值，并可按最大 Parquet 文件的峰值内存降低并发。
4. local sort 打开 data/index engine；global sort 不打开本地引擎。10ms 轮询 watcher 将框架取消传给 worker pool，然后调用 `RunEncodeSortChunks`。
5. 失败时 local sort 清理全部本地引擎，以免重试复用 engine ID 报 already exists。成功时 local sort 依次关闭并导入 data、index engine；global sort 将完整 meta 写到 `<task>/<subtask>/meta.json`，subtask 只保留带 `ExternalPath` 的 meta。

合并排序分支：

1. `readMergeSortMeta` 合并内联和外置字段；资源决定并发与每核内存。
2. 根据目标/原表索引及 `KVGroup` 决定 duplicate policy，创建 global-sort merge operator。writer close 回调在 `Mutex<SortedKVMeta>` 下合并范围、文件和冲突信息。
3. `MergeOverlappingFiles` 完成后累加 processed/row count，更新冲突数，并无条件把结果外置到标准 meta 路径。

写入摄取分支：

1. `readWriteIngestMeta` 解码文件、range keys、timestamp 与冲突计数；兼容 base64 字符串和字节数组形式的 keys。
2. 由步骤内存、KV 范围、文件统计、duplicate policy 和 subtask prefix 组装 `WriteIngestRequest`；若 planner 没有显式生成 job keys，则以 split keys 作为 backend 请求的 job keys。
3. 调用顺序固定为 `SetCollector`、`CloseExternalEngine`、`ImportEngine`、`GetExternalEngineConflictInfo`、`CleanupEngine`。cleanup 错误被有意忽略；close/import 错误会规范化后返回。
4. 无冲突时保持 subtask meta 不变；有冲突时更新统计并外置 meta，同时保持原先 `range-job-keys` 缺失/空值的 wire 语义。

## 数据与状态

- `TaskMeta` 是各 executor 的任务级快照；`TaskMetaModified` 可替换它。`FrameworkInfo` 持有动态步骤资源、meter 与 checkpoint 回调，`ResourceModified` 复制最新 CPU/memory capacity。
- `ImportStepMeta`、`MergeSortStepMeta`、`WriteIngestStepMeta` 是阶段间 wire state。大字段放在对象存储中，subtask meta 的 `ExternalPath` 是间接引用；`mergeExternalMeta` 的覆盖次序是兼容 planner 拆分格式的关键不变量。
- `SubtaskSummary` 内部计数原子更新：encode collector 的 `Accepted` 计字节、`Processed` 计行；ingest collector 的 `Processed` 同时计字节、data 行和写入 meter；对象存储请求由 RAII recorder 计数。
- `EncodeSortStepExecutor::parquet_estimated` 保证每个 executor 只估算一次 reader 并发；`concurrency` 至少为 1。`writerMemBudgetRatio` 为 0.5，实际 writer 内存拆分由 `getWriterMemorySizeLimit` 完成。
- `ImportNodeTaskExecutor::closed` 是一次性关闭门闩，防止显式 `Close` 与 `Drop` 重复注销 metrics/关闭 base executor。

## 依赖与调用关系

上游与接线证据：

- `lib.rs` 公开本模块；`write_ingest_backend.rs::RegisterConcreteWriteIngestExecutor` 调用 `RegisterImportExecutor`。
- RustCodeGraph 将该文件列为被 `scheduler.rs`、DDL backfilling merge-sort、ingestor global-sort merge 等文件引用；本任务的生产注册边以 `write_ingest_backend.rs` 的直接源码引用为准。
- `encode_and_sort_operator.rs` 直接使用 `writerMemBudgetRatio`、`getOnDupForConflictedKV` 和 `getOnDupForIndex`。

主要下游：

- 框架：`astersql-dxf-framework-taskexecutor`（node API）、`...-taskexecutor-execute`（步骤 API）、framework proto/metering。
- 导入与排序：`astersql-executor-importer::TableImporter`、`RunEncodeSortChunks`、`astersql-ingestor-globalsort`、`astersql-ingestor-engineapi`。
- I/O：`astersql-objstore-storeapi::StorageRef`；`MergeStoreAdapter` 与 backend 的 `BindObjectStore` 使同一子任务 store 贯穿 merge/ingest。
- 表与错误：`astersql-table::BuildTableFromMeta`、`astersql-lightning-common`、`astersql-errors`、`astersql-util-dbterror-exeerrors`。

`Cargo.toml` 声明 crate 名为 `astersql-dxf-importinto`、库入口为 `lib.rs`，`nextgen` feature 只透传到 kernel type；本文件直接使用的 framework、importer、global-sort、Lightning、object-store、KV、table、serde/base64 等均为显式依赖，没有本地 `[patch]` 或 vendor 接线。

## 错误处理与边界

- 所有构造入口先反序列化 `TaskMeta`；步骤不匹配或未知步骤返回包含 task/step 的错误。缺失表信息、表工厂、framework resource 或 importer 都是显式错误，不静默降级。
- 取消在 Init/Run 开头检查，并通过 watcher 传递到 worker pool/旧 execute context。watcher 以原子 `done` 终止并在返回前 join，避免遗留轮询线程。
- 专用对象存储使用 `Drop` guard 关闭；共享 runtime store 不由步骤关闭。`SubtaskRequestRecorder` 只在能取得前后快照时计数。
- local engine 任一 open、排序、close/import 失败都会执行 `CleanupAllLocalEngines`；这是允许相同 subtask 重试的必要条件。`Arc::try_unwrap` 失败表示 engine 仍被共享，会中止 import。
- merge 的 JSON 类型、对象存储 I/O、KV group/index 解析错误直接传播。`Mutex::lock().unwrap()` 假定内部回调未 panic；若已 poison 会 panic，这是当前实现边界。
- write-ingest 的 `CleanupEngine` 是 best-effort，错误被忽略；backend `Close` 仅在 executor cleanup 调用。duplicate-key 错误跨三个错误表示统一转为用户可识别的 executor 8167。
- `parquet_reader_concurrency` 在空 chunks、非 Parquet、估算失败或峰值不正时保留 CPU 并发；有效估算下至少返回 1，并且不会超过 CPU。

## 并发与资源生命周期

- Node `StepExecutor` 要求可跨线程调用，而实际 import executor 含有线程绑定状态；两个 node adapter 用单生产者/消费者 `mpsc` 将所有可变操作串行化到专属 worker。`Cleanup` 发送命令后 join；`Drop` 发送 `Stop` 并 join，避免 worker 泄漏。
- `RunWithNodeCancellation` 和 encode worker-pool watcher 都使用 acquire/release 原子门闩及 10ms 轮询。前者桥接 node context，后者桥接 encode pool；它们均在主工作完成后 join。
- local engines 以 `Arc` 传入并行 encode workers，成功后必须没有其他引用才能 close/import。失败路径清空 importer 记录的所有本地 engine。
- merge writer 回调可并发触发，因此通过 `Arc<Mutex<SortedKVMeta>>` 汇总；summary 与 metrics 使用原子计数。`ImportNodeTaskExecutor` 的 metrics 生命周期覆盖整个 base executor，且以 `AtomicBool` 保证一次关闭。
- 动态资源修改只影响后续读取的 framework resource；encode 的 Parquet 并发一旦由首个子任务估算，不会因后续 subtask 或资源变化重新估算，这是明确的 one-shot 语义。

## 与 Go 版本的对应关系

Go 对照文件是 `task_executor.go`，核心行为保持一致：

- Go `importExecutor.GetStepExecutor` 解码 meta 后按七个步骤分派，并使用 `TaskRuntime.Store()`；Rust `GetImportStepExecutor` 用 `ImportStepHost::TaskStore` 明确保持“任务已绑定 store，不额外按 keyspace 查询”。Go/Rust 测试都覆盖未知步骤和非法 meta。
- Go 的 `importStepExecutor` 同时承担 import/encode-sort；Rust 的生产 node 路径拆成 `EncodeSortStepExecutor` 并把其他阶段交给 host。拆分没有改变 writer 预算、Parquet 最大文件估算、local engine 清理或 summary 语义。
- Go `mergeSortStepExecutor` 和 `writeAndIngestStepExecutor` 对应 Rust 同名语义类型；Rust 额外显式抽象 `MergeStoreAdapter` 与 `WriteIngestBackend`，以隔离对象存储和物理 SST 导入。
- `getOnDupFor*` 与 Go 的映射一致：data/unique index 在 capture 下 Record，error 模式一律 Error，non-unique capture 为 Remove；未知索引和非法 group 报错。
- `normalizeSubtaskErr` 与 Go 一样只转换 duplicate-key；Rust 需额外遍历不同 Rust 错误包装形式。
- Go `ingestCollector.Processed` 只让 data group 计行，并上报写入字节；Rust保持相同行为，但负数字节上报时钳为 0。
- Go `NewImportExecutor` 创建/注销 metrics；Rust由 `RegisterImportExecutor` 注册闭包创建，`ImportNodeTaskExecutor::{Close,Drop}` 注销。

当前差异应被视为接线形态而非删减：Go 的 post-process/collect/conflict 构造位于同一具体 executor 中，Rust通过 `ImportStepHost` 交给相应模块；Rust还保留 `RegisterImportEncodeExecutor`、`RegisterImportConflictExecutor` 作为可组合入口，统一生产路径使用 `RegisterImportExecutor`。

## 扩展指南

- 新增 IMPORT INTO 阶段时，应同时修改 `GetImportStepExecutor` 的匹配、`ImportStepHost`（若由宿主构造）、node/proto step 转换及 Go 对照分派；在独立的 `task_executor_test.rs` 增加合法/未知步骤、store 绑定和 meta 更新测试，不要把测试写进生产文件。
- 修改 encode 流程优先落在 `EncodeSortStepExecutor::run` 或 `RunEncodeSortChunks`。涉及 local engine 时必须覆盖 open 部分失败、worker 失败、close/import 失败和同 ID 重试，并维持 cleanup 顺序。
- 修改 meta 拆分格式时同步检查 `mergeExternalMeta`、两个 read helper、planner 产物和 Go JSON tag。尤其不可把“字段缺失”与“空数组/null”无条件合并；write-ingest 的 job keys 回退依赖这一差异。
- 修改 duplicate policy 时同步 `getOnDupForConflictedKV`、`getOnDupForIndex`、merge operator 数值映射和 write-ingest backend；兼容风险是唯一索引冲突漏记或非唯一索引被错误拒绝。
- 修改并发/内存时同时检查 `getWriterMemorySizeLimit`、块对齐阈值、reader 30% 预算及动态 resource 行为。性能风险包括过小块导致 I/O 放大、过大并发导致 Parquet 峰值超限。
- 修改 node adapter 时维持单线程所有权、cleanup/Drop 均可终止 worker、更新后的 subtask meta 必须回传；建议扩展 `task_executor_test.rs` 和 `task_executor_testkit_test.rs` 的取消、summary JSON、资源与重试场景。
- 具体 backend 的扩展应实现 `WriteIngestBackend` 并在 `write_ingest_backend.rs` 接线；保持 close → import → conflict → cleanup 的次序，并明确 cleanup 是继续 best-effort 还是改为强错误（行为变更需与 Go 对齐）。

## 验证依据

- 目标源码：`pkg/dxf/importinto/task_executor.rs`（2242 行）；RustCodeGraph `node --file` 核对完整分段源码，`query GetImportStepExecutor` 定位入口，`callees GetImportStepExecutor` 确认三个本地构造函数及 `ImportStepHost` 边界。图索引状态为 11467 个文件、7032 个 Rust 文件；`callers GetImportStepExecutor` 未解析到动态/同文件调用，因此以 `EncodeSortNodeStepExecutor::new_import` 的直接源码调用补证。
- crate 与接线：`pkg/dxf/importinto/Cargo.toml`、`pkg/dxf/importinto/lib.rs`、`pkg/dxf/importinto/write_ingest_backend.rs`、`pkg/dxf/importinto/encode_and_sort_operator.rs`。
- Go 对照：`pkg/dxf/importinto/task_executor.go`；重点核对 `importStepExecutor`、`mergeSortStepExecutor`、`writeAndIngestStepExecutor`、`importExecutor.GetStepExecutor`、`getOnDupFor*`、`normalizeSubtaskErr`、`ingestCollector`。
- Rust 独立测试：`pkg/dxf/importinto/task_executor_test.rs` 覆盖零预算块大小、错误规范化、绑定 store 分派、merge 外置 meta、write-ingest 顺序/冲突改写、encode 外置 meta、duplicate policy、Parquet 并发及 planner external meta；`pkg/dxf/importinto/task_executor_testkit_test.rs` 覆盖真实 local engine 失败后清理和重试。
- Go 测试：`pkg/dxf/importinto/task_executor_test.go` 覆盖七步分派、任务 store、duplicate policy 与错误规范化；`task_executor_testkit_test.go` 提供 post-process 和 local-engine 重试语义对照。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求本文恰含上述 11 个固定二级标题；文件链接、符号名及测试路径已逐项人工核对。
