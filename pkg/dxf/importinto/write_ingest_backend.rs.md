# `pkg/dxf/importinto/write_ingest_backend.rs`

[查看对应 Rust 源文件](./write_ingest_backend.rs)

## 文件定位

本文件属于 `astersql-dxf-importinto` crate，由 `pkg/dxf/importinto/lib.rs` 以 `pub mod write_ingest_backend` 纳入模块树。它位于 IMPORT INTO 全局排序的 WriteAndIngest 阶段，把 `task_executor.rs` 定义的 `WriteIngestBackend`/`ImportStepHost` 调度边界连接到 Rust 版 global-sort 外部引擎和 ingestctrl 的 Region 导入管线。

这个文件不负责解析子任务 meta、选择 KV group 的重复键策略，也不直接实现 TiKV RPC。前者由 `WriteAndIngestStepExecutor::RunSubtask` 处理，后者由宿主注入的 `RegionImportTransport` 实现。文件顶部注释将该边界概括为：宿主提供 Region 发现和 TiKV RPC，本地 Rust 代码负责数据加载、重试阶段、进度与引擎生命周期。

crate 边界由 `pkg/dxf/importinto/Cargo.toml` 确认：库入口是 `lib.rs`，本文件直接使用 `astersql-ingestor-engineapi`、`astersql-ingestor-globalsort`、`astersql-ingestor-ingestctrl`、`astersql-objstore`/`storeapi` 和 DXF task-executor 执行接口。

## 核心职责

1. 用 `ObjectStore` 将 AsterSQL 对象存储 `StorageRef` 适配为 global-sort 所需的 `global::Storage`，并保证流式 reader 在离开作用域时执行 `Close`。
2. 用 `GlobalSortWriteIngestBackend` 管理每个 subtask ID 对应的 external engine，将排序文件分解为 Region jobs，并把 write/ingest/Region 重扫接入 ingestctrl 的并发和重试管线。
3. 保存物理 TiKV write 的进度，同时在导入完成后校验管线统计、引擎逻辑导入统计和已加载 KV 数相互一致。
4. 用 `GlobalSortImportHost` 只替换 `NewWriteIngestBackend`，其余 step constructor 和 task store 透传给原始 `ImportStepHost`；再由 `RegisterGlobalSortImportExecutor` 把组合后的 host 注册到统一 IMPORT INTO 执行器。

## 主要符号

- `RegionImportTransport: Send + Sync`：宿主 RPC 边界。`Scan(token, range)` 必须返回覆盖 key range 的 Region；`Write(token, job, data)` 返回实际完成的物理写入字节数和 KV 数；`Ingest` 完成 SST ingest；`Close` 关闭传输层资源。注释特别要求，即使后续 Ingest 失败，`Write` 统计也要表示已经完成的写入。
- `ClosingObjectReader`：包装 object-store reader，转发 `std::io::Read::read`，在 `Drop` 中尽力调用 `Close`。
- `open_object_stream(store, path, offset)`：打开对象、seek 到指定偏移并返回自动关闭的 `Read`。它还被 `task_executor.rs::MergeStoreAdapter` 复用，因而同时服务 merge-sort 的有界流式读取。
- `ObjectStore(StorageRef)`：实现 `global::Storage`。`open`/`open_at`、`file_size`、`read`、`write`、`delete_files`、`list_prefix` 分别映射到 `Open`/`Seek`、`ReadFile`、`WriteFile`、`DeleteFiles`、`WalkDir`；`record_format()` 固定返回 `GoBigEndian64`，用于读取 Go 生成的排序记录。
- `GlobalSortWriteIngestBackend`：核心后端。它持有可重绑定的 object store、RPC transport、按 subtask ID 索引的 engine map、进度 collector、共享 cancellation token 和至少为 1 的并发度。
- `GlobalSortWriteIngestBackend::new`：初始化后端，并用 `concurrency.max(1)` 消除零 worker 配置。
- `WriteIngestBackend` 实现：`BindObjectStore`、`SetCollector`、`CloseExternalEngine`、`ImportEngine`、`GetExternalEngineConflictInfo`、`CleanupEngine`、`Close` 构成 executor 可见的完整生命周期。
- `GlobalSortImportHost`：包装原 `ImportStepHost`，保留原 store 和其他 step 构造器，仅在 WriteAndIngest 阶段创建 native global-sort 后端。
- `RegisterGlobalSortImportExecutor`：公开注册入口，将 runtime 的 object store、host 和 transport 组合后，调用 `task_executor::RegisterImportExecutor`。

## 执行流程

