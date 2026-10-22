# `pkg/ddl/ddl_workerpool.rs`

## 文件定位

Go `pkg/ddl/ddl_workerpool.go` 的 Rust 适配层：表达 General 与 Reorg 两类 DDL worker 池的非阻塞借还、容量限制和关闭语义。

## 核心职责

- `JobType` 标记整个池服务于普通 DDL 还是数据重组 DDL。
- `Worker` 保留可观测的 worker 编号和池类型，作为池中被借还的资源值。
- `WorkerPool` 在创建时预分配固定数量的 worker，通过 FIFO 队列进行非阻塞借出与归还，并跟踪空闲数和已借出数。
- `Arc<Mutex<PoolState>>` 让 `WorkerPool::clone` 得到共享同一状态的句柄，并串行化对队列、关闭标志和计数器的修改。
- `close` 是幂等的逻辑关闭：禁止新借出、清空当前空闲队列，并使之后归还的 worker 不再回到队列。

## 主要符号

- `pub enum JobType { General, Reorg }`：公开的二分作业类型。它只是本池的分类标签，与 `pkg/ddl/job_scheduler.rs::JobType` 是两个不同的 Rust 类型。
- `pub struct Worker { pub id: usize, pub job_type: JobType }`：公开资源值；本文件不为它定义任务执行方法。
- `pub struct WorkerPool`：公开池句柄。`job_type` 在构造后不变，`state` 是被所有 clone 共享的内部可变状态。
- `struct PoolState { closed, available, borrowed }`：私有状态。`available` 是 FIFO 队列，`borrowed` 是池对借还操作的计数，而不是一个按 worker ID 维护的集合。
- `WorkerPool::new(job_type, size) -> Self`：创建 ID 为 `0..size` 的 worker；`size == 0` 是有效输入，会产生打开但始终无可用 worker 的池。
- `WorkerPool::get(&self) -> Result<Option<Worker>, String>`：关闭时返回 `Err("workerPool is closed")`，耗尽时返回 `Ok(None)`，成功时返回 `Ok(Some(worker))` 并增加 `borrowed`。
- `WorkerPool::put(&self, worker)`：将 `borrowed` 饱和减一；未关闭时把传入值加到队尾，已关闭时丢弃。
- `WorkerPool::close(&self)`：幂等设置 `closed` 并清空 `available`；不等待 `borrowed` 归零。
- `job_type`、`available`、`borrowed`：公开观测方法，后两者分别在短持锁期间读取队列长度和计数。

## 执行流程

1. 调用方用 `WorkerPool::new(JobType, size)` 建池。构造器为每个 `id` 创建同一 `job_type` 的 `Worker`，初始状态为 `closed = false`、`borrowed = 0`。
2. `get` 获取互斥锁。若已关闭则返回错误；否则从 `available.pop_front()` 取队首。
3. 若队首存在，`borrowed += 1` 并将 worker 交给调用方；队列为空时立即返回 `None`，没有等待、通知或超时路径。
4. 调用方完成使用后必须调用 `put`。`put` 先用 `saturating_sub(1)` 更新计数；池仍打开时将 worker 加入队尾，保持循环复用。
5. 任一共享句柄调用 `close` 都会把共享状态关闭。关闭前已借出的 worker 可继续存活，但归还时被丢弃；再次 `get` 得到固定错误。

上述流程由 `pkg/ddl/ddl_workerpool_test.rs::test_ddl_worker_pool` 直接覆盖：容量为 1 的池在借出后耗尽，归还后恢复，关闭后拒绝借出并丢弃归还，重复关闭不改变状态。

## 数据与状态

池的逻辑容量由 `new` 创建的 worker 数量决定，但实现未单独保存 `size` 或 worker ID 集合。正常配对使用时，打开池满足 `available.len() + borrowed == size`。这一关系是调用协议的结果，不是由类型系统或运行时校验强制：`Worker` 字段公开，`put` 也不验证 worker 来源、ID、`job_type` 或是否重复归还。

`borrowed` 仅表示本池观测到的成功 `get` 减去 `put` 次数。`saturating_sub` 防止无匹配 `put` 导致无符号下溢，但这种误用仍可向队列注入额外 worker，使实际可用数超过初始容量。

`close` 之后 `available` 恒为空（因为 `put` 不再入队），但 `borrowed` 可在关闭时仍大于零，并随后续归还递减。关闭不是“已借出数立即清零”；测试中关闭时计数为零，没有覆盖带有未归还 worker 的关闭。

## 依赖与调用关系

- 下游依赖：`WorkerPool::new/get/put/close/available/borrowed` 只调用 `std::collections::VecDeque` 和 `std::sync::{Arc, Mutex}` 的操作；无 I/O、数据库、任务调度器或异步运行时依赖。
- 装配边：`pkg/ddl/lib.rs:55` 公开此模块；`pkg/ddl/lib.rs:190-191` 在 `cfg(test)` 下装配独立测试文件。
- 已验证的 Rust 上游：`pkg/ddl/ddl_workerpool_test.rs` 导入 `JobType`、`Worker` 和 `WorkerPool`，是仓库内唯一直接使用者。
- 未存在的生产边：`pkg/ddl/job_scheduler.rs` 的调度路径在 `schedule` 中直接根据其自有 `ScheduledJob.job_type` 选择 `general_worker` 或 `reorg_worker`，不借还 `ddl_workerpool::Worker`。因此不能将 Go `loadAndDeliverJobs -> workerPool.get -> deliveryJob -> workerPool.put` 调用边归于当前 Rust 代码。
- Go 生产对照边：`pkg/ddl/job_scheduler.go::start` 创建 reorg/general/可选 background 池；`loadAndDeliverJobs` 用 `available` 预判并调用 `get`；`deliveryJob` 在 goroutine 的 defer 中 `put`；`jobScheduler.close` 关闭各池；`workerPoolExhausted` 用可用数控制本轮加载。

