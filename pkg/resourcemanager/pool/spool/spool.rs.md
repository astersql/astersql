# `pkg/resourcemanager/pool/spool/spool.rs`

## 文件定位

本文件实现受实例级资源管理器管控的 `spool::Pool`：每次占到并发槽位后创建一个操作系统线程，不复用线程；既可执行单个闭包，也可让若干 worker 持续消费一个 `TaskChannel`。源码入口是 [`spool.rs`](spool.rs)，由同目录 [`lib.rs`](lib.rs) 的 `mod spool; pub use spool::*;` 导出；根门面 [`pkg/lib.rs`](../../../lib.rs) 又将整个 crate 暴露为 `resourcemanager::pool::spool`。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-resourcemanager-pool-spool`，库入口为 `lib.rs`。直接依赖包括 `prometheus`、`crossbeam-channel`，以及路径依赖 `astersql-resourcemanager`；其中通道类型实际由同 crate 映射进来的 `poolmanager` 模块提供。

构造成功的池会在进程级 `InstanceResourceManager` 中按名称和 `Component` 注册，并通过 `GoroutinePool` trait 参与资源管理器的容量调度。仓库搜索只发现根门面导出和本 crate 测试直接构造 Rust `Pool`，未发现测试之外的 Rust 或 Go 生产构造点；因此当前可确认的是“实现及调度接线已经存在”，不能据此宣称某条线上业务路径已经实例化该池。

## 核心职责

1. `Pool::new` 校验零容量、加载函数式选项、初始化指标与共享状态，并把池注册到 `InstanceResourceManager`（`spool.rs:126-169`）。
2. `Pool::run` 为一次性闭包申请一个槽位并创建线程；满载时依据 `Options::Blocking` 轮询等待或返回 `Error::Overload`（`spool.rs:201-212,265-299`）。
3. `Pool::run_with_concurrency` 按当前空闲槽位截断请求并发数，登记 `Meta`，再启动多个消费同一 `TaskChannel` 的 worker（`spool.rs:214-231`）。
4. `Pool::tune` 更新容量和 Prometheus gauge，并通过 `TaskManager::Overclock` / `Downclock` 为已登记的多并发任务增减 worker（`spool.rs:172-199`）。
5. `Pool::release_and_wait` 阻止新提交，等待正在准入的提交者离开，再 join 已记录的线程并从资源管理器注销（`spool.rs:301-317`）。
6. `GoroutinePool for Pool` 让资源管理器通过统一接口读取名称、容量、运行数、原始并发和调谐时间，并执行调容或释放（`spool.rs:348-376`）。

## 主要符号

- `WAIT_INTERVAL: Duration`：阻塞提交者在满载时每 5 ms 重试一次，不使用条件变量等待容量变化（`spool.rs:35-36`）。
- `Error::{InvalidParams, Closed, Overload, Registration}`：本文件的四类公开错误。`Display` 对前三类复用 `BasePool` 的 Go 兼容文案，注册错误固定显示 `pool is already exist`（`spool.rs:38-62`）。
- `PoolInner`：所有克隆句柄共享的状态，包括准入锁、释放条件变量、原始/当前容量、运行/等待计数、停止标记、线程句柄、任务管理器、指标和 `BasePool`（`spool.rs:64-84`）。
- `Pool`：只含 `Arc<PoolInner>` 的可克隆公开句柄；`Debug` 有意只展示名称、容量和运行数（`spool.rs:86-101`）。
- `WaitingGuard`：提交阶段的 RAII 计数器。构造时增加 `waiting`；析构时在 `admission` 锁内减少计数并唤醒释放方（`spool.rs:103-121`）。
- `Pool::new` / `NewPool`：Rust 风格构造函数及 Go 风格兼容门面（`spool.rs:126-170,387-394`）。
- `Pool::run` / `Run`：提交一次性 `FnOnce() + Send + 'static` 闭包（`spool.rs:201-212,323-325`）。
- `Pool::run_with_concurrency` / `RunWithConcurrency` / 包级 `run_with_concurrency`：多 worker 通道消费入口及兼容转发函数（`spool.rs:214-231,327-329,378-385`）。
- `Pool::tune` / `Tune`：动态调容入口（`spool.rs:172-199,319-321`）。
- `check_and_add_running`：在 `admission` 锁内计算 `capacity - running`，一次保留 `min(available, requested)` 个槽位（`spool.rs:265-289`）。
- `spawn_reserved`：启动线程、捕获任务 panic、归还一个 `running` 计数并保存 `JoinHandle`（`spool.rs:291-299`）。
- `release_and_wait` / `ReleaseAndWait`：停止、等待与注销入口（`spool.rs:301-317,339-341`）。
- `run_task`：多并发 worker 循环；`RunningTask` 的 `Drop` 保证退出或 panic 时调用 `Meta::DecTask`（`spool.rs:396-414`）。