1. 应用宿主先调用 `RegisterGlobalSortImportExecutor`。真实 TiKV 回归 harness `tests/realtikvtest/importintotest4/recorded_summary_harness.rs` 展示了这个接线：它创建 runtime 和 `RegionRpc`，注册后驱动 MergeSort/WriteAndIngest 等阶段。
2. `task_executor.rs::GetImportStepExecutor` 在 `ImportStepWriteAndIngest` 分支调用 `host.NewWriteIngestBackend`，再把后端传给 `NewWriteAndIngestStepExecutor`。`GlobalSortImportHost` 以 task runtime slots 作为后端并发度。
3. `WriteAndIngestStepExecutor::RunSubtask` 读取 meta 和资源配额，确定 duplicate-key 策略，组装 `WriteIngestRequest`，设置 `ingestCollector`，然后依次调用 `CloseExternalEngine` 和 `ImportEngine`。
4. `CloseExternalEngine` 将 engineapi 的四种 duplicate-key 模式映射为 global-sort 模式，再把 data/stat 文件、key 边界、job/split keys、TS、总量、内存容量和前缀传入 `global::engine::NewExternalEngine`。只有在构造成功且 ID 未存在时才将 adapter 插入 map。
5. `ImportEngine` 取出 engine 的 `Arc` 副本，创建 job generator。对每个 sorted range，generator 先 `Scan`，再用 `newRegionJobs` 按 Region 和 split 配额切分 job，把 engine data 的 TS 写入每个 job。
6. worker factory 为每个 worker 创建 `NewRegionJobBaseWorker`。write callback 获取 job 的 ingest data，调用 transport `Write`，非 empty job 才将实际 write 字节数和 KV 数传给 collector；ingest callback 调用 transport `Ingest`；pre-run 仅检查取消；regenerate callback 重新 `Scan` 并保留原 job 的 TS。
7. `local::import_pipeline::do_import` 加载 external-engine batches，生成 jobs，在有界 worker pool 中执行，并调度重试和取消。返回后，`ImportEngine` 必须同时满足 `(bytes, count) == engine.ImportedStatistics()` 且 `count == engine.GetTotalLoadedKVsCount()`。
8. executor 将 collector 进度合并到 subtask summary，读取冲突信息，尽力清理 engine；只有存在记录冲突时才重写外部 subtask meta。整个 step cleanup 最终调用 backend `Close`。

## 数据与状态

- `store: Mutex<StorageRef>` 是可替换的子任务 object-store handle。`RunSubtask` 在记录对象存储请求的场景下会通过 `BindObjectStore` 换成当前 handle；创建 engine 时 clone handle，因而后续重绑定不会改写已存在 engine 的 storage adapter。
- `engines: Mutex<HashMap<i64, Arc<ExternalEngineAdapter>>>` 是 subtask ID 到外部引擎的唯一性表。重复 ID 被拒绝；未找到 ID 时 import 报错，而 conflict query 返回默认空信息，cleanup 为幂等成功。
- `collector: Mutex<Option<Arc<dyn Collector>>>` 保存当前 subtask 的进度收集器。`ImportEngine` 开始前必须已设置，否则返回 `ingest collector is not set`。
- `token: CancellationToken` 为后端生命周期内所有 scan/write/ingest 和 pipeline worker 共享的取消根。
- `concurrency` 来自 task runtime slots，构造时下限为 1；同时传给 external engine 和 import pipeline。
- engine 内部分开“已加载 KV 数”、“逻辑导入统计”与 transport 返回的“物理 write 进度”。重试 Ingest 时可能不重写；某些失败分类会重写，此时 collector 按真实物理写入重复累加，但 engine 逻辑统计仍只计入一次数据。

## 依赖与调用关系

上游主链为：

`RegisterGlobalSortImportExecutor` → `task_executor::RegisterImportExecutor` → `GetImportStepExecutor` → `GlobalSortImportHost::NewWriteIngestBackend` → `NewWriteAndIngestStepExecutor` → `WriteAndIngestStepExecutor::RunSubtask` → `CloseExternalEngine`/`ImportEngine`。

RustCodeGraph 对目标文件的文件节点报告直接使用者包含 `pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/task_executor_test.rs`、`pkg/dxf/importinto/clean_up_test.rs` 和 `tests/realtikvtest/importintotest4/recorded_summary_harness.rs`。其中生产调用的关键证据是 `task_executor.rs` 对 `open_object_stream` 的复用，以及 WriteAndIngest executor 对 `WriteIngestBackend` trait 方法的调用；回归 harness 则提供注册入口的真实接线。

下游主要依赖是：

