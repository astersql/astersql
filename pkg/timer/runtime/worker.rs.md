# `pkg/timer/runtime/worker.rs`

## 文件定位

`worker.rs` 是 `astersql-timer-runtime` crate 的 Hook Worker 实现。crate 入口在 [`lib.rs`](lib.rs) 中以 `pub mod worker` 暴露本模块；crate 边界和直接依赖由 [`Cargo.toml`](Cargo.toml) 定义，其中本文件实际使用 `timer-api` 提供的定时器模型/存储/Hook 接口、`crossbeam-channel` 提供的有界通道，以及 `chrono` 的时间运算。

它位于分组运行时与业务 Hook 之间：[`runtime.rs`](runtime.rs) 的 `TimerGroupRuntime::tryTriggerTimerEvents` 扫描到期缓存项、构造 `TriggerEventRequest` 并投递到按 `HookClass` 划分的 Worker；Worker 执行 `Hook::OnPreSchedEvent`、以带版本检查的更新把事件置为 Trigger、再执行 `Hook::OnSchedEvent`；最终 `TimerGroupRuntime::handleWorkerResponse` 根据 `TriggerEventResponse` 更新缓存记录、处理状态和下一次重试时间。

## 核心职责

- `newHookWorker` / `newHookWorkerWithRetry` 创建容量为 `workerRecvChanCap`（128）的请求通道，并启动一个独立线程；每个 Worker 串行消费其通道中的请求。
- `runWorkerSession` 管理单个 Hook 实例的 `Start`/请求循环/`Stop` 生命周期，`spawnWorkerLoop` 在会话级 panic 后等待并重建会话。
- `triggerEventWithCounters` 实现单次触发的状态机：处理过期手动请求、执行 PreSched、通过乐观版本检查持久化 Trigger 状态、重新读取权威记录、校验 EventID，然后执行 Sched 回调。
- `TriggerEventResponse` 将结果编码为三类信息：成功与否、可选的新记录（包括“明确已删除”）、可选重试延迟（缺省表示立即重试）。这三者由 `runtime.rs::handleWorkerResponse` 解释。
- `WorkerCounters` 以 Relaxed 原子计数记录请求数和两类 Hook 回调的调用、错误、延迟分支，供测试与轻量观测使用。

## 主要符号

- 常量：`workerRecvChanCap`、`workerRespChanCap` 分别限定请求/响应有界通道容量为 128；`workerEventDefaultRetryInterval` 是普通失败的 10 秒退避；`chanBlockInterval` 表示与 Go 版本一致的“长时间阻塞”窗口，但 Rust `responseChan` 实际以不超过 10 毫秒的小步轮询取消；`hookWorkerRetryLoopInterval` 为会话 panic 后的 10 秒等待。`hookWorkerRetryRequestInterval` 被传入会话但当前参数名为 `_retryRequestWait`，没有参与请求级退避。
- `TriggerEventRequest`：携带 `eventID`、触发时的 `TimerRecord` 快照、`TimerStore` 和专属响应发送端。`DoneResponse`、`RetryDefaultResponse`、`TimerMetaChangedResponse` 统一构造响应；后者同时附带最新元数据并清除延迟，表示应立即按新状态重算。
- `TriggerEventResponse`：`newTimerRecord: OptionalVal<Option<TimerRecord>>` 是双层可选值——未设置表示无需替换缓存，设置为 `Some(record)` 表示刷新，设置为 `None` 表示记录已删除。`retryAfter` 未设置表示立即重试；设置值表示延后重试。私有构造函数 `done` / `retry` 当前没有进入主流程，公开链式方法用于调整响应。
- `TimerEvent`：把事件 ID 与记录快照适配成 `TimerShedEvent`，两个访问器都返回拥有所有权的克隆，隔离 Hook 对 Worker 本地记录的直接修改。
- `WorkerCounters`：六个 `AtomicU64` 字段分别对应 trigger、PreSched、PreSched error、PreSched delay、Sched、Sched error。
- `HookFactoryFn`：线程安全的 `Arc<dyn Fn() -> Box<dyn Hook>>` 工厂；每次会话重启都会重新调用工厂创建 Hook。
- `HookWorker`：公开请求发送端 `ch` 和计数器，私有取消上下文与受 `Mutex` 保护的 `JoinHandle`。`stopAndJoin` 只负责 join；调用方必须先取消共享上下文。
- `newHookWorker` / `newHookWorkerWithRetry`：前者填入默认等待参数，后者建立通道、线程和共享计数器。`groupID`、`hookClass` 在当前 Rust 实现中以 `_` 前缀接收但未保存或用于日志。
- `spawnWorkerLoop`、`runWorkerSession`、`sleepWithContext`：分别负责跨会话恢复、单次 Hook 生命周期、可取消等待。
- `triggerEvent`、`triggerEventWithCounters`：前者是使用临时计数器的公开单次调用入口，后者是 Worker 真正调用的核心实现。
- `responseChan`：在成功发送、上下文取消或接收端断开之一发生前重试有界发送。
- `buildEventUpdate`：构造 Idle→Trigger 的 `TimerUpdate`，写入事件 ID、开始时间、Hook 生成的数据、原版本、watermark，并在手动请求场景标记请求已由当前事件处理。