## 执行流程

构造流程如下：`Pool::new` 先拒绝 `size == 0`，再创建带名称标签的 `tidb_rm_pool_concurrency` gauge；随后把选项、容量、计数器、`TaskManager::NewTaskManager(size)` 和 `BasePool` 装入 `Arc<PoolInner>`。最后将 `pool.clone()` 擦除为 `Arc<dyn GoroutinePool>` 注册到 `InstanceResourceManager`。只有注册成功才返回句柄；重名等注册失败映射为 `Error::Registration`。

单任务流程如下：`run` 先创建 `WaitingGuard`，检查 `is_stop`，再调用 `check_and_add_running(1)`。有空闲槽时先增加 `running`，后由 `spawn_reserved` 启动线程；线程无论正常返回还是 panic，都会将 `running` 减一。没有槽位且 `Blocking == false` 时立即返回 `Overload`；阻塞模式则每隔 `WAIT_INTERVAL` 重试，同时在每轮锁外休眠。

通道任务流程如下：`run_with_concurrency` 用同一准入逻辑得到 `actual`，创建带唯一任务 ID、任务通道和“请求并发度”的 `Meta`，向 `TaskManager` 登记后启动 `actual` 个 worker。每个 `run_task` 先 `IncTask`，随后反复等待 `MetaEvent`：`Task` 分支执行一个闭包；`Exit` 表示缩容退出；`Closed` 表示任务通道关闭且已无可取任务。`RunningTask` 在所有退出路径调用 `DecTask`。

调容流程如下：`tune(0)` 直接忽略；其他值在 `admission` 锁内更新时间戳、容量与 gauge。扩容且实际运行数低于新容量时，`TaskManager::Overclock` 选择一个可提升并发的已登记任务，本次调用至多新建一个 worker。运行数高于新容量时，`Downclock` 向合适的 worker 元数据发送退出信号。资源管理器的 `schedule.rs::Exec` 会基于调谐间隔与上下限调用 trait 方法 `Tune`，因此这里是资源调度命令落实到实际 worker 的边界。

释放流程如下：`release_and_wait` 先用 `release_lock` 串行化多个释放者，再设置 `is_stop`。它持有 `admission` 锁检查 `waiting`，通过 `waiting_changed` 等待所有已进入提交路径的调用者退出；条件变量等待期间会释放该锁，使 `WaitingGuard::drop` 能减少计数。之后取走当前全部 `JoinHandle`、逐个 join，最后按池名注销。

## 数据与状态

- `origin_capacity` 在构造后不变，供调度器限制超频；`capacity` 是可调的当前准入上限。
- `running` 表示已保留并发槽的线程数，而不是已执行闭包总数。单任务线程结束时减一；通道 worker 只有在通道关闭、收到退出信号或 panic 后才减一。
- `waiting` 覆盖从 `run` / `run_with_concurrency` 进入直到准入成功、失败或线程句柄登记完成的整个提交阶段。它是释放逻辑与提交逻辑之间的屏障。
- `is_stop` 是单向停止标志；本文件没有重新开启池的接口。
- `threads` 保存所有创建过且尚未由释放流程取走的 `JoinHandle`。线程结束不会自行从向量移除，所以资源在 `release_and_wait` 时统一回收。
- `task_manager` 只服务 `run_with_concurrency` 的长驻 worker；普通 `run` 不登记 `Meta`，因此调容不会把已提交的一次性闭包迁移或复制。
- `BasePool` 保存注册名、递增任务 ID 和最近调谐时间；`concurrency_metric` 跟踪配置容量，而非实时 `running`。
- 所有本文件原子操作均使用 `Ordering::SeqCst`。复合的准入读改写由 `admission: Mutex<()>` 串行化；释放者之间由 `release_lock` 串行化。

## 依赖与调用关系

上游关系：

- 同目录 [`lib.rs`](lib.rs) 导出全部公开符号，根 [`pkg/lib.rs`](../../../lib.rs) 再经 `resourcemanager::pool::spool` 门面导出。
- `Pool::new` 把 `Arc<Pool>` 注册成 [`GoroutinePool`](../../util/util.rs)，因此 [`pkg/resourcemanager/schedule.rs`](../../schedule.rs) 可经 `Running`、`Cap`、`LastTunerTs`、`GetOriginConcurrency` 和 `Tune` 对其调度。
- RustCodeGraph 对 `NewPool` 找到的直接调用者是同目录 `migration_aster_unit_test.rs`；对核心 `run_with_concurrency` 找到的调用者是三个兼容转发入口。配合仓库文本搜索，未发现测试外直接构造此 Rust 池的生产调用。

