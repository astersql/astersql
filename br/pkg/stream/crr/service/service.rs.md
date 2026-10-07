# `br/pkg/stream/crr/service/service.rs`

## 文件定位

本文件是 `astersql-br-pkg-stream-crr-service` library crate 的服务层主实现。crate 边界由同目录 `Cargo.toml` 和 `lib.rs` 确定：`lib.rs` 以 `pub mod service` 挂载本文件并用 `pub use service::*` 导出 API，本 crate 直接依赖 `astersql-br-pkg-stream-crr-internal-checkpoint` 与 `prometheus`。它位于 CRR（跨区域复制）checkpoint 路径的外层：内层 `Calculator` 负责计算可安全推进的 checkpoint，本文件则负责循环驱动、失败重试、无进展等待、resume 状态持久化与运行状态联动。

当前接线存在明确边界：`br/pkg/stream/crr/config/config.rs` 直接依赖本 crate 的 `Config` 和 `DefaultRetryInterval`；但顶层 `br/pkg/task/operator/crr_checkpoint.rs` 当前从同包 `stubs.rs` 引入另一套 `CRRService`/`NewCRRService`，且 operator manifest 未引用本 crate。因而本文件是有独立行为测试的实现，不能据此宣称它已经被 operator 生产组装路径调用。

## 核心职责

- `New` 校正重试间隔，创建 `StatusStore`/`StatusObserver`，把 PD 元数据、上游存储与同步检查器注入内层 `NewCalculator`，并保留独立的 checkpoint watcher 和可选 resume store。
- `Service::Run` 建立合作式取消的长运行 worker，反复执行 `run_once`；可重试失败会记录状态并休眠，context 取消则正常退出。
- `run_once` 把一轮定义为“恢复/刷新 resume 状态 → 通知新计算轮 → 计算下一 checkpoint → 排队并持久化快照 → 无进展时等待 PD”。
- `RunOnceError` 区分已由 calculator observer 上报的失败和服务层失败，避免同一错误重复累加失败计数。
- `RunStopGuard` 在任何 `Run` 退出路径上尝试刷新未落盘状态，再将服务标记为 stopped。

HTTP 路由同 crate 的 `http.rs` 注册，状态机与 metrics 分别在 `status.rs` 和 `metrics.rs`；本文件只维护它们所需的 `StatusStore` 和 observer 事件。

## 主要符号

- `DefaultRetryInterval: Duration`：外层错误重试默认值 1 秒，同时是关机刷盘 context 的超时上界。
- `Config { CalculatorConfig, RetryInterval }`：将内层 `CheckpointCalculatorConfig` 和外层重试策略组合起来；`Default` 委托 calculator 默认配置。`CalculatorConfig` 只是 `CheckpointCalculatorConfig` 的别名。
- `UpstreamCheckpointWaiter::WaitGlobalCheckpointAdvance(&Context, &str, u64)`：checkpoint 未前进时的阻塞抽象，与 `PDMetaReader` 分离以便注入错误、延迟和取消。
- `ResumeStateStore::{LoadState, SaveState}`：可选持久化边界，载荷是 checkpoint crate 的 `PersistentState`。`None` 是合法配置，表示只计算而不恢复/落盘。
- `NewExistenceSyncChecker<C>`：将 `FileExistenceChecker` 包装为 `Box<dyn ObjectSyncChecker>`，实际判定语义在 checkpoint crate 的同名实现中。`ObjectSyncCheckerAlias` 是 trait object 别名，本文件内无其他使用。
- `Deps`：构造期依赖集合。`PD` 读元数据，`Watcher` 等待全局 checkpoint，`Upstream` 读 meta/log 对象，`Sync` 确认下游对象可用，`State` 可选地持久化 resume 状态。
- `Service`：拥有 calculator、status/observer、watcher、state store 和配置。`calc`、`resume_state_initialized` 与 `pending_resume_state` 均用 `Mutex` 保护。
- `New(Deps, Config) -> Result<Service, Error>`：唯一公开构造器。Rust 的 `Box<dyn ...>` 字段不能为 nil，因此不需要 Go 版对 nil watcher 的运行时检查。
- `Service::{Run, Status}`：公开运行入口和一致性状态快照入口。其余方法均是 crate 内部循环、错误和持久化机制。
- `RunStopGuard`、`RunOnceError`、`should_stop`、`sleep_context`、`SystemTimeNow`：分别封装 RAII 清理、错误来源标记、取消判定、可中断重试休眠与失败事件时间戳。

