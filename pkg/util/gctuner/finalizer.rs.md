# `pkg/util/gctuner/finalizer.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-util-gctuner`，由 `pkg/util/gctuner/lib.rs` 以 `pub mod finalizer` 暴露。它不是数据库请求链上的独立服务，而是 GC 调谐基础设施：为 `tuner.rs` 的 GOGC 调谐和 `memory_limit_tuner.rs` 的运行时内存上限调谐提供可停止、可重复触发的周期回调。

Rust 没有 Go `runtime.SetFinalizer` 对应的 tracing-GC 钩子，因此本文件不是逐句复刻，而是在保持“每个回收边界通知一次、通知后继续武装、停止后不再通知”的可观察契约下提供运行时适配。其下游真实回收边界由 `crate::mem::releaseUnusedMemory` 提供。

## 核心职责

- `newFinalizer` 创建共享的 `Arc<Finalizer>`，保存可变回调并立即启动一个后台驱动线程。
- `Finalizer::startRuntimeDriver` 每隔 `RUNTIME_COLLECTION_INTERVAL`（300 ms）请求系统分配器归还空闲页，再执行一次回调；对象停止或全部强引用释放后退出。
- `Finalizer::run` 提供同一套回调执行边界，供后台线程、测试以及上层的显式调谐入口复用。
- `Finalizer::stop` 原子地封锁后续执行并唤醒等待线程，使其尽快退出；`isStopped` 提供状态观测。
- 回调由互斥锁串行化，维持 Go 版本所依赖的“finalizer 回调不并发执行”性质。

## 主要符号

- `RUNTIME_COLLECTION_INTERVAL: Duration`：后台轮询周期，固定为 300 ms；它是 Rust 适配新增的调度参数，Go 版本没有等价常量。
- `finalizerCallback = Box<dyn FnMut() + Send + 'static>`：允许回调跨线程、持有可变捕获状态，并由 finalizer 长期保存。`'static` 保证后台线程不会引用短生命周期数据。
- `Finalizer`：核心状态对象。`callback: Mutex<finalizerCallback>` 串行保护回调；`stopped: AtomicBool` 是终止闸门；`wake_lock` 与 `wake: Condvar` 让后台线程可超时等待并能被 `stop` 提前唤醒。
- `finalizer = Finalizer`：保留 Go 风格命名的公开类型别名，便于迁移代码对照。
- `newFinalizer(callback) -> Arc<Finalizer>`：公开构造入口。创建状态、调用私有 `startRuntimeDriver`，再把强引用交给调用者。
- `Finalizer::startRuntimeDriver(&Arc<Self>)`：私有线程启动函数。线程只捕获 `Weak<Finalizer>`，不因自身存在而永久延长对象生命周期。
- `Finalizer::run(&self) -> bool`：未停止且回调确实执行时返回 `true`；已停止时返回 `false`。
- `Finalizer::stop(&self)` / `isStopped(&self) -> bool`：分别发布停止状态并唤醒线程、读取停止状态。

## 执行流程

1. `tuner::newTuner`、`MemoryLimitTuner::withResetInterval` 或 `MemoryLimitTuner::Start` 把一个回调交给 `newFinalizer`；这些回调通过 `Weak` 回到所属调谐器，避免形成 `调谐器 -> finalizer -> 回调 -> 调谐器` 的强引用环。
2. `newFinalizer` 初始化 `stopped=false`、回调锁和条件变量，然后调用 `startRuntimeDriver`。
3. 后台线程在每轮开头升级 `Weak`。升级失败说明调用者已释放全部强引用，线程立即返回。
4. 线程持有 `wake_lock`，通过 `Condvar::wait_timeout_while` 等待至 300 ms 超时或 `stopped` 变为真。`stop` 会 `notify_all`，所以正常停止不必等待完整周期。
5. 若已经停止，线程退出；若确为超时，则先调用 `releaseUnusedMemory`，再调用 `run`。当前代码不根据 `releaseUnusedMemory` 的布尔返回值决定是否回调。
6. `run` 在取得回调锁前检查一次 `stopped`，取得锁后再次检查；两次均未停止才调用 `FnMut` 并返回 `true`。回调完成后对象保持 armed，下一周期可再次执行。
7. 上层也可经 `Tuner::runFinalizer` 或 `MemoryLimitTuner::runFinalizer` 显式触发相同流程；这些入口主要由独立测试使用。

## 数据与状态