下游关系：

- [`BasePool`](../basepool.rs) 提供名称、Go `uint64` 回绕语义的任务 ID 和调谐时间戳，同时提供兼容错误文案常量。
- [`TaskManager`、`Meta`、`TaskChannel` 与 `MetaEvent`](../../poolmanager/task_manager.rs) 提供多 worker 任务登记、通道接收和 worker 计数；`task_manager_scheduler.rs` 提供 `Overclock` / `Downclock`。
- [`InstanceResourceManager`](../../rm.rs) 保存 `Arc<dyn GoroutinePool>`，构造时 `Register`，释放完成后 `Unregister`。
- [`option.rs`](option.rs) 的 `load_options` 决定满载时阻塞还是立即报错，默认值为 `Blocking = true`。
- `prometheus::Gauge` 记录配置并发；`std::thread` 承担实际执行；`catch_unwind` 隔离任务 panic。

## 错误处理与边界

- 构造只显式拒绝 `size == 0`；负容量不会被 `InvalidParams` 拒绝，但会导致正常准入时没有正的可用槽位。该事实来自当前条件判断，不应扩写成推荐用法。
- `tune` 同样只忽略零，不拒绝负数。容量相减使用 `wrapping_sub`，与 Go `int32` 边界语义对齐；`migration_aster_unit_test.rs::admission_subtraction_wraps_like_go_int32` 覆盖了溢出场景。
- `run` 和 `run_with_concurrency` 在入口已停止时返回 `Closed`；若停止发生在入口检查与准入循环之间，`check_and_add_running` 返回 `None`，外层当前会映射成 `Overload`。调用方不能假定所有并发关闭竞态都返回 `Closed`。
- 非阻塞满载明确返回 `Overload`。阻塞模式没有超时或取消参数，会持续轮询，直到容量可用或池停止。
- 请求的多并发数大于空闲槽时不是错误，而是截断到空闲槽；现有 Rust/Go 测试均验证此行为。若传入 `concurrency == 0` 且池有空位，当前实现会登记 `Meta`、不启动 worker并返回成功；现有测试未覆盖这一边界。
- 任务 panic 被 `spawn_reserved` 的 `catch_unwind` 捕获，槽位仍会归还；与 Go 版本记录错误和堆栈不同，Rust 当前丢弃 panic payload，不写日志。
- `Mutex` / `Condvar` 的 `unwrap` 意味着相关锁中毒会再次 panic；任务闭包自身的 panic 被隔离，但池内部同步原语失败没有转换成 `Error`。
- 注册层的任意错误都被压缩为 `Error::Registration`，不会保留底层 `PoolMapError` 细节。
- `release_and_wait` 忽略线程 `join` 的错误，因为任务 panic 已在工作线程内捕获；注销缺失名称也由资源管理器按约定忽略。

## 并发与资源生命周期

`Pool` 克隆只增加 `Arc` 引用计数，所有句柄共享容量和停止状态。准入先增加 `running`、再启动线程，避免线程已运行但尚未计数；`WaitingGuard` 直到 `spawn_reserved` 已把句柄写入 `threads` 后才析构，因此释放方观察到 `waiting == 0` 时，不会漏掉一个已经准入但尚未登记句柄的提交。

`release_and_wait` 将停止标志设为 true 后，阻塞在 5 ms 轮询中的提交者会退出准入循环。条件变量只负责等待这些提交者，不负责普通容量等待。随后 join 保证所有已登记线程结束后才注销。调用方必须先让一次性任务结束、关闭 `TaskChannel`，或依靠调容退出信号使长驻 worker 可退出；否则释放会一直阻塞，这是 API 的资源生命周期契约。

多并发 worker 共享 `Meta` 和 MPMC `TaskChannel`。通道关闭后，`Meta::recv_event` 会优先取仍可获得的任务；队列排空后返回 `Closed`，所以已有任务可以在关闭后被排空。缩容通过独立退出信号竞争选择；`run_task` 的 RAII 守卫保证正常关闭、降频和任务 panic 三种退出都减少 `Meta` 的运行计数，而外层线程包装同时减少池级 `running`。

当前实现没有 `Drop for Pool`。仅丢弃最后一个公开 `Pool` 句柄不能替代 `release_and_wait`，并且资源管理器自身持有注册时的 `Arc<dyn GoroutinePool>`；安全使用者应显式释放以 join 线程并注销。

## 与 Go 版本的对应关系