## 执行流程

1. `New` 将零或非法重试间隔替换为 1 秒，用任务名创建 status store/observer，再调用 checkpoint crate 的 `NewCalculator`。构造 calculator 失败会原样返回 `Error`。
2. `Run` 首先调用 `status.start()`，然后建立 `RunStopGuard`。每轮顶部先检查 `ctx.Err()`；已取消时以 `Ok(())` 结束。
3. `run_once` 调用 `prepare_resume_state`。首轮由 `initialize_resume_state` 从可选 store 加载快照，交给 `Calculator::RestorePersistentState`，并同步 status；只有整个步骤成功才将 initialized 置为 true。后续轮次则先重试上次未落盘的 pending 快照。
4. observer 开启新轮次后，服务在 calculator 锁下读取 `LastCheckpoint`，再在另一次加锁中执行 `ComputeNextCheckpoint`。计算错误转成 `ObservedCalculator`，表明内层 observer 已上报过失败。
5. 只当配置了 state store 且 `next_checkpoint > last_checkpoint` 时，`queue_resume_state_save` 才获取 `StateSnapshot` 并替换 pending 快照。紧接着 `flush_pending_resume_state` 进行持久化；成功后先更新 status，再清空 pending 与失败状态。
6. 如果 checkpoint 未进展，`wait_checkpoint_advance` 把任务名和当前水位交给 watcher，直到 PD 通知前进、返回错误或 context 取消，避免循环空转。
7. `retry_after_run_error` 先用 `should_stop` 判断是否为取消/超时终止。其他 `Other` 错误经 `record_service_failure` 转为 `EventCalculationFailed`，`ObservedCalculator` 不再上报；随后 `sleep_context` 以最多 5ms 步长检查取消。
8. `Run` 退出时 `RunStopGuard::drop` 用 `Context::WithTimeout(Background, RetryInterval)` 尽力刷新 pending，失败只写入标准错误流；最后 `status.stop()` 保证 Live/Ready 为 false。

## 数据与状态

`Service` 有三组关键状态。计算状态位于 `Calculator`，通过 `LastCheckpoint`、`RestorePersistentState`和 `StateSnapshot` 与外层交互；观测状态位于 `StatusStore`，由 `StatusObserver` 消费 calculator 事件，并由服务层补充 resume/watch 失败与 start/stop 转换；持久化状态位于 `pending_resume_state`，是“已经计算出但还未成功保存”的单个最新快照。

`resume_state_initialized` 是一次性门闩：Load 或 Restore 失败时保持 false，下轮会重试；无 state store 或 store 返回 `None` 也算初始化成功。`pending_resume_state` 只在 checkpoint 严格前进时生成，Save 失败时保留，成功后才清空。这使 status 中的安全水位对应已持久化快照，而不是仅存在于进程内的计算结果。

`Status()` 调用 `snapshot_copy`返回深拷贝快照；详细字段和 starting/running/degraded/stopped 状态机由同目录 `status.rs` 定义。

## 依赖与调用关系

下游依赖集中在 checkpoint crate：`NewCalculator`、`CalculatorDeps`、`Calculator`、`Context`、`Error`、`PersistentState`、`CheckpointEvent` 以及 PD/存储/同步检查 traits。本 crate 内，`status.rs` 提供 `new_status_store`、`StatusObserver`、`StatusStore`和 `StatusSnapshot`；observer 是 calculator 和对外 status/metrics 之间的桥。

已验证的上游使用者包括：

- `br/pkg/stream/crr/config/config.rs`：构造和解析本文件的 `Config`，引用 `DefaultRetryInterval`。
- `br/pkg/stream/crr/service/lib.rs`：将本模块导出为 crate 公开 API，并挂载 `service_test.rs` 与 `parity_test.rs`。
- `br/pkg/stream/crr/service/http.rs`：通过 `Service::Status`/status store 提供健康和状态端点；HTTP 注册不在本文件内。
- `br/pkg/stream/crr/service/service_test.rs` 和 `parity_test.rs`：直接构造 `Service`、调用 `Run`/`Status`，并实现 watcher 与 resume store 测试替身。RustCodeGraph 的 `Service` 节点也记录了 `parity_test.rs::contract_normal_status_and_http` 的实例化边。