- `global::engine::NewExternalEngine`/`ExternalEngineAdapter`：读取外部排序文件、管理加载数据和 duplicate-key 信息。Rust 实现会验证 data/stat 文件数相等、worker concurrency 大于零、job keys 非降序。
- `local::region_job::newRegionJobs`：将 sorted range 与 Region 覆盖、split size/key 阈值组合为 `RegionJob`。
- `local::job_worker::NewRegionJobBaseWorker`：组合 write、ingest、pre-run 及 regenerate 回调；默认 write timeout 为 15 分钟。
- `local::import_pipeline::do_import`：拥有 loader、generator、worker pool、result dispatcher、retry queue、cancellation monitor 和最终 join/release 顺序。
- `RegionImportTransport`：反转依赖的 RPC 实现，使本文件不依赖具体 TiKV client。

## 错误处理与边界

- object-store `Open`/`Seek`/`Close`/`ReadFile`/`WriteFile`/`DeleteFiles`/`WalkDir` 错误统一映射为 `global::Error::InvalidData`。`open_object_stream` 的 seek 失败路径会在返错前主动 `Close`；成功路径由 `Drop` 关闭。`file_size` 无论 seek 结果都先取得 close 结果，然后依次检查，避免正常路径泄漏 reader。
- duplicate-key 模式只接受 Ignore/Record/Remove/Error，未知值立即失败，不会创建 engine。`NewExternalEngine` 自身的参数不变式失败也向上传播。
- engine ID 重复、import 时 ID 缺失、collector 未设置、Region scan 无法产生覆盖 job、以及导入统计不一致都是硬错误。
- generator 对每个 range 的空 job 结果报 `region scan returned no covering regions`，防止误将未覆盖数据当成成功。regenerate 回调本身返回可能为空的列表，其后续语义由 ingestctrl 的重试机制处理。
- `CleanupEngine` 先从 map 移除引擎，然后要求 `Arc::try_unwrap`。如果仍有引用，它返回 `external engine {id} is still in use`；由于条目已被移除，调用方不应在还有活跃 import 引用时 cleanup。
- `Close` 是尽力清理：先取消 token，然后遍历当前 ID 并忽略单个 cleanup 错误，最后关闭 transport。因此关闭阶段不向上报告引擎关闭失败。
- `Mutex::lock().unwrap()` 意味着任一持锁代码 panic 造成的 poison 会使后续调用也 panic；当前文件没有恢复 poison 的设计。

## 并发与资源生命周期

`GlobalSortWriteIngestBackend` 以 `Mutex` 保护 store、engine map 和 collector，以 `Arc` 共享 transport、engine 和 collector，所以满足 `WriteIngestBackend: Send + Sync`。实现会在进入耗时导入管线前 clone 出 engine/transport/collector，不长时持有这些状态锁。

`do_import` 使用有界任务通道和 `concurrency.max(1)` 个 worker，并在 scoped threads 中运行 monitor、retry、producer/generator 和 dispatcher。当 parent token 取消或某个组件失败时，它关闭任务通道、通知等待者，释放 worker pool，再 join 所有 scoped threads。job resources 通过 `Arc` 保持一个完整 batch，直到所有派生 jobs 完成。

单个 engine 的预期顺序是：构造并插入 map → import 期间持有 clone 的 `Arc` → import 返回后读冲突信息 → `CleanupEngine` 从 map 取出且唯一持有，再调用 engine `Close`。backend 级别顺序是：所有 subtask 完成后由 step `Cleanup` 调用 `Close` → token 取消 →尽力关闭残留 engines → transport `Close`。

object reader 有两个显式关闭保障：打开后 seek 失败立即关闭，成功返回后由 `ClosingObjectReader::drop` 关闭。`StorageRef` 本身的子任务级关闭则由 `RunSubtask` 中的 `CloseGuard` 管理，只有 factory 为当前子任务新建 handle 时才关闭。

## 与 Go 版本的对应关系

最近的 Go 对照实现不是同名文件，而是 `pkg/dxf/importinto/task_executor.go` 中的 `writeAndIngestStepExecutor`，以及其下游 Lightning/local backend：

- Go `RunSubtask` 创建带记录的 object store、兼容 `RangeJobKeys == nil` 的旧 meta、选择 on-duplicate 策略、设置 collector，然后调用 `localBackend.CloseEngine` 和 `ImportEngine`。Rust `task_executor.rs` 保留这一顺序，但通过 `WriteIngestRequest` 和本文件的 trait 将物理导入拆出。
- Go 把 `sm.RangeJobKeys == nil` 回退到 `RangeSplitKeys`；Rust `readWriteIngestMeta` 额外保留 JSON 字段是否存在的 `has_job_keys`，组装 request 时做同样回退，重写 meta 时也保留旧格式的 null 语义。
- Go `CloseEngine` 的 `ExternalEngineConfig` 传递 storage/files/ranges/split config/TS/memory/on-duplicate/prefix。Rust `CloseExternalEngine` 对应这些字段并构造 Rust global-sort engine。一个明确差异是 Go 版此处把 `TotalKVCount` 设为 `0`，Rust request 传入 `SortedKVMeta.TotalKVCnt`，且 Rust 导入后会用它相关的 loaded count 做一致性检查。
- Go `onFinished` 先读 conflict info、尽力 cleanup，无冲突时不产生 object-store PUT，有冲突时写外部 meta。Rust executor 保留这一语义；本文件为它提供 conflict query 和 cleanup。
- Go executor `Init` 通过 CPU resource 设置 backend worker concurrency；Rust native host 在构造 backend 时使用 `task.GetRuntimeSlots()`，并以 1 为下限。扩展动态资源更新时需特别检查这个差异。
- Go storage 记录格式由 global-sort 原实现产生；Rust adapter 显式返回 `RecordFormat::GoBigEndian64`，表明它要兼容 Go 大端 64 位长度前缀。

