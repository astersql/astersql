# `pkg/ingestor/ingestctrl/job_worker.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；`pkg/ingestor/ingestctrl/Cargo.toml` 将 `lib.rs` 设为库入口，`pkg/ingestor/ingestctrl/lib.rs` 以 `pub mod job_worker` 暴露本模块。它位于 Region 任务生成与导入流水线之间：接收已经携带 Region、半开键范围 `[start, end)` 和待写 KV 的 `RegionJob`，驱动“已扫描 → 已写入 → 已导入”状态机，并把需要重扫的任务拆成新的 Region 子任务。

生产接线可见 `pkg/dxf/importinto/write_ingest_backend.rs`：其 Worker factory 用 `NewRegionJobBaseWorker` 注入实际的 Write、Ingest、取消检查和 Region 重扫回调，再交给 `pkg/ingestor/ingestctrl/import_pipeline.rs` 的资源池 Worker 调用 `RegionJobWorker::HandleTask`。本文件还定义块存储导入前的 Store 空间检查和对象存储分批写入边界；当前仓库搜索只发现这两种具体包装在独立测试中直接构造，未发现生产构造点，因此不能把它们描述为已经接入当前 Rust 主链。

文件没有条件编译项。Cargo 中直接支撑本文件的依赖包括 `astersql-metrics`（panic 计数）和 `astersql-resourcemanager-pool-workerpool`（`TaskMayPanic`）；crate 的大量额外依赖受 `cfg(windows)` 约束，但不是本文件源码自身的条件分支。

## 核心职责

1. 定义 Region 导入任务的数据模型：`RegionJobStage`、`RegionInfo`、`TikvWriteResult` 和 `RegionJob`。
2. 通过 `RegionJobBaseWorker` 和四个注入回调，把调度框架与实际写入、ingest、前置检查、重扫生成逻辑解耦。
3. 在 `runJob` 中维护阶段不变量，区分可重试与不可重试错误，并支持一次 Write/Ingest 后从 `remaining_start_key` 继续处理剩余键范围。
4. 在 `process` 中把 `NeedRescan` 任务替换为新任务，同时转移引用计数、资源统计、完成标志和最近的可重试错误。
5. 在 `HandleTask` 边界捕获 panic、记录堆栈和 panic 指标，并释放原任务。
6. 提供 `BlockStoreRegionJobWorker::preRunJob` 的磁盘空间门禁，以及 `ObjectStoreRegionJobWorker::{write, ingest}` 的对象存储客户端适配。

## 主要符号

- `toAtomic(i32) -> AtomicI32`：创建原子 `i32`，对应 Go 的 `atomic.NewInt32` 辅助函数；本文件内没有调用者。
- `RegionJobStage::{RegionScanned, Wrote, Ingested, NeedRescan}`：任务阶段枚举。默认值为 `RegionScanned`。
- `RegionInfo`：保存 Region ID、leader Store ID 和全部 peer Store ID。`leader_store_id == 0` 是“没有 leader”的判据。
- `TikvWriteResult`：保存写入条数、字节数、剩余起始键、空任务标志和供 Ingest 使用的响应字节。
- `RegionJob`：聚合阶段、Region、键范围、数据、时间戳、写入结果、重试错误、分裂阈值，以及共享的引用/资源/完成状态。`convertStageTo`、`ref`、`done`、`ref_count` 管理阶段和生命周期。
- `RegionJobWorker`：调度层使用的 `Send` trait；`HandleTask` 返回下一轮要发送的任务列表，`Close` 负责 Worker 收尾。
- `WriteFn`、`IngestFn`、`PreRunFn`、`RegenerateFn`：分别抽象写入、导入、前置门禁和重扫生成；均要求 `Send + Sync`，并共享 `CancellationToken`。
- `NewRegionJobBaseWorker`：构造基础 Worker，默认 `write_timeout` 为 15 分钟；`set_after_run` 与 `set_write_timeout` 提供后续配置点。
- `RegionJobBaseWorker::{process, runJob, writeWithTimeout}`：依次负责输出/重扫编排、状态机、写入耗时判定。
- `StoreSpaceProvider` 与 `BlockStoreRegionJobWorker::preRunJob`：查询 peer Store 可用空间比例；低于 10% 时返回 `DiskQuotaExceeded`。
- `ObjectWriteClient` 与 `ObjectStoreRegionJobWorker::{write, ingest}`：对象存储写入/导入抽象及实现。
- `isRetryableImportTiKVError`：把 `Retryable`、`Timeout` 或错误文本含 `EOF` 的错误归为可重试。
- `TaskMayPanic for RegionJob`：向资源池 panic 恢复接口提供固定标签 `regionJob`。