直接对照文件是 [`spool.go`](spool.go)，行为测试是 [`spool_test.go`](spool_test.go)。Rust 保留了 `NewPool`、`Run`、`RunWithConcurrency`、`Tune`、`Cap`、`Running`、`ReleaseAndWait`、`GetOriginConcurrency` 等 Go 风格名称，同时提供 Rust 风格小写方法。

主要一致点：零容量构造失败；默认阻塞、可选非阻塞；满载截断/拒绝规则；5 ms 轮询；容量、运行数和等待数分离；构造注册、释放注销；多并发任务由 `TaskManager` 调容；原始并发和最后调谐时间参与资源管理器决策。`spool_test.rs` 对应 Go 的释放、扩缩容、过载、并发截断和任务管理器场景；`migration_aster_unit_test.rs` 另行覆盖重名注册、Go API 名称、panic 恢复、关闭竞态顺序和整数回绕。

实现差异：Go 使用 goroutine、`sync.WaitGroup`、`sync.Cond` 和全局 metrics 向量；Rust 使用 OS 线程、显式 `JoinHandle`、`Arc`/原子/互斥锁，并为每个池直接构造 gauge。Go 在任务 panic 时记录错误与堆栈，Rust 只捕获并丢弃 panic。Go 的任务/退出通道由原生 `select` 处理，Rust 的 `TaskChannel`/`SignalChannel` 基于 `crossbeam-channel`，由 `Meta::recv_event` 统一成 `MetaEvent`。这些差异不改变现有测试覆盖的核心调度语义，但会影响线程成本、诊断信息和指标注册方式。

## 扩展指南

- 新增准入策略（超时、取消、公平等待）应集中修改 `check_and_add_running`、`WaitingGuard` 和 `release_and_wait`，并保持“计数预留—句柄登记—等待计数归零”的顺序不变量。
- 新增错误类型时同步 `Error`、`Display`、Go 兼容常量及调用方分支；若要区分停止竞态，需调整 `check_and_add_running` 的返回类型，而不是仅修改外层错误映射。
- 修改多并发执行或扩缩容时，应同时检查 `run_with_concurrency`、`run_task`、`TaskManager::{RegisterTask,Overclock,Downclock}` 和 `Meta::recv_event`；尤其要保证池级 `running` 与 Meta 级运行计数在 panic、关闭和降频路径都成对回落。
- 修改指标时注意当前 gauge 表示 `capacity` 而非实时运行数，并评估同名 label/重复构造的注册与采集语义。
- 若接入新的生产调用点，优先通过根门面 `resourcemanager::pool::spool` 使用，并确保所有成功构造路径最终显式调用 `release_and_wait`。
- 测试逻辑必须继续放在独立文件。行为回归扩展 [`spool_test.rs`](spool_test.rs)，Go/Rust 迁移与兼容边界扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并同步审视 [`spool_test.go`](spool_test.go)；不要把测试内嵌到 `spool.rs`。
- 性能风险主要来自“一槽一 OS 线程”、5 ms 轮询和不断增长到释放时才清空的句柄向量；兼容风险主要来自 Go 风格错误文案、整数回绕、调容每次至多增一个 worker，以及关闭后排空已排队任务的语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，可查询本文件全部 414 行。
- RustCodeGraph 文件/符号查询：`files --filter pkg/resourcemanager/pool/spool`；`node --file .../spool.rs --offset 1 --limit 500`；`query` / `node` 核对 `NewPool`、两个 `run_with_concurrency` 和 `run_task`。图中确认 `NewPool -> Pool::new`，`run_with_concurrency -> check_and_add_running / spawn_reserved / run_task`，以及 `tune`、`run_with_concurrency -> run_task`；图对跨 crate 构造调用未给出生产调用边，随后用 `rg` 补查门面和引用。
- crate/入口证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`pkg/lib.rs`](../../../lib.rs) 和根 `Cargo.toml` 的 `facade_resourcemanager_pool_spool` 路径依赖。
- 下游实现证据：[`basepool.rs`](../basepool.rs)、[`poolmanager/task_manager.rs`](../../poolmanager/task_manager.rs)、[`poolmanager/task_manager_scheduler.rs`](../../poolmanager/task_manager_scheduler.rs)、[`util/util.rs`](../../util/util.rs)、[`rm.rs`](../../rm.rs)、[`schedule.rs`](../../schedule.rs) 和 [`option.rs`](option.rs)。
- Go 对照与独立测试证据：[`spool.go`](spool.go)、[`spool_test.go`](spool_test.go)、[`spool_test.rs`](spool_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前以任务指定命令验证本文恰含 11 个固定二级章节，并人工复核所有“已接线/未发现调用”结论均有上述源码、图查询或仓库搜索支撑。
