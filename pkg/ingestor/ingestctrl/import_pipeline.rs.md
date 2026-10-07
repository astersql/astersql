# `pkg/ingestor/ingestctrl/import_pipeline.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate（见同目录 `Cargo.toml`），由 `lib.rs` 以公开模块 `import_pipeline` 暴露。它是 ingest 控制面的 Region Job 流水线实现：把 `engineapi::Engine::LoadIngestData` 产生的批次转换为 `RegionJob`，交给可调并发的 worker pool，分发成功或可重试结果，并在返回前完成 worker、任务引用与底层 ingest data 的清理。

真实入口有三类：`local.rs` 的本地 `Engine` 导入使用 `LocalEngineSource`/`ClientWorker`，同文件的外部引擎导入使用 `ExternalEngineSource`/`ExternalClientWorker`；`pkg/dxf/importinto/write_ingest_backend.rs::ImportEngine` 则直接组装 `JobGenerator` 与分阶段 `RegionJobWorker` 后调用 `do_import`。因此它不是独立服务入口，而是 Local Backend 和分布式 Import Into 写入后端之间的通用执行内核。

## 核心职责

- `do_import` 统一管理加载、生成、平衡、执行、结果分发、延迟重试、取消传播和最终汇总，成功时返回 `(导入字节数, 导入 KV 数)`。
- 维护“每次 `RegionJob::ref` 最终恰有一次 `RegionJob::done`”的不变量。`JobResources` 将该任务级不变量扩展到一个 `IngestData` 批次，并保证 `DecRef` 完成后才减少 `Group.pending`。
- 区分“结果通道关闭”与“流水线成功”。只有 worker pool `Release` 完成、未出现 operator/父级取消错误并设置 `succeeded` 后，`dispatch_results` 才能成功退出。
- 对本地引擎启用 `storeBalancer`，对外部引擎禁用无背压的 balancer，并将生成器并发限制为 1，以约束驻留的 ingest data；该差异由 `ImportOptions::local_engine` 控制。
- 将现有引擎、客户端和内存 KV 适配到 `engineapi::Engine`、`IngestData`、`ForwardIter`、`RegionJobWorker` 边界。

## 主要符号

- `do_import(parent, engine, concurrency, generate, factory, options) -> Result<(i64, i64)>`：公开主入口；并发至少按 1 创建 worker，组织所有 scoped threads，返回 `Group` 累积统计或首个错误。
- `Group`：共享 `pending`/`Condvar`、首错槽 `error` 及原子 `bytes`/`count`。`fail` 只保存首错，同时调用 worker context 的 `OnError` 触发全链路取消。
- `JobResources`：持有批次 `IngestData` 和 `Group`；`reference`、`done`、`finish` 分别维护数据引用、未完成任务数和成功统计。
- `OwnedJob(Option<RegionJob>)`：通道所有权护栏。未成功转交的值在 `Drop` 中自动 `job.done()`，覆盖发送与取消竞争。
- `Worker`：把 `RegionJobWorker` 接入通用 worker pool；捕获 worker panic、上报指标/错误，并以 `StoreLoadGuard` 确保 store load 被释放。
- `ImportPoolTuner`/`RunningPool`：向外部引擎暴露正在运行的真实 pool；`Tune` 拒绝 0 或超出 `i32` 的并发值，并等待退休 worker 完成回调和 `Close`。
- `ImportOptions`：选择本地引擎模式，并提供 pool 启动、release 前、等待 outcome 前、接收结果前的显式 hook；这些 hook 对应 Go failpoint 测试边界而非全局状态。
- `dispatch_results`：消费 worker 结果；完成任务、按指数退避送入 `regionJobRetryer`，或在超过 `MAX_RETRIES = 30` 时返回保留类型的最后错误。
- `ClientWorker`/`ExternalClientWorker`：分别调用 `WriteAndIngest` 和 `WriteAndIngestData`；后者从 `job.ingest_data()` 取得原始批次并在 `Close` 时关闭 client。
- `ExternalEngineSource`/`LocalEngineSource`：实现 `engineapi::Engine`。前者转发 `ExternalEngine`；后者把本地快照、时间戳和 ranges 包成单个 `DataAndRanges`。
- `LocalData`/`DataIter`：本地快照的 `IngestData` 与前向迭代器适配；使用半开区间 `[lower, upper)` 过滤 KV，并受 `engineapi::Context` 取消控制。
- `RegionJob::ingest_data`：为物理写入尝试取得共享 ingest data；缺少资源绑定时返回 `InvalidData`。