## 执行流程

`RegionJobBaseWorker::HandleTask` 先克隆任务并在 `catch_unwind` 内调用 `process`。正常路径中，`process` 调用 `runJob`，随后无论 `runJob` 成败都执行可选的 `after_run_job_fn(peer_store_ids)`。若最终阶段为 `RegionScanned`、`Wrote` 或 `Ingested`，它先检查取消，再传播 `runJob` 错误，最后返回原任务；若为 `NeedRescan`，则先调用 `regenerate_jobs_fn`，再传播原错误，并返回新任务。

`runJob` 的状态机为：

1. 先执行 `pre_run_job_fn`；失败立即返回，不进入 Write。
2. 每轮先执行 `token.check()`。处于 `RegionScanned` 且 leader Store ID 为 0 时，记录可重试文本并转为 `NeedRescan`。
3. 有 leader 时调用 `writeWithTimeout`。空写结果直接进入 `Ingested`；非空结果保存到 `write_result` 并进入 `Wrote`。
4. Write 可重试错误会保存文本和原始 `Error`。含 `RequestTooNew` 时留在 `RegionScanned` 以便重写，其他可重试错误进入 `NeedRescan`；不可重试错误原样返回。
5. `Wrote` 阶段调用注入的 `ingest_fn`。成功进入 `Ingested`；可重试错误由 `region_job.rs::getNextStageOnIngestError` 选择 `RegionScanned`、`Wrote` 或 `NeedRescan`；不可重试错误原样返回。
6. `Ingested` 且没有 `remaining_start_key` 时完成。若有剩余起始键，则更新 `key_range.start`，回到 `RegionScanned`，继续下一轮 Write/Ingest。

重扫时，`process` 让新增任务共享原任务的 `references`、`resources` 和最近错误；第一个任务还共享 `completed`。若生成 `n > 1` 个任务，原任务先执行 `n-1` 次 `ref`，使一个逻辑任务拆分后仍保持正确的引用总量。

## 数据与状态

`RegionJob` 是状态载体。阶段、范围、写入结果和错误随每次处理改变；`last_retryable_error` 便于日志/诊断，`last_retryable_cause` 保留结构化错误身份，供流水线重试耗尽时返回原始原因。`retry_count` 由 `import_pipeline.rs` 的 dispatcher/retry 逻辑消费，本文件只携带它。

资源统计由可选的 `JobResources` 共享对象承担。`convertStageTo(Ingested)` 仅在第一次进入该阶段且存在 `write_result` 时调用 `resources.finish(total_bytes, count)`；`ref` 同时增加原子引用和资源引用；`done` 在有资源对象时通过共享 `completed` 防止重复完成，然后减少引用并通知资源对象。无资源对象时，调用方仍须保证 `done` 次数与引用数匹配，否则无符号原子减法可能下溢。

对象存储写入先按 `[start, end)` 过滤 `job.data`；空 `end` 表示无上界。数据按键值总字节累计，达到 `write_batch_size.max(1)` 后结束当前批次；返回的 `count` 和 `total_bytes` 根据最终批次重新汇总。`timestamp == 0` 在过滤前即报错，因此即便范围内没有数据也要求有效时间戳。

## 依赖与调用关系

已验证的主链为：

`pkg/dxf/importinto/write_ingest_backend.rs` Worker factory
→ `NewRegionJobBaseWorker`
→ `pkg/ingestor/ingestctrl/import_pipeline.rs::Worker::HandleTask`
→ `RegionJobWorker::HandleTask`
→ `RegionJobBaseWorker::process`
→ `runJob`
→ 注入的 `write_fn` / `ingest_fn` / `pre_run_job_fn`
→ 必要时 `region_job.rs::getNextStageOnIngestError` 或 `regenerate_jobs_fn`。