未形成的调用边同样重要：`br/pkg/task/operator/crr_checkpoint.rs` 目前使用 `br/pkg/task/operator/stubs.rs` 中的同名概念，其 `ResumeStateStore` 签名也没有本文件的 `Context` 参数。安全接入生产 operator 需要显式做类型与资源生命周期适配，不是当前代码已存在的直接调用。

## 错误处理与边界

- `NewCalculator` 的构造错误向上返回；`RetryInterval <= 0` 不报错，而是回落默认值防止忙等。Rust 类型系统使必需的 boxed trait 依赖无法为 nil。
- Load/Save 错误分别增加 `load resume state: ` 与 `save resume state: ` 上下文，以 `RunOnceError::Other` 进入统一重试路径。Restore 错误保留 calculator 的原错误。
- calculator 错误使用 `ObservedCalculator`，watch/resume 错误使用 `Other`。只有后者由 `record_service_failure` 补发失败事件，不可去掉这个区分，否则 `ConsecutiveFailures` 会双计。
- `should_stop` 需要 context 已取消，且错误文本包含 `context canceled` 或 `context deadline exceeded` 才终止重试。这与 Go 的 `errors.Is` 不同，是对当前自定义 `Error` 能力的文本适配；更改错误文本时需同步此判定与测试。
- `Mutex::lock`/status 内部锁使用 `expect`，锁中毒会 panic，不会转为业务 `Error`。`flush_pending_resume_state_on_shutdown` 是 best-effort，错误只 `eprintln!`。
- `sleep_context` 对零延迟会直接成功，但公开构造器已将非正值间隔改为默认值。它使用小步长轮询，取消响应延迟上界约为 5ms，代价是长时间重试等待会多次唤醒线程。
- 没有配置 `State` 时不进行 resume I/O；这不是错误，但重启后无法从外部快照续跑。

## 并发与资源生命周期

`Service` 通过 `&self` 运行，测试会用 `Arc<Service>` 在后台线程执行 `Run`，同时在另一线程读 `Status`。`Calculator` 被 `Mutex` 保护；当前 `ComputeNextCheckpoint(ctx)` 在持锁期间执行，因而其他 calculator 状态访问会被串行化，但 `Status()` 读的是独立 `StatusStore`，不需要 calculator 锁。

resume 的两个 `Mutex` 分开保护“是否已初始化”和“待持久化快照”。方法在锁内只复制/替换值，Load/Save 外部 I/O 不持有这两把锁。然而本类的设计仍假定只启动一个 `Run` 循环：如果并发调用多个 `Run`，多个线程可以同时克隆同一 pending 快照并重复 Save，且 start/stop 状态也会互相覆盖；公开 API 没有内建防重入标志。

watcher 的阻塞生命周期由 `Context` 约束；实现必须在取消后返回，否则 `Run` 不能及时结束。`RunStopGuard` 保证正常返回和 Rust unwind 都会执行关机清理；关机刷盘使用独立 background context，不受已取消的运行 context 影响，但受 `RetryInterval` 超时约束。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `br/pkg/stream/crr/service/service.go`：`Config`、waiter/store traits、`Deps`、`Service`、`New`、`Run`/`runOnce`、resume 方法、错误重试与 `sleepContext` 都保留了相同的职责和分支顺序。Rust `RunStopGuard` 等价于 Go `defer s.flushPendingResumeStateOnShutdown()` 与 `defer s.status.stop()`；其 `drop` 中先 flush 后 stop，与 Go defer 的后进先出实际顺序一致。

需要注意的语言适配有：

- Go 的 `Config` 匿名嵌入 `CalculatorConfig`，Rust 使用显式 `CalculatorConfig` 字段，因此任务名路径是 `cfg.CalculatorConfig.TaskName`。
- Go `New` 检查 `deps.Watcher == nil`；Rust `Box<dyn UpstreamCheckpointWaiter>` 不表示空值，这一不变量由类型系统保证。Go 的可空 `ResumeStateStore` 对应 Rust `Option<Box<...>>`。
- Go calculator 是可变指针，Rust 使用 `Mutex<Calculator>` 为 `&self` 方法提供内部可变性。Go 字段依赖运行时并发约定，Rust trait 要求 `Send + Sync`。
- Go 用 `observedCalculatorError` 及 `errors.As` 防止双计，Rust 用 `RunOnceError` enum。Go 用 `errors.Is` 识别 context 终止，Rust 当前使用错误文本包含判断。
- Go `sleepContext` 用 timer/select，Rust 用 5ms 分段 `thread::sleep`；取消语义相同，但运行时唤醒模型和性能特征不同。
- Go 用结构化 logger 记录 shutdown flush 失败，Rust 当前使用 `eprintln!`，不带结构化字段。