兼容机械翻译命名的别名 `triggerEventRequest`、`triggerEventResponse`、`hookWorker` 仍存在，但主运行时使用公开的 PascalCase 类型。

## 执行流程

1. `runtime.rs::TimerGroupRuntime::tryTriggerTimerEvents` 从缓存收集可触发记录；`ensureWorker` 按 `HookClass` 查找或调用 `newHookWorker` 创建 Worker，并把运行时的存储、时间函数和 Hook 工厂封装进去。
2. 运行时构造 `TriggerEventRequest`。空 EventID 会生成 UUID，已有 Trigger 事件则复用记录中的 EventID。请求经 `worker.ch.try_send` 成功后，缓存状态被置为 `procTriggering`；通道满或断开则将下次尝试推迟 `retryBusyWorkerInterval`。
3. `spawnWorkerLoop` 在线程中调用 `runWorkerSession`。会话创建 Hook，调用 `Start`，以 10 毫秒超时轮询请求通道，使取消可及时生效。通道断开或上下文取消会结束会话。
4. 每个请求在独立 `catch_unwind` 中调用 `triggerEventWithCounters`；该调用 panic 时生成默认 10 秒重试响应，不结束当前 Hook 会话。响应随后由 `responseChan` 发回运行时。
5. 核心触发首先检查手动请求是否已超时。若超时，以 `CheckVersion` 标记请求已处理；成功后重读记录并返回立即重试的元数据变更响应。记录已删除和普通存储错误走不同响应分支。
6. 若快照仍为 `SchedEventIdle`，先调用 `OnPreSchedEvent`。Hook 返回正 `Delay` 时不写存储，按指定延迟重试；返回错误时按默认间隔重试；成功且无延迟时由 `buildEventUpdate` 生成带原版本检查的 Trigger 更新。
7. 更新遇到 `ErrVersionNotMatch` 时重读当前记录并立即交回运行时；遇到 `ErrTimerNotExist` 时明确返回删除标记；其他错误按默认间隔重试。
8. 无论请求快照最初是 Idle 还是 Trigger，Worker 都重新 `GetByID`。若当前 `EventID` 已不是请求的 EventID，说明事件已关闭或被替代，返回最新记录并立即重算；匹配时才调用 `OnSchedEvent`。
9. `OnSchedEvent` 成功返回成功响应和最新记录；错误则保留最新记录并附默认退避。运行时收到成功响应后把缓存处理状态置为 `procWaitTriggerClose`，收到失败响应后恢复 `procIdle`，必要时写入下一重试时间。
10. 会话退出时始终尝试调用 `Hook::Stop`。会话体或 `Stop` 的 panic 会继续抛到 `spawnWorkerLoop`，后者在可取消等待后用新 Hook 重启；取消则直接结束线程。