`import_pipeline.rs` 用 `OwnedJob::Drop` 调用 `job.done()`，并用 `StoreLoadGuard::Drop` 释放 peer Store 负载；因此本文件返回的任务列表直接决定后续资源所有权。RustCodeGraph 确认 `runJob` 的直接下游包括 `CancellationToken::check`、`convertStageTo`、`writeWithTimeout`、`isRetryableImportTiKVError` 和 `getNextStageOnIngestError`。动态 trait 调用和部分构造边没有被图完整解析，生产上游由源码搜索和调用点读取补证。

`ObjectStoreRegionJobWorker` 下游是 `ObjectWriteClient::{Write, Ingest}`；`BlockStoreRegionJobWorker` 下游是 `StoreSpaceProvider::available_ratio`。两者持有一个 `base` 字段，但本文件没有为它们实现 `RegionJobWorker`，也没有把其方法自动安装为 base 回调；调用方若要接线，必须显式构造闭包或增加完整实现。

## 错误处理与边界

取消通过每轮状态机、普通输出前以及注入回调中的 `CancellationToken` 协作传播。可重试错误转为阶段变化并返回 `Ok`，由外层重试；不可重试错误保持当前阶段并向上传播。`NeedRescan` 路径先执行重扫回调，重扫失败会直接覆盖返回；重扫成功后才传播先前的 `runJob` 错误，不过按当前状态机，转入 `NeedRescan` 的可重试分支返回的是 `Ok(())`。

`writeWithTimeout` 是事后耗时检查：它同步执行完整 `write_fn`，返回后才比较 elapsed。超过阈值时把原回调结果替换为 `Error::Retryable("write to TiKV is too slow")`；它不会中断正在执行的写入，也不创建带 deadline 的子 token。`HandleTask` 捕获 unwind 后记录错误和强制回溯、增加 `PanicCounter`、调用 `job.done()`，再返回 `InvalidData`。

磁盘门禁仅在 `check_tikv_space` 为真时遍历 peer。比例严格小于 `0.10` 才失败；查询错误被忽略并继续，这一点由 Rust 独立测试锁定。对象写入保持半开范围语义，批次阈值为零时按 1 字节处理；`ingest` 在缺失 `write_result` 时向客户端传空响应，由客户端决定是否接受。

## 并发与资源生命周期

所有可注入函数和客户端/provider trait object 都要求 `Send + Sync`，Worker trait 要求 `Send`，适合被资源池跨线程持有。任务引用计数使用 `AtomicUsize`：增减为 `AcqRel`，读取为 `Acquire`；完成去重使用 `AtomicBool::swap(AcqRel)`。重扫子任务共享同一 `Arc` 状态，而普通 `RegionJob::clone` 也会共享这些原子对象。

本文件不创建线程、异步任务或通道。并发调度、发送结果、重试队列和最终 `done` 由 `import_pipeline.rs` 管理。`HandleTask` 内部和流水线 Worker 外层均有 panic 捕获；内层已把 panic 转成普通错误时，外层不会再次恢复。`Close` 对基础 Worker是空操作；对象客户端的真实连接/流生命周期留给 `ObjectWriteClient` 实现。

