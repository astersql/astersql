# `pkg/ttl/ttlworker/worker.rs`

## 文件定位

本文件属于 Cargo crate `astersql-ttl-ttlworker`，由同目录 `lib.rs` 的 `pub mod worker` 公开。它实现 TTL 后台 worker 的通用生命周期原语：一次性启动一个线程、向该线程同步投递类型擦除消息、请求取消，以及等待线程停止。`Cargo.toml` 将该 crate 对应到 Go 包 `pkg/ttl/ttlworker`；本文件本身只依赖 Rust 标准库，不使用 crate 的业务依赖。

当前 Rust 接线边界必须特别说明：仓库搜索显示 `BaseWorker`、`WorkerStatus`、`WorkerError`、`CancellationToken` 和 `WorkerMessage` 在生产 Rust 文件中尚无调用者，只有独立测试 `worker_test.rs` 使用 `BaseWorker` 与 `Worker`。因此它是已经实现并导出的迁移基础设施，但还不是 Rust TTL 扫描、删除或作业管理主链的公共基类。Go 对照 `worker.go` 则已被 `scan.go`、`del.go`、`job_manager.go` 等生产代码嵌入和调用。

## 核心职责

- `BaseWorker::new` 建立共享状态、取消令牌和零容量 `sync_channel(0)`，保存只能消费一次的循环闭包与接收端。
- `Worker` trait 统一暴露 `start`、`stop`、`status`、`error`、`send`、`wait_stopped` 六项能力。
- `start` 保证只从 `Created` 启动一次，将循环放入独立 OS 线程，并把正常结束、业务错误和 panic 汇总为最终状态。
- `stop` 只负责发出取消信号和推进状态，不强制终止线程；循环闭包必须观察 `CancellationToken` 或以其他方式自行退出。
- `send` 保留 Go 无缓冲 `chan any` 的会合/背压语义；发送者会等待接收者实际接收消息。
- `wait_stopped` 用条件变量等待最终状态，并区分成功、调用方取消和超时。

## 主要符号

- `WorkerStatus::{Created, Running, Stopping, Stopped}`：完整生命周期状态；默认值为 `Created`。
- `WorkerError::{Loop(String), Panic, Timeout, Canceled, ChannelClosed}`：分别表示循环业务失败、panic、等待超时、外部等待上下文取消和消息通道关闭。`Loop` 的字符串内容由循环闭包提供。
- `CancellationToken(Arc<AtomicBool>)`：可克隆取消标志。`cancel` 使用 `Release` 写，`is_canceled` 使用 `Acquire` 读；所有克隆观察同一标志。
- `WorkerMessage = Box<dyn Any + Send>`：跨线程消息的类型擦除边界。接收方需自行 `downcast`，本文件不规定消息协议。
- `LoopFunction`：接收 worker 自身取消令牌和消息接收端、只执行一次并返回 `Result<(), WorkerError>` 的闭包。
- `WorkerState`：锁内状态，保存 `status`、最终 `error` 和待回收的 `JoinHandle<()>`。
- `WorkerInner`：由 `Arc` 共享，组合状态锁、`Condvar`、取消令牌、同步通道两端和待启动闭包；各 `Mutex<Option<_>>` 同时表达互斥访问与“一次取走”所有权。
- `BaseWorker`：可克隆的 `Arc<WorkerInner>` 句柄；克隆不创建新线程或新通道。
- `BaseWorker::state`：锁中毒时通过 `into_inner` 恢复数据，避免此前 panic 永久破坏生命周期查询。
- `BaseWorker::transition_to_stopped`：线程退出的统一收尾点，写入最终错误、关闭发送端并 `notify_all`。
- `BaseWorker::cancellation_token`：暴露 worker 自身令牌的克隆，供外部观察同一取消状态。

## 执行流程