`Finalizer` 只有“运行中”和“已停止”两个单向状态；`stop` 不提供重新启动，且重复调用是幂等的。需要重新启动的 `MemoryLimitTuner::Start` 会新建一个 finalizer、停止旧实例并替换保存的 `Arc`，而不是复位旧对象。

`stopped` 的所有读写都使用 `Ordering::SeqCst`，形成单一的全序状态观测。`callback` 必须使用 `Mutex`，因为类型为 `FnMut`，且后台触发和显式 `run` 可能竞争；同一实例任意时刻最多有一个回调在执行。`wake_lock` 不保护业务状态，只服务于条件变量等待协议。

后台线程不保存 `Arc` 跨越整个对象生命期：每轮通过 `Weak::upgrade` 临时取得强引用。线程在等待与当轮回调期间会暂时延长对象生命期，随后进入下一轮再判断是否仍有外部所有者。

## 依赖与调用关系

直接依赖只有 Rust 标准库的原子、`Arc`/`Weak`、`Mutex`、`Condvar`、线程和时间类型，以及同 crate 的 `mem::releaseUnusedMemory`。后者在 macOS 调用 `malloc_zone_pressure_relief`，在 glibc Linux 调用 `malloc_trim(0)`，其他目标返回 `false`；本文件在所有目标上仍继续通知回调。

主要调用边如下：

- `tuner.rs::newTuner -> newFinalizer -> 回调中的 Tuner::tuning`；`Tuner::stop` 和 `Tuner::runFinalizer` 分别下接 `Finalizer::stop`、`Finalizer::run`。
- `memory_limit_tuner.rs::withResetInterval/Start -> newFinalizer -> 回调中的 MemoryLimitTuner::tuning`；`Stop` 与 `runFinalizer` 下接对应 finalizer 方法。
- `startRuntimeDriver -> releaseUnusedMemory -> Finalizer::run -> finalizerCallback`。
- `finalizer_test.rs::test_finalizer` 与 `migration_aster_unit_test.rs::migration_finalizer_rearms_until_stopped` 验证显式执行链；`finalizer_runs_without_manual_notification` 验证后台自动链。

`Cargo.toml` 指定 crate 根为 `lib.rs`、关闭自动测试发现和 doctest，并声明 `task-memory`、`task-util` 两个包级依赖；本文件本身不直接引用它们。测试通过 `lib.rs` 中显式的 `#[cfg(test)] mod finalizer_test` 和迁移聚合模块接入。

## 错误处理与边界

本 API 不返回 `Result`。正常的停止边界以 `run == false` 表达；分配器不支持主动归还时，`releaseUnusedMemory == false` 被有意忽略，回调仍会运行。

两个互斥锁若中毒，代码分别以 `finalizer wake lock poisoned` 或 `finalizer callback lock poisoned` 调用 `expect`，导致当前调用线程 panic。后台线程 panic 不会通过句柄传播，因为 `thread::spawn` 的 `JoinHandle` 未保存；显式 `run` 中的 panic 会直接传播给调用者。回调自身 panic 同样会使 `callback` 锁中毒，并终止触发它的线程。

双重 `stopped` 检查缩小了 `stop` 与尚未取得回调锁的 `run` 之间的竞态窗口，但它不是对已经开始执行的回调的取消：若 `run` 完成第二次检查后 `stop` 才发生，本次回调仍会完成。`stop` 也不等待正在执行的回调结束，因此调用者不能把它当作同步 join 屏障。

## 并发与资源生命周期

每次 `newFinalizer` 都创建一个未命名、未保存 `JoinHandle` 的独立系统线程。线程终止条件有两个：`stop` 发布停止状态并唤醒条件变量，或下一轮 `Weak::upgrade` 发现对象已无强引用。由于没有 join，`stop` 只发出终止信号，调用者无法从本 API 等待线程退出。

回调锁覆盖整个用户回调，保证多个后台/显式触发者串行，但也意味着回调不得递归调用同一实例的 `run`，否则会等待自己持有的非重入锁；耗时回调也会阻塞其他触发者。回调执行期间调用 `stop` 不需要取得回调锁，因此不会因该锁阻塞，但不能撤销已进入的回调。

条件变量的谓词在未停止时保持等待；虚假唤醒不会触发回调，只有 `timeout.timed_out()` 才进入回收与通知。`SeqCst` 原子状态、条件变量通知和回调互斥共同定义了可观察次序。

## 与 Go 版本的对应关系