`after_run_job_fn` 在 `runJob` 后同步调用，当前签名不能返回错误，且 panic 会被 `HandleTask` 捕获。它主要适合释放 peer 负载之类的配对动作；若扩展为阻塞工作，会直接占用 Worker 执行时间。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestctrl/job_worker.go`，测试对照是同目录 `job_worker_test.go`。Rust 保留了 Go 的四阶段模型、无 leader 重扫、`RequestTooNew` 原阶段重试、空任务直达 `Ingested`、部分写入循环、15 分钟阈值、EOF 可重试、低于 10% 磁盘门禁以及对象写入/ingest 两段式结构。

当前 Rust 并非 Go 的完全等价实现：

- Go `writeWithTimeout` 创建可取消的 deadline context，并只在 deadline 原因匹配时转换为 `ErrWriteTooSlow`；Rust 是同步调用后的耗时比较，不能提前取消慢 Write。
- Go 的错误分类基于 `errors.Cause` 和 `common.IsRetryableError`；Rust 只识别两个枚举变体或错误文本中的 `EOF`。
- Go 的 ingest 错误可携带 `NewRegion` 并直接替换 `job.region`；Rust `getNextStageOnIngestError` 只按枚举/文本选下一阶段，不返回新 Region。
- Go 路径包含 retry 指标、随机错误注入、详细日志和 failpoint；Rust 除 panic 指标外没有这些观测/注入行为。
- Go `process` 通过 channel 和 WaitGroup 转交/完成任务；Rust 返回拥有型 `Vec<RegionJob>`，由 `import_pipeline.rs` 的 `OwnedJob` 和资源池接管。
- Go 对象 Worker从 `IngestData` 迭代、管理 buffer、流式 Write/Recv/Close 并通知 collector；Rust 从内存 `Vec<KvPair>` 过滤和分批，把完整 batches 一次交给抽象客户端。
- Go block Worker从 PD HTTP 获取 Store 信息并调用 `checkDiskAvail`；Rust 使用注入的比例 provider，查询失败同样选择忽略。

Rust 测试 `test_region_job_base_worker`、`test_is_retryable_ti_kv_write_error`、`block_store_worker_ignores_store_lookup_failures_like_go`、`test_cloud_region_job_worker` 覆盖主要阶段和适配器边界，但 Go 测试注释中保留的若干流式客户端失败、failpoint、channel/WaitGroup 场景并未全部转化为可执行 Rust 断言。

## 扩展指南

- 修改阶段转换时，优先调整 `runJob` 和 `region_job.rs::getNextStageOnIngestError`，并同步独立测试 `pkg/ingestor/ingestctrl/job_worker_test.rs` 与 `region_job_test.rs`；核对 Go 文件及 Go 测试，避免错误分类漂移。
- 新增可重试错误类型时，不要只匹配显示文本；评估扩展 `Error` 的结构化变体、`isRetryableImportTiKVError` 和 dispatcher 的 `last_retryable_cause` 传播。
- 若要实现真正超时取消，需要扩展 `CancellationToken` 或 Write 回调协议，而不只是调整 `set_write_timeout`；同时验证慢调用是否安全终止、是否可能重复写入。
- 修改重扫拆分时，必须保持 `references/resources/completed` 的所有权不变量，并覆盖零个、一个、多个再生任务以及取消/错误路径。
- 接入 `BlockStoreRegionJobWorker` 或 `ObjectStoreRegionJobWorker` 到生产主链前，需要实现或组合完整的 `RegionJobWorker`，明确 `base` 与方法闭包的绑定以及 `Close` 语义，不能仅构造当前结构体就假定包装生效。
- 对象分批策略变化需要验证范围端点、零阈值、单 KV 大于阈值、尾批、计数/字节统计、客户端 Write 与 Ingest 失败；测试继续放在 `job_worker_test.rs`，不要内嵌到生产文件。

主要正确性风险是阶段选择、重扫资源计数和完成去重；兼容风险是 Rust 错误文本分类与 Go 结构化错误语义不同；性能风险来自 `RegionJob` 克隆、对象路径先复制范围数据再构建批次，以及同步慢 Write 长时间占用 Worker。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `explore "pkg/ingestor/ingestctrl/job_worker.rs RegionJobBaseWorker ObjectStoreRegionJobWorker BlockStoreRegionJobWorker"`：核对目标文件、Go 对照、测试符号和调用影响。
- RustCodeGraph `query`：核对 `RegionJobBaseWorker`、`ObjectStoreRegionJobWorker`、`BlockStoreRegionJobWorker`、`isRetryableImportTiKVError`；`node --file ... --offset 175 --limit 340` 核对目标文件后半部完整实现。
- RustCodeGraph `callees runJob` 与 `node getNextStageOnIngestError`：确认 Rust 状态机的直接下游和 ingest 错误阶段映射。图对动态 trait callers 和若干结构体使用没有返回精确边，未把缺失边当成“不存在调用”。
- crate/模块边界：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/lib.rs`；目标包没有 `doc.go`。
- 生产调用证据：`pkg/dxf/importinto/write_ingest_backend.rs`、`pkg/ingestor/ingestctrl/import_pipeline.rs`。
- Go 对照：`pkg/ingestor/ingestctrl/job_worker.go`、`pkg/ingestor/ingestctrl/region_job.go`。
- 测试证据：`pkg/ingestor/ingestctrl/job_worker_test.rs`、`pkg/ingestor/ingestctrl/job_worker_test.go`，以及相关 `region_job_test.rs`。
- 人工复核结论：本文件存在是为了把 Region 状态机与具体 Write/Ingest/重扫实现解耦；运行时由导入流水线交付任务并接管返回任务的资源生命周期；安全扩展必须同步阶段、错误分类、引用/资源不变量和独立 parity 测试。