## 数据与状态

源文件不维护定时器全集；请求内的 `timer` 只是运行时投递时的快照，`TimerStore` 才是触发状态的权威来源。Idle→Trigger 更新携带 `CheckVersion = req.timer.Version`，防止旧快照覆盖并发变更；更新后再次读取确保传给 `OnSchedEvent` 的是已持久化、EventID 仍匹配的记录。

事件数据的状态转换由 `buildEventUpdate` 明确限定：`EventStatus=Trigger`、`EventID=req.eventID`、`EventStart=nowFunc()`、`EventData=PreSchedEventResult.EventData`，并把原 `Watermark` 放入 `EventExtra`。手动请求还会把 `ManualRequestID` 复制到 `EventExtra.EventManualRequestID`，同时以当前 EventID 标记请求已处理。

响应中的两个 `OptionalVal` 必须按“是否 Present”而不是仅按内层值判断：`newTimerRecord` 未出现、出现且为记录、出现且为空分别代表不改缓存、刷新缓存、删除缓存；`retryAfter` 出现时由运行时计算绝对重试时间，未出现时保留立即重新评估的语义。

计数器只用于统计分支进入次数，不参与控制流。它们使用 `Ordering::Relaxed`，只保证原子数值更新，不提供跨字段一致快照或其他内存同步语义。

## 依赖与调用关系

上游主链（由 [`runtime.rs`](runtime.rs) 直接源码核对）为：

`TimerGroupRuntime::loopOnce` → `tryTriggerTimerEvents` → `ensureWorker` → `newHookWorker` → `HookWorker.ch` → `runWorkerSession` → `triggerEventWithCounters` → `responseChan` → `TimerGroupRuntime::handleWorkerResponse`。

下游依赖包括：

- `timer-api`：`Context`/取消语义，`Hook`/`TimerShedEvent` 回调协议，`TimerRecord`/`TimerUpdate`/`EventExtra` 数据模型，`TimerStore::{Update, GetByID}` 持久化接口，以及 `ErrVersionNotMatch`、`ErrTimerNotExist` 可分类错误。
- `cache.rs`：仅复用 `NowFn` 和 `systemNow`；缓存本身由 `runtime.rs` 根据 Worker 响应更新。
- `crossbeam-channel`：请求通道、响应通道和可取消的 `send_timeout`/`recv_timeout` 轮询。
- 标准库线程、`JoinHandle`、`Arc<Mutex<_>>`、原子计数器和 `catch_unwind`：构成 Worker 的隔离、共享所有权与恢复边界。
- `chrono`：把标准库 `Duration` 转为时间戳偏移，用于判定手动请求超时。

RustCodeGraph 已将 `worker.rs` 识别为 48 个符号，并索引了相邻的 `runtime.rs`、`worker.go`、`worker_test.rs`、`worker_test.go`；精确 `callers/callees` 查询因 Go/Rust 同名符号没有输出直接边，因此上述主链来自索引源码中的显式构造、发送和接收点，而不是由缺失的图边推断。

## 错误处理与边界

