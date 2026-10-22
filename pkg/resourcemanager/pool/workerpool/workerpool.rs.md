# `pkg/resourcemanager/pool/workerpool/workerpool.rs`

## 文件定位

定义 `TaskMayPanic`、`Worker<T, R>`、`Tuner` 和 `PoolOption<T, R>`，把任务恢复信息、任务处理、资源关闭和动态调容划分为独立契约。

## 核心职责

1. 用 `Context` 保存第一个业务错误，并以可克隆的 `done` 接收端向整个子上下文树广播取消。
2. 用 `Channel<T>` 在 crossbeam MPMC 通道之上补充显式、幂等的关闭状态，以及“关闭后先排空已缓冲数据”的 Go channel 兼容语义。
3. 定义 `TaskMayPanic`、`Worker<T, R>`、`Tuner` 和 `PoolOption<T, R>`，把任务恢复信息、任务处理、资源关闭和动态调容划分为独立契约。
4. 由 `WorkerPool<T, R>` 创建并管理命名 OS 线程，选择任务、缩容请求或取消信号，捕获任务 panic，维护运行中任务数，并最终关闭结果通道。
5. 对齐 Go `workerpool.go` 的关键行为：非正并发度钳制为 1、无结果占位类型 `None`、启动前调容、缩容握手、首错优先和 `Worker::Close` 错误上报。

本文件不拥有业务任务队列的生产策略，也不决定何时关闭外部传入的任务通道；这些责任属于调用方。`AddTask` 在源码注释中明确是测试侧入口，生产流水线通常通过 `SetTaskReceiver` 注入并直接持有 `Channel<T>`。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `Error` | 公开、可克隆的错误值；消息保存在 `Arc<str>`，实现 `Display`、`StdError` 以及从字符串转换。它保留文本，不保留任意 Go error 的类型链。 |
| `ContextInner` / `Context` | 内部共享状态与公开句柄。`cancelled` 是原子标志，`first_error` 只写一次，`done_sender` 的丢弃表示广播关闭，`children` 保存弱引用以传播父取消。 |
| `NewContext` | 为给定 `Context` 创建子上下文；父取消传向子，子取消不反向影响父。 |
| `ChannelInner<T>` / `Channel<T>` | 可克隆 MPMC 通道。`bounded`/`unbounded` 创建数据通道和独立 close 信号；`send`、`recv`、`recv_timeout`、`close` 暴露主要操作。 |
| `TaskMayPanic` | 任务必须提供 `(label, func_info, Option<Error>)`；panic 时预置错误优先，否则合成包含标签和函数信息的错误。 |
| `Worker<T, R>` | `HandleTask` 消费任务并可经回调发送零到多个结果；`Close` 是每个已创建 worker 的退出清理点。`Box<W>` 有转发实现。 |
| `Tuner` | 不暴露泛型参数的调容接口，只包含 `Tune(numWorkers, wait)`。 |
| `PoolOption<T, R>` | 构造期选项接口，由 `NewWorkerPoolWithOptions` 按传入顺序调用 `Apply`。 |
| `None` | 表示无结果池；`Start` 通过 `TypeId` 识别它并跳过默认结果通道创建。 |
| `Countdown` | `Mutex<usize> + Condvar` 实现的内部倒计时器，缩容且 `wait=true` 时等待被移除 worker 完成 `Close`。 |
| `RunningGuard` | RAII 计数器；进入任务处理时递增 `runningTask`，无论正常返回、错误或 panic 解 unwind 都递减。 |
| `WorkerPool<T, R>` | 池的主体，保存上下文、配置容量、任务/结果通道、无缓冲 quit 通道、线程句柄、worker 工厂和调容时间。 |
| `NewWorkerPool` / `NewWorkerPoolWithOptions` / `NewWorkerPoolWithFallibleFactory` | 普通工厂、带选项工厂和允许返回 `None` 的可失败工厂三个构造入口，最终汇合到 `newWithFactory`。 |
| `Start` / `runAWorker` / `handleTaskWithRecover` | 分别负责一次性启动、创建单个 worker 线程、执行单个任务并统一恢复错误。 |
| `Tune` | 启动前只更新配置容量；启动后扩容会创建线程，缩容会逐个发送退出请求，并可等待 `Close`。 |
| `Release` / `CloseAndWait` / `Drop` | 正常释放、主动取消后释放以及遗忘显式释放时的兜底回收路径。 |