## 执行流程

1. `do_import` 先检查父取消，建立 `Group`、worker/operator context、零缓冲任务通道、结果成功标志、retryer，以及仅本地模式启用的 balancer。
2. worker pool 通过 `WorkerFactory` 创建 `Worker`，绑定任务通道并启动；`on_pool_started` 可把 `RunningPool` 交给外部引擎做在线调并发。
3. producer 的 loader 调用 `Engine::LoadIngestData`。本地模式按 worker 并发启动 generator，外部模式只启动一个；每批生成完成后先为全部 job 绑定同一个 `JobResources` 并整体 `ref`，再逐个提交，避免较早任务完成时提前释放整批数据。
4. 本地模式的提交先进入 `storeBalancer`，balancing 线程选择负载合适的 job 再发给 pool；外部模式直接发送。发送失败或取消时由 `OwnedJob::Drop` 回收引用。
5. `Worker::HandleTask` 调用具体 `RegionJobWorker`。正常输出重新包装后发送；错误进入 `Group::fail`；panic 被转成 `InvalidData("region job worker panic")` 并增加 `PanicCounter`。`StoreLoadGuard` 无论哪条路径都释放 peer store load。
6. dispatcher 对 `Ingested` 调用 `done`；对 `RegionScanned`/`Wrote` 增加重试计数并以 `min(2^retry_count, 30s)` 延迟重投；`NeedRescan` 在此层是不变量破坏，返回 `InvalidData`。
7. producer 等待 `Group.pending == 0` 或取消后取消 worker context。主线程观察到该 context 后执行 `before_release`、`WorkerPool::Release`，再读取晚到的 operator error；只有 release 无错且父/worker 均未取消才发布 `succeeded`。
8. retry、balancing、producer、dispatcher 和 monitor 全部 join；成功路径读取原子统计，错误路径返回 `Group` 保存的首错，返回成功前再次检查父取消。

## 数据与状态

`RegionJob` 的主要状态机为 `RegionScanned -> Wrote -> Ingested`，可重试的前两种状态由 dispatcher 回队；`NeedRescan` 应在 worker 内被重新生成子任务，不能抵达 dispatcher。`RegionJob::convertStageTo(Ingested)` 调用 `JobResources::finish`，因此统计只在首次进入最终阶段时累计。

`Group.pending` 是批次资源生命周期的权威计数。`JobResources::reference` 先增加 pending 再 `IngestData::IncRef`；`done` 先执行可能阻塞的 `DecRef`，再减少 pending 并通知条件变量。该顺序确保 `do_import` 的完成意味着数据清理也完成，而不只是 job 逻辑完成。

`OwnedJob` 用 `Option` 表示所有权是否已转交：`take` 后包装不再负责清理；保留 `Some` 的包装离开作用域时自动 `done`。`StoreLoadGuard` 以同样的 RAII 方式管理 balancer 的 peer 负载记账。

`LocalData` 保存本地 Engine 的快照 KV、提交时间戳和原子引用数。`DataIter` 自有过滤后的 KV 副本、当前位置和取消 context；`First`/`Next` 推进位置，`Valid` 同时检查索引范围与取消，`Close` 清空位置。

## 依赖与调用关系

上游直接调用边经 RustCodeGraph 文件关系与 `rg` 核对：

- `pkg/ingestor/ingestctrl/local.rs` 两次调用 `do_import`，分别实现本地 Engine 和 external Engine 路径；外部路径通过 `on_pool_started` 调用 `ExternalEngine::SetWorkerPool`。
- `pkg/dxf/importinto/write_ingest_backend.rs::ImportEngine` 调用 `do_import`，其 generator 扫描 Region 并写入数据时间戳，worker 通过 `RegionJob::ingest_data` 执行 write/ingest/rescan。
- `pkg/ingestor/ingestctrl/job_worker.rs::RegionJob` 持有 `JobResources`，其 `ref`、`done`、`convertStageTo` 回调本文件的资源与统计方法。