1. 构造：`BaseWorker::new` 创建零容量同步通道，将状态初始化为 `Created`，并把闭包、发送端和接收端分别存入可取走的槽位。
2. 启动：`start` 先持有状态锁；非 `Created` 直接返回。它依次取走闭包和接收端，将状态改为 `Running`，再启动线程并保存 `JoinHandle`。
3. 循环：新线程以 worker 自身的取消令牌和唯一接收端调用闭包。`catch_unwind(AssertUnwindSafe(...))` 将 unwind 转成 `WorkerError::Panic`；闭包的 `Err` 原样成为最终错误，`Ok(())` 不记录错误。
4. 收尾：无论闭包正常、报错还是 panic，线程都调用 `transition_to_stopped`，将状态设为 `Stopped`，保存错误，取走发送端使后续发送失败，并唤醒所有条件变量等待者。
5. 主动停止：若尚未启动，`stop` 立即取消、关闭发送端、置 `Stopped` 并通知等待者；若正在运行，则取消并置 `Stopping`，最终仍由循环线程收尾。重复停止是空操作。
6. 等待：`wait_stopped` 对已经停止的 worker 直接取走并 `join` 线程后成功；否则以最多 10 ms 的切片在条件变量上等待，每轮检查外部取消和绝对截止时间。观察到 `Stopped` 后同样只由一个等待者取走 `JoinHandle` 并回收线程。

## 数据与状态

允许的正常状态迁移是 `Created -> Running -> Stopped`、`Created -> Stopped`，或 `Created -> Running -> Stopping -> Stopped`。`start`、`stop` 和最终收尾都由 `WorkerState` 的互斥锁串行化；状态查询和错误读取返回锁内快照。循环闭包与通道接收端只能被 `start` 取走一次，线程句柄只能被第一个成功等待者取走一次。

`error` 仅在循环线程收尾时写入。未启动即停止的路径不写错误，初值仍为 `None`。进入 `Stopping` 不代表线程已经退出，也不会关闭发送端；只有 `transition_to_stopped` 才关闭运行中 worker 的发送端。零容量通道不保存消息，所以成功的 `send` 同时证明接收端已接收该消息，而非仅已排队。

## 依赖与调用关系

下游全部来自标准库：`Arc` 共享所有权，`Mutex`/`Condvar` 保护状态和等待，`AtomicBool` 传播取消，`mpsc::sync_channel(0)` 提供会合通道，`thread::spawn`/`JoinHandle` 管理线程，`catch_unwind` 隔离 panic，`Instant`/`Duration` 实现超时。

内部主要调用边为：`start -> thread::spawn -> loop_function -> transition_to_stopped`；`stop -> CancellationToken::cancel`；`send -> SyncSender::send`；`wait_stopped -> Condvar::wait_timeout`，成功时再调用 `JoinHandle::join`。RustCodeGraph 已索引本文件并能返回完整源码和符号，但对这些通用方法名执行 `callers/callees` 未返回静态边；因此跨文件使用关系另由仓库精确搜索核对。目前唯一 Rust 上游是 `worker_test.rs`，crate 入口是 `lib.rs` 的 `pub mod worker`。

## 错误处理与边界

- 循环返回的 `WorkerError` 被保存，可在停止后用 `error` 读取；panic 的载荷被丢弃，只保留 `Panic` 分类。
- `send` 在发送端已被取走或接收端断开时返回 `ChannelClosed`。它可能无限阻塞，因为 API 没有发送超时或外部取消参数。
- `wait_stopped` 在调用方令牌取消时返回 `Canceled`，到达期限时返回 `Timeout`。如果调用时 worker 已是 `Stopped`，即使调用方令牌也已取消，仍按 Go 语义返回成功。
- `stop` 是协作式取消：不读取令牌、阻塞在其他资源或忽略消息的循环可能永不结束，随后 `wait_stopped` 只能超时/取消。
- `Instant::now() + timeout` 采用标准库的时间加法；极端大到溢出的 `Duration` 未由本文件单独防护。
- 对 poison 的状态锁、发送端锁、接收端锁和闭包锁均恢复内部值；`Condvar::wait_timeout` 的 poison 结果也被恢复。`join` 返回值被忽略，因为循环 panic 已在内部捕获，但线程基础设施层面的异常不会再次覆盖已记录错误。
- `WorkerMessage` 的运行时类型完全由上下游约定；错误 downcast 的处理不在本文件职责内。

## 并发与资源生命周期

所有 `BaseWorker` 克隆共享同一个线程、状态、取消位和通道。状态锁覆盖启动与停止的关键迁移，避免并发 `start` 重复消费闭包；原子取消允许循环无锁轮询。`transition_to_stopped` 在持有状态锁期间关闭发送端并广播条件变量，因此等待者被唤醒后能观察到 `Stopped`。

