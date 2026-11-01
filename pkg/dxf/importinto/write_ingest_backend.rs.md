# `pkg/dxf/importinto/write_ingest_backend.rs`

[查看对应 Rust 源文件](./write_ingest_backend.rs)

## 文件定位

文件顶部注释将该边界概括为：宿主提供 Region 发现和 TiKV RPC，本地 Rust 代码负责数据加载、重试阶段、进度与引擎生命周期。

## 核心职责

1. 用 `ObjectStore` 将 AsterSQL 对象存储 `StorageRef` 适配为 global-sort 所需的 `global::Storage`，并保证流式 reader 在离开作用域时执行 `Close`。
2. 用 `GlobalSortWriteIngestBackend` 管理每个 subtask ID 对应的 external engine，将排序文件分解为 Region jobs，并把 write/ingest/Region 重扫接入 ingestctrl 的并发和重试管线。
3. 保存物理 TiKV write 的进度，同时在导入完成后校验管线统计、引擎逻辑导入统计和已加载 KV 数相互一致。
4. 用 `GlobalSortImportHost` 只替换 `NewWriteIngestBackend`，其余 step constructor 和 task store 透传给原始 `ImportStepHost`；再由 `RegisterGlobalSortImportExecutor` 把组合后的 host 注册到统一 IMPORT INTO 执行器。

## 主要符号

## 执行流程

## 数据与状态

- `store: Mutex<StorageRef>` 是可替换的子任务 object-store handle。`RunSubtask` 在记录对象存储请求的场景下会通过 `BindObjectStore` 换成当前 handle；创建 engine 时 clone handle，因而后续重绑定不会改写已存在 engine 的 storage adapter。
- `engines: Mutex<HashMap<i64, Arc<ExternalEngineAdapter>>>` 是 subtask ID 到外部引擎的唯一性表。重复 ID 被拒绝；未找到 ID 时 import 报错，而 conflict query 返回默认空信息，cleanup 为幂等成功。
- `collector: Mutex<Option<Arc<dyn Collector>>>` 保存当前 subtask 的进度收集器。`ImportEngine` 开始前必须已设置，否则返回 `ingest collector is not set`。
- `token: CancellationToken` 为后端生命周期内所有 scan/write/ingest 和 pipeline worker 共享的取消根。
- `concurrency` 来自 task runtime slots，构造时下限为 1；同时传给 external engine 和 import pipeline。
- engine 内部分开“已加载 KV 数”、“逻辑导入统计”与 transport 返回的“物理 write 进度”。重试 Ingest 时可能不重写；某些失败分类会重写，此时 collector 按真实物理写入重复累加，但 engine 逻辑统计仍只计入一次数据。

## 依赖与调用关系

上游主链为：

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

373679dd4df08d656e3a95ff0cc0cd245152a9ff 移除 engines 注册表。NewExternalEngine 用完整 globalsort 配置创建调用者持有的 NativeWriteIngestEngine，Source 返回相同底层 adapter。IngestExternalEngine 复用真实 region-job 管线；导入统计与真实已加载条数校验仍保留。CurrentVersion 使用 GlobalSortImportHost 传入的任务 KV store，只有旧 TS 元信息调用。Close 取消管线并关闭 transport，不关闭调用者引擎；句柄的 CloseShared 支持重复关闭。

## 扩展指南

性能风险主要在 Region scan 数量、worker concurrency、memory capacity 和重试导致的物理重写；兼容风险主要在 Go 记录格式、旧 meta 的 job-key fallback、duplicate-key 语义和 external meta 写回。