`service_test.rs` 明确标注与 Go `service_test.go` 的对应场景，包括成功推进、错误降级、calculator 失败不双计、watch 阻塞/恢复、resume Load/Save 重试与关机 flush。

## 扩展指南

- 新增 calculator 配置时，优先扩展 checkpoint crate 的 `CheckpointCalculatorConfig`，再同步 `br/pkg/stream/crr/config/config.rs` 的默认值/解析和 Go `Config`；不要在服务层复制 calculator 已有配置。
- 新增外层错误源时，先判断它是否已经通过 observer 上报。已上报路径应使用 `ObservedCalculator` 或等价标记，其他错误进入 `record_service_failure`；同步扩展 `test_service_does_not_double_count_calculator_failure`。
- 修改 resume 策略时必须保留“成功 Save 后才清 pending”与“初始化成功后才置 initialized”两个不变量。相关独立测试是 `test_service_loads_persisted_resume_state`、`test_service_retries_failed_resume_state_persist`、`test_service_retries_failed_resume_state_load` 和 `test_service_flushes_pending_resume_state_on_shutdown_after_persist_failure`。
- 修改等待或重试机制时，要保持 context 可中断，并复核 `test_service_waits_for_checkpoint_watch`、`test_service_recovers_from_checkpoint_watch_error` 以及长 RetryInterval 关机用例。若将 `sleep_context` 换为异步实现，还需重新评估 `Run` 的同步 trait/API 和 watcher 实现。
- 新增状态字段或事件时，接入点是 `record_service_failure`、`StatusObserver` 和 `status.rs`，同时需更新 `http.rs`、`metrics.rs`、`service_test.rs`、`status_test.rs` 与 Go 对照测试。
- 将本实现接入 `br/pkg/task/operator/crr_checkpoint.rs` 时，应删除或隔离当前 operator stubs 的重复概念，实现 `ExternalStorage`/PD mgr 到本文件 traits 的适配，传入 watcher，并明确 `Context` 与 cleanup 的所有权。这是跨 crate 接线任务，不应仅通过同名类型替换。
- Rust 测试继续放在独立 `service_test.rs`/`parity_test.rs`，不要内嵌回生产文件。任何行为修改还应对照 `service.go` 与 `service_test.go`，避免 Rust 路径被简化为只满足现有用例。

## 验证依据

- 生产源码：`br/pkg/stream/crr/service/service.rs`（`Config`、`Deps`、`Service`、`New`、`Run`、`run_once`、resume 方法、`RunStopGuard`、`RunOnceError`、`should_stop`、`sleep_context`）。
- crate 与模块边界：`br/pkg/stream/crr/service/Cargo.toml` 和 `br/pkg/stream/crr/service/lib.rs`；相邻状态实现：`br/pkg/stream/crr/service/status.rs`。该包没有 `doc.go`，因此 Go 语义以同路径生产文件为准。
- Go 对照：`br/pkg/stream/crr/service/service.go` 与 `br/pkg/stream/crr/service/service_test.go`。Rust 独立测试：`br/pkg/stream/crr/service/service_test.rs` 和 `br/pkg/stream/crr/service/parity_test.rs`。
- 上游配置证据：`br/pkg/stream/crr/config/config.rs` 导入本 crate 的 `Config`/`DefaultRetryInterval`；生产接线限制证据：`br/pkg/task/operator/crr_checkpoint.rs`、`br/pkg/task/operator/stubs.rs` 以及全库 Cargo manifest 中只有 CRR config crate 对本 service crate 的路径依赖。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`query UpstreamCheckpointWaiter`、`query ResumeStateStore`、`query NewExistenceSyncChecker`、`query Service` 定位了 Rust/Go 对应定义；`node br/pkg/stream/crr/service/service.rs::Service` 返回字段源码，并给出 `parity_test.rs::contract_normal_status_and_http` 的实例化边。自然语言 `explore` 与最初 `node --file` 未产生输出，故其余直接证据按技能回退规则使用精确 `rg` 和文件读取核验。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验收以目标文件存在且上述 11 个二级标题各出现一次为准。