## 错误处理与边界

`get` 的显式业务错误只有“池已关闭”，用 `String` 承载，没有专用错误类型或可区分的错误枚举。资源耗尽是正常控制流 `Ok(None)`，调用方必须与关闭错误分开处理。

所有锁操作均使用 `lock().unwrap()`。如果某个持锁线程 panic 导致 mutex poisoned，后续 `get`、`put`、`close`、`available` 或 `borrowed` 会在 `unwrap` 处 panic，而不是返回 `Result`。`put` 没有返回值，因此已关闭时的丢弃是静默行为。

其他需由调用方遵守的边界包括：每次成功借出应恰好归还一次；不应手工构造或跨池归还 `Worker`；不应依赖关闭等待在途 worker；不应把 `None` 解释成执行失败。

## 并发与资源生命周期

`WorkerPool` 的 clone 共享 `Arc<Mutex<PoolState>>`，所以关闭与队列变化对所有句柄立即可见。每个公开状态操作在一次 mutex 临界区内完成，因此 `get` 的“检查关闭—弹出—计数”和 `put` 的“计数—检查关闭—入队/丢弃”各自具有原子性。

实现没有条件变量或通道：当池耗尽时，借用者不会被排队或唤醒，必须在外部安排重试。锁只保护池账本，`Worker` 离开池后不持锁，该值本身也没有运行任务或需要 join 的线程。

`close` 是逻辑销毁而不是阻塞清算：它在持锁期间清空空闲资源后立即返回，不等待已借出值；已借出值的真正生命期受所有者控制，其后在 `put` 入参移动并被丢弃时结束。`WorkerPool` 未实现 `Drop`；最后一个句柄被 drop 时，共享队列中的 worker 随 `Arc` 内部状态一起释放。

## 与 Go 版本的对应关系

实现差异必须保留：

- Go 池包装 `ngaut/pools.ResourcePool`，worker 由 factory 懒创建且是能执行 DDL 的 `*worker`；Rust 在 `new` 时预建轻量 `Worker { id, job_type }`，它不是 `pkg/ddl/job_worker.rs::JobWorker`。
- Go `get` 还传播 `ResourcePool.TryGet` 错误；Rust 内部弹队不产生可返回错误，除关闭外的异常是 mutex poison panic。
- Go `put` 在 `exit` 已设置时直接返回，未关闭时把资源交还底层池；Rust 无论是否关闭都先饱和递减 `borrowed`，然后决定是否入队。
- Go `ResourcePool.Close` 的生命周期与资源返还由外部库定义，Go 源码注释特别说明关闭期间需归还 context；Rust `close` 自身不等待、不通知且不调用 worker 级关闭。
- Rust 额外提供 `borrowed()` 和公开 `Worker` 字段，Go 对照对象没有同等 API。
- 最重要的迁移差异是接线状态：Go 池已被 `jobScheduler` 生产使用，Rust 池目前只由独立单元测试直接使用，不应声称它已实现 Go 调度器的并发限制效果。

Rust `test_ddl_worker_pool` 覆盖了这一意图，并额外覆盖类型/ID、借出计数、耗尽时 `None`、关闭错误和幂等关闭。

## 扩展指南

- 修改借还或关闭语义时，主要接入点是 `WorkerPool::{get, put, close}` 和私有 `PoolState`；应同步扩展独立的 `pkg/ddl/ddl_workerpool_test.rs`，不要把测试内嵌到生产文件。
- 若要防止伪造、跨池或重复归还，需从 `Worker` 公开构造能力、池身份令牌和 `PoolState` 的在途集合一并设计；只修改 `borrowed` 计数无法强制容量不变式。
- 若增加阻塞获取、超时或取消，需明确选择 `Condvar`、channel 或异步原语，并测试“归还唤醒”、“关闭唤醒”和竞态顺序；当前 `get` 的非阻塞契约不可无意改变。
- 若要把本池接入 Rust DDL 生产调度，不能只替换一个类型：需先解决 `ddl_workerpool::Worker` 与 `job_worker::JobWorker`、两个 `JobType`、调度器的任务所有权及关闭顺序的差异。验证面至少应包含 worker 耗尽后不过量调度、General/Reorg 隔离、任务结束/异常后必然归还，以及 owner 退出时的在途资源处理。
- 保持 Go 对齐时，应对照 `pkg/ddl/ddl_workerpool.go`、`pkg/ddl/job_scheduler.go` 和 `pkg/ddl/ddl_workerpool_test.go`；但必须把 Go 外部 `ResourcePool` 的行为与当前 Rust 自管队列的实际行为分开记录。
- 涉及并发或关闭的新测试应尽量使用可控同步点，避免靠 sleep 推测时序；兼容性风险主要是错误形态和非阻塞契约，性能风险主要是单一 mutex 在高频借还下的争用。