## 执行流程

1. 构造时，`newWithFactory` 将 `numWorkers <= 0` 钳制为 1，记录 `originWorkers`，创建容量为 0 的 `quitSender`/`quitReceiver`，并把 `lastTuneTs` 初始化为 `UNIX_EPOCH`。`fail::eval("NewWorkerPool", ...)` 保留 Go failpoint 的观测入口。
2. 调用方可在启动前通过 `SetTaskReceiver`、`SetResultSender` 注入通道。`Start` 断言只能调用一次；缺省任务通道为无缓冲通道，除 `R == None` 外缺省结果通道也为无缓冲通道。
3. `Start` 保存 operator context (`wctx`)，再创建 worker 子 context (`ctx`)，按当前 `numWorkers` 次数调用 `runAWorker`，最后设置 `started = true`。
4. `runAWorker` 先调用工厂；返回 `None` 时不创建线程。成功时，线程用 `select_biased!` 优先接收任务，其次观察任务通道关闭、缩容退出请求和上下文取消。收到任务后进入 `handleTaskWithRecover`；任一退出条件成立后调用一次 `Worker::Close`。
5. `handleTaskWithRecover` 用 `RunningGuard` 包围处理过程，从任务读取恢复元数据，以 `catch_unwind(AssertUnwindSafe(...))` 捕获 `HandleTask` panic。正常结果可经 `send_or_cancel` 发往结果通道；业务错误或 panic 均调用 operator context 的 `OnError`，从而记录首错并取消流水线。
6. 扩容时，`Tune` 直接补建 `difference` 个 worker；缩容时，为每个待移除 worker 经无缓冲 quit 通道发送同一个 `Countdown`。即使 `wait=false`，发送本身仍要等某个空闲 worker 接受退出请求；`wait=true` 还会继续等这些 worker 的 `Close` 完成。
7. 正常生产结束应先关闭任务通道或取消 context，再调用 `Release`。它 join 所有已登记线程、取消 worker context 并关闭/移走结果通道，使结果消费者看到结束。`CloseAndWait` 先取消再调用 `Release`，适合主动停止。

## 数据与状态

`WorkerPool` 同时维护“配置状态”和“实际资源”，两者不能混为一谈：

- `numWorkers`/`Cap()` 是目标配置容量；`originWorkers` 永远保留构造时经钳制后的值。可失败工厂返回 `None` 时，实际线程数可能小于 `Cap()`。
- `runningTask`/`Running()` 只统计正在执行 `HandleTask` 的任务，不统计存活但空闲的 worker，也不统计正在执行 `Close` 的 worker。
- `workers` 保存所有成功 spawn 的 `JoinHandle`，包括已因缩容退出的线程；句柄直到 `Release`、`Drop` 才被取出和 join，因此其长度不等于当前容量。
- `started` 只由需要 `&mut self` 的方法访问；本实现依靠 Rust 独占借用和上层加锁来串行修改池，而不像 Go 版本在池内用 `RWMutex`/atomic 保护并发方法调用。
- `lastTuneTs` 每次 `Tune` 都更新，包括启动前调容和钳制后的调容；初始值是 `UNIX_EPOCH`。
- `Context::first_error` 受 `Mutex` 保护，仅当为空时写入；后续错误仍触发取消和日志，但不会覆盖 `OperatorErr()`。
- 父 `ContextInner::children` 只保存 `Weak`，不会延长子 context 生命周期；取消时升级仍存活的弱引用、清理失效项，然后递归取消子树。

`Channel<T>` 的 `closed` 只描述显式 `close()`；数据收发端仍由共享的 `Arc` 持有。为模拟 Go 的共享关闭事件，它另外维护一个零容量 close 通道，丢弃唯一 `close_sender` 会唤醒所有克隆的接收端。`recv`/`recv_timeout` 先 `try_recv`，因此缓冲数据在关闭信号之前被排空。

## 依赖与调用关系

下游依赖如下：