- Hook 的普通 `Err` 不传播出 Worker：PreSched 错误返回默认退避；Sched 错误返回默认退避并附最新记录。
- Hook 工厂、`Start`、会话体或 `Stop` 的 panic 属于会话级故障，由外层循环恢复并重建 Hook；单个请求中 PreSched/Sched 的 panic 被请求级 `catch_unwind` 转成默认退避响应，因此同一会话可继续处理后续请求。
- 存储版本冲突会优先重读权威记录。重读成功或确认删除时返回立即重算的元数据响应；无法重读时退化为默认退避。一般存储错误同样使用默认退避。
- 手动请求超时使用严格的 `nowFunc() > request_time + timeout`；刚好等于截止时间尚不视为超时。若时间转换或加法失败，`timedOut` 为 false，流程继续尝试正常触发。
- 更新超时手动请求时若 `Update` 返回 `ErrTimerNotExist`，Rust 与现有 Rust 测试保留请求快照并返回立即重算；这是一个特意与测试固定的边界行为。
- `responseChan` 在响应通道持续满时不会因“60 秒”主动丢弃响应；它持续轮询直到发送成功、上下文取消或接收端断开。`chanBlockInterval` 在 Rust 中只参与计算轮询间隔的上限，没有 Go 版本的周期警告日志。
- `HookWorker::stopAndJoin` 不会自己触发取消。如果调用方在活动上下文上直接 join，可能永久等待；正常调用方式由 `TimerGroupRuntime::Stop` 和测试辅助函数展示为先 cancel、后 join。
- Rust 请求字段是拥有所有权的非空值，因此 Go 版本对 `nil` request/timer/resp 的防御分支没有对应的可构造安全 Rust 状态；响应接收端提前释放则由 `Disconnected` 处理。

## 并发与资源生命周期

每个 `HookWorker` 拥有一个后台 OS 线程和一个容量 128 的多生产者请求通道。单个 Worker 内请求严格串行，因此同一 Hook 实例不会被本文件并发调用；不同 `HookClass` 的 Worker 可并行运行。Go 源码中也注明未来可并行多个请求循环，Rust 当前同样没有实现这种并行化。

`HookWorker` 可克隆，但克隆共享同一发送端、上下文、计数器与 `join` 槽。`stopAndJoin` 通过 `Mutex<Option<JoinHandle>>` 的 `take` 保证最多一个克隆实际 join。锁中毒时统一通过 `into_inner` 继续访问，避免管理路径因先前 panic 再次失败。

Hook 生命周期以会话为单位：工厂创建 → `Start` → 零到多个请求 → `Stop`。请求级 panic 不重建 Hook；工厂/Start/Stop 或会话外层 panic 会终止当前会话并在等待后创建新 Hook。`sleepWithContext` 和 10 毫秒通道轮询都检查取消，使长恢复等待、空闲接收和阻塞响应发送可以快速停止。

响应通道通常是运行时共享的容量 128 通道；每个请求也携带其发送端，测试可使用专属通道。若发送阻塞，Worker 暂停消费后续请求，形成有界背压；取消或断开可解除阻塞。

## 与 Go 版本的对应关系

Rust 实现以 [`worker.go`](worker.go) 为直接对照，保留了通道容量、默认退避、请求/响应字段语义、Idle→PreSched→持久化 Trigger→重读→Sched 的顺序、手动请求信息写入 `EventExtra`、版本冲突/删除分类，以及 Hook Start/Stop 和 panic 恢复意图。[`worker_test.rs`](worker_test.rs) 对应复刻 [`worker_test.go`](worker_test.go) 的成功、延迟、错误、手动请求和恢复场景。

已确认的实现差异如下：

- Go Worker 保存 `groupID`/`hookClass`、结构化 logger 和 Prometheus counter；Rust 构造函数接受前两者但不保存，使用本地原子计数器，也没有等价日志输出。
- Go `withRecoverUntil` 的请求循环保存 `unhandledRequest`，panic 后第一次重试会直接发送失败响应，并使用可注入的 `retryRequestWait`；Rust 在单请求周围直接 `catch_unwind` 并立即返回默认失败响应，因此 `_retryRequestWait` 当前未使用，但外部可见的首个 panic 响应和会话继续处理行为由 Rust 测试覆盖。
- Go 通过 `select` 同时等待发送、取消和 60 秒日志 ticker；Rust 用最多 10 毫秒的 `send_timeout` 轮询，保留取消/断开语义但没有阻塞告警。
- Go 指针类型允许并防御 nil 请求、nil timer、nil response channel；Rust 类型系统排除了这些安全代码路径。
- Rust 为可恢复线程显式保存 `JoinHandle` 并提供 `stopAndJoin`；Go 把 goroutine 交给 `WaitGroupWrapper` 管理。
- Rust `TimerEvent` 返回克隆记录，Go 返回原记录指针；这使 Rust Hook 获得快照所有权，而非共享可变身份。