## 扩展指南

- 增加或修改 TiKV RPC 行为：优先扩展 `RegionImportTransport` 及宿主实现，保持本文件不耦合具体 client。必须明确 write 成功、ingest 失败时的统计语义，并在独立的 `pkg/dxf/importinto/write_ingest_backend_test.rs` 增加对应重试回归。
- 增加 duplicate-key 模式：同步修改 `CloseExternalEngine` 的穷举映射、engineapi/global-sort 定义及测试，不要让未知值静默降级。
- 修改 Region 分割或重试：主接入点是 `ImportEngine` 中的 generator/regenerate callbacks。必须保留 range 覆盖检查、TS 传递、job resource 引用和 cancellation token；同时核对 `pkg/ingestor/ingestctrl` 的独立 pipeline/region-job 测试。
- 修改进度统计：只在 `!result.empty_job` 时记录 transport 报告的实际物理 write。不得把该统计与 engine 的逻辑 imported statistics 混合，应同步更新 `physical_region_rewrite_increments_progress_without_recounting_logical_import` 测试。
- 修改 engine 生命周期：保持“先停止并发使用，再 cleanup”。若需允许 cleanup 与 import 并发，必须重新设计 `Arc::try_unwrap` 和 map 移除失败后的可恢复性。
- 修改并发度：当前值在 backend 创建时固定。要支持 runtime resource modification，需同时评估 task runtime slots、external engine worker resource 和 `do_import` pool 大小，不能只改其中一处。
- 修改 object-store 读取：优先复用 `open_object_stream`，保持 seek 失败和 Drop 两条关闭路径，并注意它同时被 merge-sort adapter 使用。

性能风险主要在 Region scan 数量、worker concurrency、memory capacity 和重试导致的物理重写；兼容风险主要在 Go 记录格式、旧 meta 的 job-key fallback、duplicate-key 语义和 external meta 写回。

## 验证依据

- RustCodeGraph `status`：本地索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/importinto/write_ingest_backend.rs` 确认目标文件已索引，有 48 个符号。
- RustCodeGraph `node --file pkg/dxf/importinto/write_ingest_backend.rs --offset 1 --limit 500`：读取全部 353 行，并确认文件级 `used by` 关系。
- RustCodeGraph `query`/`node`：查询了 `WriteIngestBackend`、`RegisterGlobalSortImportExecutor`、`NewExternalEngine`、`do_import`、`NewRegionJobBaseWorker` 和 `newRegionJobs`；节点源码确认 external-engine 参数校验、pipeline 的 worker/retry/cancellation/join 结构和 worker 的 15 分钟 write timeout。`callers`/`callees` 对精确符号的 CLI 查询在本次会话内超时且未返回边，因此调用关系另由已索引的源文件节点和 `used by` 证据逐处核对，没有把超时查询当作成功证据。
- 源码与模块边界：`pkg/dxf/importinto/write_ingest_backend.rs`、`pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/lib.rs` 和 `pkg/dxf/importinto/Cargo.toml`。目标包不存在 `doc.go`。
- Go 对照：`pkg/dxf/importinto/task_executor.go` 的 `writeAndIngestStepExecutor::{Init,RunSubtask,onFinished,Cleanup}` 及 local backend 调用顺序。
- 独立 Rust 测试：`pkg/dxf/importinto/write_ingest_backend_test.rs::physical_region_rewrite_increments_progress_without_recounting_logical_import`，覆盖无失败、`KVIngestFailed` 导致物理重写、`ServerIsBusy` 不重写三种情况，并验证 rows、writes、逻辑导入统计、collector 进度和 cleanup。
- 应用接线证据：`tests/realtikvtest/importintotest4/recorded_summary_harness.rs` 创建 `RegionRpc` 并调用 `RegisterGlobalSortImportExecutor`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付仅运行任务文件指定的 11 章结构检查；仓库指令提到的 `.agents/skills/tidb-verify-profile` 在当前检出中不存在，因而无法加载额外 Ready profile 命令。