零容量通道产生明确背压，但也带来死锁风险：在没有活跃接收者时同步调用 `send` 会阻塞；测试专门用另一个发送线程证明此行为。运行中 `stop` 不立即删除 sender，也不会解除已阻塞发送，是否退出取决于循环如何配合取消与接收。`wait_stopped` 的 10 ms 轮询切片用于弥补外部 `CancellationToken` 没有通知机制，代价是取消响应最多有一个切片量级的延迟。线程资源只在成功观察 `Stopped` 的 `wait_stopped` 调用中显式 `join`；若调用方从不等待，句柄随共享内部对象销毁时被 detach。

## 与 Go 版本的对应关系

`WorkerStatus` 对应 Go 的 `workerStatus` 四个常量；`Worker` 对应 Go 的 `worker` interface；`BaseWorker`/`WorkerInner`/`WorkerState` 合起来对应嵌入互斥锁、context、channel、loop、错误和 wait group 的 `baseWorker`。`BaseWorker::new` 对应 `baseWorker.init`，`transition_to_stopped` 对应 `toStopped`，Rust 的零容量 `sync_channel(0)` 明确保留 `make(chan any)` 的无缓冲语义。

Rust 用 `CancellationToken` 替代 Go `context.Context`，用 `Condvar` 和 `JoinHandle` 替代 `WaitGroupWrapper` 加派生 timeout context；因此 Rust 等待逻辑通过 10 ms 切片检查取消。Go panic 路径记录日志并触发 `intest.Assert`，但延迟收尾里的局部 `err` 通常仍为 `nil`；Rust 则显式记录 `WorkerError::Panic`，不在本层日志输出。Go 的 `Send()` 返回发送通道，Rust 的 `send` 直接执行一次发送并返回错误。

两版最重要的成熟度差异是接线：Go 的 `ttlScanWorker`、`ttlDeleteWorker` 和 `jobManager` 嵌入 `baseWorker`；Rust 当前没有对应生产调用者。因此不能仅凭本文件存在就断言 Rust TTL 后台任务已经统一使用该抽象。

## 扩展指南

- 新增生命周期状态或迁移时，应同时修改 `WorkerStatus`、`start`、`stop`、`transition_to_stopped` 和 `wait_stopped`，并在独立的 `worker_test.rs` 增加竞争与幂等测试；不要把测试内嵌到生产文件。
- 新增消息协议宜在具体 worker 模块定义枚举，并在闭包接收端集中 downcast；若所有消费者已统一类型，可评估把 `BaseWorker` 泛型化，但需兼顾 Go `any` 兼容性和现有公开 API。
- 若要让停止解除阻塞发送或让取消即时唤醒等待者，需要同时设计通道关闭顺序和通知机制；单纯缩短 10 ms 切片只会增加唤醒开销。
- 将该抽象接入 `scan.rs`、`del.rs` 或 `job_manager.rs` 时，应逐项对照 Go 的 `init`、循环取消检查、发送点和 `WaitStopped` 调用，并同步各自现有的独立 Rust 测试，而不是把 Go 的完整生产接线视为已迁移。
- 性能风险集中在每 worker 一个 OS 线程、频繁等待时的 10 ms 轮询，以及零容量通道的吞吐/阻塞；兼容风险集中在停止后发送、重复启停、panic 错误可见性和“已停止优先于外部取消”的返回顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库；`files --filter pkg/ttl/ttlworker` 确认 `worker.rs` 有 45 个符号；`node --file pkg/ttl/ttlworker/worker.rs --offset 1 --limit 500` 核对全部 308 行实现；`query BaseWorker`、`query transition_to_stopped`、`query wait_stopped` 核对主要符号；`callers/callees` 无可用精确边的限制已在正文披露。
- 源码与边界：`pkg/ttl/ttlworker/worker.rs`；模块入口 `pkg/ttl/ttlworker/lib.rs`；crate 清单 `pkg/ttl/ttlworker/Cargo.toml`。
- Rust 测试：`pkg/ttl/ttlworker/worker_test.rs` 证明同步发送在接收前阻塞，并覆盖启动、克隆句柄发送、循环结束与成功等待；当前未覆盖其他状态迁移、错误、panic、取消和超时分支。
- Go 对照：`pkg/ttl/ttlworker/worker.go`；生产接线由 `scan.go`、`del.go`、`job_manager.go` 中的 `baseWorker` 嵌入和 `init` 调用核对；相关行为还由 `scan_test.go`、`del_test.go`、`task_manager_test.go` 及集成测试中的启停/等待断言覆盖。
- 仓库精确搜索确认 Rust 生产文件没有这些符号的上游使用点，避免将 Go 调用图误归到 Rust。本文档任务不修改运行时代码，按计划不运行 Cargo。