这些差异是当前源码事实。尤其是未使用的请求级等待、缺少生产指标/日志，不应在文档中描述为已实现的 Go 等价能力。

## 扩展指南

- 新增触发状态或持久化字段时，优先修改 `triggerEventWithCounters` 和 `buildEventUpdate`，保持“带 `CheckVersion` 更新后重读并校验 EventID”的并发不变量；同时扩展独立的 [`worker_test.rs`](worker_test.rs)，不要把测试内嵌到生产文件。
- 新增响应语义时，要同步检查 `runtime.rs::handleWorkerResponse` 对 `success`、`newTimerRecord`、`retryAfter` 的解释，特别避免混淆“字段未设置”和“明确删除记录”。
- 改变 Hook 调用顺序或重试策略时，需要同时核对 Go 的 `worker.go::{handleRequestLoop, triggerEvent}` 与两侧 `worker_test`；若有意产生差异，应补充明确测试和迁移说明。
- 引入请求并行处理前，必须确认同一 Hook 是否允许并发调用、同一 timer/event 是否可能重复执行 `OnSchedEvent`、响应乱序如何影响 `runtime.rs` 缓存状态，并评估通道容量与背压。
- 若接通 `groupID`/`hookClass` 日志或生产指标，应避免只依赖 `WorkerCounters` 的测试用途原子值；还需考虑 Worker 重启后标签/计数器是否延续。
- 若启用当前未使用的 `retryRequestWait`，应先定义它与请求级 panic 立即失败响应的关系，并更新 `TestHookWorkerLoopHandleRequestPanicRecover`；不要仅加入 sleep 而改变 Go 首次恢复的可见时序。
- 生命周期变更需保持先取消后 join，并覆盖工厂、Start、Stop 持续 panic时的快速停止，以及响应通道阻塞时的取消路径。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/timer/runtime` 找到 16 个 Rust/Go 实现与测试文件；`node --file pkg/timer/runtime/worker.rs` 核对了目标文件 526 行和 48 个符号；另用 `node` 核对 `runtime.rs`、`worker.go`、`worker_test.rs`、`worker_test.go`。对 `newHookWorker`、`triggerEvent`、`buildEventUpdate` 执行的精确 `callers/callees` 查询未返回直接边，故没有把图中不存在的边作为证据。
- crate 与装配：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。当前目录没有 `doc.go`。
- Rust 生产主链：[`runtime.rs`](runtime.rs) 中 `tryTriggerTimerEvents`、`ensureWorker`、`handleWorkerResponse`；[`worker.rs`](worker.rs) 中请求/响应类型、Worker 循环、`triggerEventWithCounters`、`responseChan`、`buildEventUpdate`。
- Go 对照：[`worker.go`](worker.go) 中 `newHookWorker`、`handleRequestLoop`、`triggerEvent`、`responseChan`、`buildEventUpdate`。
- Rust 独立测试：[`worker_test.rs`](worker_test.rs) 的 `TestWorkerStartStop`、两条成功路径、`TestWorkerProcessDelayOrErr`、`TestWorkerProcessManualRequest`、两个 panic 恢复测试和 `TestWorkerBlockedResponseStopsOnCancel`。
- Go 独立测试：[`worker_test.go`](worker_test.go) 的对应 Start/Stop、成功、错误/竞态、手动请求和 panic 恢复测试。
- 本任务是纯文档分析，按任务约束未运行 Cargo；验证以源码/调用点/对照测试的静态事实复核和任务规定的 11 章节结构检查为准。