主要下游是 `engineapi::{Engine, IngestData, ForwardIter, Context}`、`resourcemanager/pool/workerpool::{WorkerPool, Worker, Channel, Context}`、`region_job::{regionJobRetryer, storeBalancer}` 和 `job_worker::{RegionJob, RegionJobWorker}`。`Cargo.toml` 将 engineapi、workerpool、membuf、metrics 声明为直接依赖；大量 Windows 条件依赖属于整个 crate 的其他移植代码，不是本文件执行路径特有依赖。

## 错误处理与边界

- `Group::fail` 保留首个业务错误，随后错误只负责继续取消；worker pool 的字符串错误只用于 operator context，最终尽量返回原始 `Error`。
- engineapi 的动态错误由 `engine_error` 下转为本 crate `Error`；无法下转时保留消息并转成 `InvalidData`。
- worker factory 失败、loader/generator/worker panic、worker `Close` 失败、balancer/retryer 失败、通道断开与父取消都有显式路径。worker panic 和 loader panic分别转成稳定的 `InvalidData` 文本。
- dispatcher 不能把结果通道关闭误判为成功；它调用 `wait_pool_outcome`，等待 release 后的 `succeeded` 或取消。超过 30 次重试时优先返回 `last_retryable_cause`，其次是字符串错误，最后才构造兜底错误。
- `concurrency == 0` 在创建初始 pool 时被归一为 1，但 `ImportPoolTuner::Tune(0)` 明确报错；这两个 API 的边界语义不同。
- `DataIter::Key`/`Value` 要求调用者先确认 `Valid`，否则索引解包或访问会 panic，这是 `ForwardIter` 调用协议而非内部容错点。

## 并发与资源生命周期

所有辅助线程位于 `std::thread::scope` 中，`do_import` 不会在它们退出前返回。monitor 在父 token、worker context 和 engine API context 之间传播取消；producer、retry、balancing、dispatcher 分工持有任务，退出时必须转交或清理各自所有权。

成功发布被刻意放在 `workers.Release()` 之后：release 会 join worker 并运行 `Close`，因此晚到的关闭错误仍能进入最终结果。dispatcher 即使先看到结果通道关闭，也会等待该 outcome。最终 join 顺序为 producer、dispatcher、retry、balancing、monitor，且 balancer 在 producer/retry 均停止添加任务后才排空队列。

同步原语各有明确职责：`Mutex<Option<Error>>` 保存首错；`Mutex<usize> + Condvar` 等待 pending 清零；`AtomicBool` 发布 producer/retry 完成及 pipeline success；`AtomicI64` 以 Relaxed 次序累计最终统计，因为线程 join/最终控制流提供生命周期边界。毒化只在 `RunningPool::Tune` 显式映射为 `Error::Poisoned`；其他内部 mutex `unwrap` 依赖“不在持锁区 panic”的实现约束。

## 与 Go 版本的对应关系

Go 对照主体是 `pkg/ingestor/ingestctrl/local.go::Backend.doImport`，dispatcher/retryer/balancer 位于 `region_job.go`。两版均保持 `prepare/generate -> optional storeBalancer -> workerpool -> dispatcher -> retryer` 拓扑、job ref/done 守恒、错误取消传播，以及“worker pool 完成清理后才确认成功”的退出协议。

Rust 将 Go 的 `WaitGroup` 具体化为 `Group.pending + Condvar`，将通道竞争中的所有权显式化为 `OwnedJob::Drop`，并用 scoped threads/join 表达 goroutine error group 的生命周期。Go 的 failpoint 边界在 Rust 中由 `ImportOptions` hook 注入。Go dispatcher 对不可达 `needRescan` 执行 panic；Rust 返回 `InvalidData`，避免在调度线程中扩大 panic。当前 Rust 常量 `MAX_RETRIES` 为 30，而所读 Go `local.go` 的一般 `maxRetryTimes` 为 20、dispatcher 使用 `MaxWriteAndIngestRetryTimes`；因此不要假设两处数值常量完全相同，应以各自源码和回归测试为准。