- `crossbeam_channel::{Sender, Receiver, select!, select_biased!}`：数据、取消和缩容信号的多路选择；`after` 支持带超时接收。
- `std::sync::{Arc, Weak, Mutex, Condvar}` 与原子类型：共享状态、父子 context、倒计时等待和计数。
- `std::thread::{Builder, JoinHandle}`：每个 worker 对应一个命名 OS 线程。
- `std::panic::{catch_unwind, AssertUnwindSafe}`：只包围 `Worker::HandleTask` 的任务级恢复边界。
- `fail::eval`：保留 `NewWorkerPool` failpoint 接线；`log`：报告 worker 错误与调容事件。

上游的标准接线顺序是“构造池 → 可选注入通道 → `Start` → 生产任务/消费结果 → 关闭任务通道或取消 → `Release`”。例如 `globalsort/merge.rs` 和 `ingestctrl/import_pipeline.rs` 都创建外部任务通道、调用 `SetTaskReceiver`，启动后取得结果通道，并把池放入 `Arc<Mutex<_>>` 以安全调容。`session/runtime/modify_column_pipeline.rs` 展示了 `None` 结果池和带确认结果池组合使用的方式。

RustCodeGraph 的 `query NewWorkerPool` 能定位本文件构造函数及 Go 对照函数；目标文件节点还能报告文件级使用者。精确 `callers`/`callees` 对该 Rust 关联函数没有生成边，因此上述函数级接线来自源码引用核对，而不是缺失图边的推断。

## 错误处理与边界

- `Context::OnError` 记录首个 `Error`、写错误日志并总是调用 `Cancel`。锁中毒使用 `unwrap`，会 panic；这是当前实现的明确边界。
- `HandleTask` 返回 `Err` 与其 panic 都会转成 operator error。panic 的默认文本为 `task panic: {label}, func info: {func_info}`；若 `RecoverArgs` 提供错误，则直接使用它。
- `Worker::Close` 返回错误时也写入 operator context。线程在任务恢复边界之外 panic 时，`Release` 从失败的 `join` 合成 `worker thread panicked outside task recovery`；`Drop` 只忽略该 join 错误，不能替代需要读取错误的显式 `Release`。
- `thread::Builder::spawn` 失败会以 `expect("failed to start worker thread")` panic；`Start` 重复调用也会 panic。
- `Channel::send`/`send_or_cancel` 用 `bool` 表示是否完成发送；关闭或取消获胜时返回 `false`，不返还未发送的值。`recv_timeout` 用 `Ok(None)` 表示“已关闭且无剩余数据”，用 `Err(Timeout)` 表示“仍打开但本次超时”。
- `Release` 本身不会先关闭任务通道。若任务通道仍打开、context 也未取消，worker 会继续等待，`join` 会阻塞；调用方必须先满足退出条件，或改用先取消的 `CloseAndWait`。
- `Tune` 将非正目标钳制为 1。缩容时若 context 已取消或 quit 发送失败，会提前停止发送，但最终仍把 `numWorkers` 设为目标值；此时 `Cap()` 是配置值，不是已证明的活跃线程计数。
- `None` 判定依赖 `TypeId`，因此任务与结果类型都要求 `'static`；所有 worker 和任务还必须满足跨线程的 `Send` 约束。

## 并发与资源生命周期

`ContextInner::cancel` 用 `AtomicBool::swap(SeqCst)` 保证幂等；第一个调用者丢弃 done sender 并递归取消存活子节点，后续调用直接返回。`Channel::close` 使用同样的原子交换模式。这里选择 `SeqCst`，避免取消、关闭与观察者之间需要额外推导更弱内存序。

每个成功创建的 worker 由一个线程独占，因此 `Worker` 只要求 `Send` 而不要求 `Sync`。工厂被多个启动/扩容调用共享，所以包装为 `Arc<dyn Fn() -> Option<Box<dyn Worker<...>>> + Send + Sync>`。任务和结果通过 MPMC 通道跨线程移动。

任务接收使用 `select_biased!`，把已经就绪的数据分支放在显式关闭分支之前，以满足“关闭后排空缓冲任务”的不变量；`workerpool_test.rs::closed_task_channel_is_drained_before_worker_exits` 对此做回归验证。结果发送同时监听 worker context，因此没有结果消费者时，取消仍能解除阻塞；`migration_aster_unit_test.rs::cancellation_unblocks_a_worker_waiting_to_send_result` 覆盖该路径。