Go `finalizer.go` 的 `newFinalizer` 创建 `finalizerRef`，用 `runtime.SetFinalizer` 注册 `finalizerHandler`，再清空持有引用来让每次 GC 触发 handler；handler 执行回调后再次 `SetFinalizer`，形成重挂。`stop` 通过原子整数令后续 handler 直接返回。Go 测试以多次 `runtime.GC()` 验证每次 GC 通知以及停止后的计数冻结。

Rust 保留了重复通知、串行回调和停止闸门，但触发语义不是“语言运行时完成一次 tracing GC”，而是固定 300 ms 定时器先请求系统分配器归还空闲页、再通知回调。Rust 新增了显式 `run`、`isStopped`、条件变量唤醒和 `Weak` 线程生命周期控制；Go 的 `finalizerRef`、`runtime.SetFinalizer` 与引用清空在 Rust 中不存在。

因此，扩展或排障时不能假设一次 Rust 回调严格对应一次 Rust 语言级 GC，也不能把 300 ms 当成 Go runtime 的保证。当前测试验证的是适配后的行为契约，而非触发时刻与 Go 完全一致。

## 扩展指南

- 改变调度周期、触发源或分配器回收策略时，首先修改 `RUNTIME_COLLECTION_INTERVAL` / `startRuntimeDriver`，并同步 `finalizer_test.rs::finalizer_runs_without_manual_notification`；还应检查 `memory_limit_tuner_test.rs::memory_limit_tuner_runs_without_manual_finalizer` 对自动调谐时序的要求。
- 改变停止或重挂语义时，修改 `run`、`stop`，并同步 `finalizer_test.rs::test_finalizer` 与 `migration_aster_unit_test.rs::migration_finalizer_rearms_until_stopped`。测试逻辑应继续放在独立测试文件，不应嵌入本生产文件。
- 若需要等待线程退出，应引入明确的 join/完成通知所有权模型；仅保存 `JoinHandle` 而不处理回调中调用 `stop`、析构线程自等待等情形会引入死锁风险。
- 若放宽回调锁范围或允许并发，必须同时审计 `Tuner::tuning` 和 `MemoryLimitTuner::tuning` 的共享状态不变量；当前上层依赖串行通知。
- 若要传播分配器回收失败或回调失败，需要设计新的错误通道。直接改变 `run -> bool` 的含义会影响两个调谐器的 `runFinalizer` 及现有断言。
- 性能上，每个实例一条线程且 300 ms 唤醒一次；若实例数量增长，优先评估共享调度器，同时保持各实例的停止隔离、回调串行和弱引用释放语义。

## 验证依据

- 源码与符号：`pkg/util/gctuner/finalizer.rs` 的 `finalizerCallback`、`Finalizer`、`newFinalizer`、`startRuntimeDriver`、`run`、`stop`、`isStopped`；下游回收实现为 `pkg/util/gctuner/mem.rs::releaseUnusedMemory`。
- crate 边界：`pkg/util/gctuner/Cargo.toml` 与 `pkg/util/gctuner/lib.rs`；前者确认包名、crate 根、依赖和 `autotests = false`，后者确认模块与独立测试接线。
- 上游调用：`pkg/util/gctuner/tuner.rs::{newTuner,Tuner::stop,Tuner::runFinalizer}` 和 `pkg/util/gctuner/memory_limit_tuner.rs::{withResetInterval,Start,Stop,runFinalizer}`。
- Go 对照：`pkg/util/gctuner/finalizer.go`、`finalizer_test.go`，以及调谐接线 `tuner.go::newTuner`、`memory_limit_tuner.go::Start/Stop`。
- Rust 测试：`pkg/util/gctuner/finalizer_test.rs::{test_finalizer,finalizer_runs_without_manual_notification}`、`migration_aster_unit_test.rs::migration_finalizer_rearms_until_stopped`；相关上层显式入口还由 `tuner_test.rs`、`memory_limit_tuner_test.rs` 使用。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；文件节点确认 `finalizer.rs` 共 117 行，并被 `finalizer_test.rs`、`memory_limit_tuner.rs`、`migration_aster_unit_test.rs`、`tuner.rs` 使用；精确 `query newFinalizer` 同时定位 Go/Rust 构造函数，`query Finalizer` 定位结构体、方法、测试和两个 `runFinalizer`。精确 `callers/callees` 查询在本地无输出并于约 30 秒超时，因此调用边又以索引文件节点和上述调用点源码交叉核验，未把超时解释为“没有调用边”。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令验证，并人工检查文档未把定时回调描述成 Go runtime GC 的精确等价物。