本文件还包含 Rust 本地适配层（`LocalEngineSource`、`LocalData`、`DataIter`），Go 本地 `Engine` 自身已直接实现 `LoadIngestData`，所以并非逐类型一一对应，但对外 `engineapi` 语义相同。

## 扩展指南

- 新增 pipeline 阶段时，应同时修改 `RegionJobStage` 的 worker 状态转换和 `dispatch_results` 分支，明确该阶段由谁持有、何时重试、何时 `done`；测试放在独立的 `local_test.rs` 或更聚焦的独立 `*_test.rs`，不要内嵌到生产文件。
- 新增提交队列或异步中转时必须沿用 `OwnedJob` 式所有权：任何取消、发送失败、panic 或队列排空路径都要恰好释放一次 job，并同步维护 store load guard。
- 调整完成协议时必须保留“`Release`/worker `Close` 之后才设置 success”以及“`DecRef` 先于 pending--”两个不变量，否则调用方可能在后台仍占资源或已有晚到错误时收到成功。
- 改动外部引擎吞吐策略时，重点评估 generator 数量、`LoadIngestData` 的零缓冲背压和 resident data；不能直接为 external 模式启用当前无背压的 `storeBalancer`。
- 新增可重试错误信息时要填充 `last_retryable_cause`，以便重试耗尽后保留具体错误类型；同时更新 `local_test.rs` 的重试上限与清理断言。
- 修改 `LocalData` 范围语义或迭代器时，应保持 `[lower, upper)`、取消感知及 `First/Valid/Next` 协议，并补充独立测试覆盖空范围、边界键和取消。
- 性能风险主要在 1ms 轮询、快照 KV 克隆、外部 generator 串行化、balancer 无背压和高并发 worker；兼容性风险集中于 `engineapi`/workerpool trait 契约和 Go 退出顺序。

## 验证依据

- RustCodeGraph：`files --filter pkg/ingestor/ingestctrl/import_pipeline.rs` 确认文件已索引且含 89 个符号；`node --file ... --offset 1/495` 读取全部 824 行；`query do_import --kind function` 定位主入口；`callees import_pipeline.rs::do_import` 核对 `Group::fail`、`OwnedJob`、`Worker`、`RunningPool`、`dispatch_results` 等内部边。精确 callers 查询未返回可用结果，故调用方改用源码引用核对。
- 生产源码：`pkg/ingestor/ingestctrl/import_pipeline.rs`、`lib.rs`、`local.rs`、`job_worker.rs`、`region_job.rs`，以及 `pkg/dxf/importinto/write_ingest_backend.rs`。
- crate 边界：`pkg/ingestor/ingestctrl/Cargo.toml` 的 `[lib] path = "lib.rs"`、直接依赖与 `package.metadata.porting.go-package`。
- Go 对照：`pkg/ingestor/ingestctrl/local.go::Backend.doImport` 与 `pkg/ingestor/ingestctrl/region_job.go::{dispatcher, regionJobRetryer, storeBalancer}`。
- 独立 Rust 测试：`pkg/ingestor/ingestctrl/local_test.rs` 的 `context_cancellation_waits_for_running_workers`、`dispatcher_handles_error_shutdown_when_result_channel_closes`、`dispatcher_propagates_context_cancellation`、`import_pipeline_marks_success_after_worker_cleanup`、`import_pipeline_retains_error_set_during_worker_release`、`import_pipeline_recovers_worker_panic_and_releases_data`、`import_pipeline_generation_error_cancels_loader`、`import_pipeline_retry_limit_preserves_error_and_releases_data`、`dispatcher_cancellation_interrupts_open_result_channel`、`external_import_pipeline_tunes_and_closes_actual_region_workers`；Go 语义测试位于 `local_test.go` 和 `region_job_test.go`。
- 本任务只做文档分析，按计划不运行 Cargo；交付前仅执行任务规定的文件存在与 11 个固定二级标题结构校验，并人工复核未把未验证的图查询结果写成事实。