缩容 quit 通道是零容量通道，把“退出请求被某 worker 接受”和“该 worker 完成 `Close`”分成两个同步点。`wait=false` 只跳过第二个等待，不能跳过第一个握手；`workerpool_test.rs::tune_without_close_wait_still_waits_for_worker_to_accept_exit_request` 固化了这一 Go 兼容语义。

正常资源终点是 `Release`：所有线程完成后关闭结果通道。`Drop` 是兜底路径，会取消、join 并关闭结果通道，但不报告 join 错误。外部注入的任务通道只是克隆句柄，池不会替调用方关闭它；结果通道则在释放时由池调用共享的幂等 `close()`。

## 与 Go 版本的对应关系

- Go 原生 channel 对应 Rust `Channel<T>`。Rust 必须增加显式 `closed` 和独立 close 信号，才能在句柄被多方克隆时表达 Go 的共享 close 语义。
- Go `util.Recover` 对应 `catch_unwind`。Rust 只捕获 `HandleTask` 内 panic，恢复错误仍来自 `TaskMayPanic::RecoverArgs`。
- Go 工厂可返回 nil；Rust 普通构造器的工厂必须返回 worker，另由 `NewWorkerPoolWithFallibleFactory` 显式表达 `Option<W>` 分支。
- Go 用池内 `mu`、atomic `started`/时间支持共享指针上的并发访问；Rust 调容和生命周期方法要求 `&mut self`，需要共享时由调用者使用 `Mutex`，如 `RunningPool` 和 globalsort 的 `running_pool`。
- Go error 可携带任意具体错误和包装链；本文件的 `Error` 只保留共享字符串。Rust 的 `AddTask` 还返回发送是否成功，Go 版本则无返回值。
- Rust `Start` 明确断言只启动一次，且 `Drop` 提供兜底清理；Go 版本没有等价的析构机制。

Go 测试 `workpool_test.go` 的任务累加、扩缩容、`None` 结果、自定义通道和取消场景在 `workpool_test.rs` 中有同名迁移用例；Rust 另以 `migration_aster_unit_test.rs` 和 `workerpool_test.rs` 覆盖首错/父子取消、panic、关闭排空、接收超时、可失败工厂相关路径和缩容握手细节。

## 扩展指南

- 新增构造选项：实现 `PoolOption<T, R>` 并通过 `NewWorkerPoolWithOptions` 应用；若要改变启动所需状态，应保持“仅在 `Start` 前设置”的约束，并在独立 `*_test.rs` 中验证选项顺序与默认值。
- 修改任务执行或恢复：集中改 `handleTaskWithRecover`，同步验证业务错误、预置 panic 错误、默认 panic 文本、`Running()` 归零和取消阻塞。不要把测试写进 `workerpool.rs`。
- 修改通道语义：必须同时审查 `send`、`send_or_cancel`、`recv`、`recv_timeout`、`close`，维持关闭幂等、缓冲排空以及关闭/取消解除阻塞。重点同步 `workerpool_test.rs`。
- 修改调容：以 `Tune`、`runAWorker`、`Countdown` 和零容量 quit 通道为一个整体；尤其不能把 `wait=false` 误改成异步丢弃退出请求。同步 Go `workerpool.go` 的契约并扩展 `workpool_test.rs` 或 `migration_aster_unit_test.rs`。
- 新增生命周期 API：明确它是“等待自然退出”还是“主动取消后等待”，避免模糊 `Release` 与 `CloseAndWait` 的区别；还要决定线程外 panic 是否通过 `OperatorErr` 暴露。
- 增加生产消费者时，优先遵循现有的外部任务通道模式，并确保每条路径最终关闭输入或取消 context、持续消费结果直到关闭。否则无缓冲发送或 `Release` 可能永久等待。
- 兼容风险主要是 Go/Rust 取消与 close 的竞态次序；性能风险主要是每 worker 一个 OS 线程、全局顺序一致原子操作、每次调容保留旧 `JoinHandle`，以及无缓冲通道产生的同步等待。
