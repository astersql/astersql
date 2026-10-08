# `pkg/statistics/handle/util/pool.rs`

## 文件定位

本文说明的源文件是 [`pool.rs`](pool.rs)。它属于 `astersql-statistics-handle-util` crate；crate 入口 `pkg/statistics/handle/util/lib.rs` 以 `pub mod pool` 声明模块并通过 `pub use pool::*` 重导出其公开 API。该 crate 的清单 `pkg/statistics/handle/util/Cargo.toml` 指明 Go 对照包为 `pkg/statistics/handle/util`，而本文件直接对应同目录的 `pool.go`。

它提供统计子系统使用的两类资源抽象：可复用线程的 `GoroutinePool`，以及把该线程池和 `SessionPool` 聚合起来的 `Pool`/`StatsPool`。Rust 接口层 `pkg/statistics/handle/types/interfaces.rs` 会继续重导出 `Pool`，使其成为 `StatsHandle` 的组成契约；但截至本次检查，RustCodeGraph 和仓库搜索没有找到 `new_pool`、`g_pool`、`s_pool` 或该 `GoroutinePool::submit` 的非测试 Rust 调用者。因此当前 Rust 文件已经实现并导出资源池能力，却尚未像 Go `NewHandle` 那样接入生产构造链，不能把 Go 侧的运行时使用情况视作 Rust 侧已接线。

## 核心职责

- `GoroutinePool` 接受 `FnOnce() + Send + 'static` 任务。提交时若已有等待 worker，就把任务加入 FIFO 队列并唤醒一个 worker；否则立即新建 OS 线程执行任务（`GoroutinePool::submit`）。
- 任务结束后，worker 仅在空闲 worker 数尚未达到 `maximum` 时进入复用状态；达到上限的 worker 直接退出。由此，`maximum` 限制的是可保留的空闲 worker，而不是并发执行任务数。这一点由 `pool_test.rs::capacity_limits_idle_workers_not_concurrent_jobs` 明确验证。
- 空闲 worker 通过 `Condvar` 等待后续任务；非零 `idle_timeout` 到期且队列仍为空时退出，零超时则无限等待以供复用（`submit` 的 worker 循环及 `zero_idle_timeout_keeps_worker_for_reuse`）。
- `Pool` trait 统一暴露任务池、会话池和关闭入口；`StatsPool` 是默认聚合实现，`new_pool` 返回 `Arc<dyn Pool>` 以便在统计接口边界共享。
- `close` 只关闭任务池：它拒绝关闭后的新任务并唤醒等待 worker，不等待正在执行的任务结束，也不拥有或关闭传入的 `SessionPool`。

## 主要符号

- `MAX_STATS_WORKERS: usize = i16::MAX as usize`：默认空闲 worker 上限，与 Go `math.MaxInt16` 参数对齐。
- `STATS_WORKER_IDLE_TIMEOUT: Duration`：默认 60 秒空闲回收阈值，与 Go `time.Minute` 对齐。
- `type Job = Box<dyn FnOnce() + Send + 'static>`：队列中的一次性、可跨线程任务。
- `PoolState { jobs, waiting, closed }`：互斥锁保护的共享状态。`jobs` 是 FIFO 队列，`waiting` 记录已登记为等待者的 worker 数，`closed` 是关闭标志。
- `WorkerGuard(Arc<AtomicUsize>)`：线程生命周期守卫；`Drop` 时递减存活 worker 计数，保证正常退出路径不会漏记。
- `GoroutinePool`：持有共享状态/条件变量、存活与空闲 worker 原子计数、空闲保留上限和超时。公开方法为 `new`、`submit`、`worker_count`、`close`。
- `Pool`：`Send + Sync` trait，包含 `g_pool() -> Arc<GoroutinePool>`、`s_pool() -> Arc<dyn SessionPool>` 和 `close()`。
- `StatsPool`：保存 `Arc<GoroutinePool>` 与 `Arc<dyn SessionPool>`；`StatsPool::new` 使用两个默认常量创建任务池。
- `new_pool`：公开工厂，将 `StatsPool` 擦除为 `Arc<dyn Pool>`。

## 执行流程

1. 调用 `GoroutinePool::new(maximum, idle_timeout)` 时，只初始化共享状态和计数，不预创建线程。
2. `submit` 把闭包装箱后锁定 `PoolState`。若 `closed` 为真，返回 `Ok(())` 且不执行任务；若 `waiting != 0`，则把任务压入 `jobs` 尾部、通知一个等待者并返回。
3. 没有等待者时，`submit` 先释放状态锁，再增加 `workers`，随后用 `thread::spawn` 创建线程。线程首先执行本次任务；任务并不先经过共享队列。
4. 首次任务返回后，worker 用 `idle_workers.fetch_update` 尝试占用一个空闲保留名额。若当前空闲数已经达到 `maximum`，线程立即返回；否则进入复用循环。
5. 每轮等待前，在状态锁内增加 `waiting`。零超时使用 `Condvar::wait` 持续等待，非零超时使用 `wait_timeout_while`；被唤醒后减少 `waiting`，优先从队首取任务并在释放锁后执行。
6. 超时且队列为空，或池关闭且没有任务可取时，worker 退出循环；随后递减 `idle_workers`，并由 `WorkerGuard` 递减 `workers`。
7. `StatsPool::new` 保存调用者提供的会话池，同时创建默认 `GoroutinePool`。`g_pool`/`s_pool` 各自克隆 `Arc`，不会转移底层资源所有权。
8. 显式 `close` 或最后一个 `GoroutinePool` 值的 `Drop` 会设置关闭标志并 `notify_all`。该操作不 join worker；运行中的任务自行完成，等待中的 worker 醒来退出。

## 数据与状态

`PoolState` 的三个字段必须在同一把 `Mutex` 下观察和修改，因此“队列为空、等待者数量、是否关闭”的组合判断是一致的。任务用 `VecDeque` 保存，消费端使用 `pop_front`，所以进入共享队列的任务遵循 FIFO；直接触发新线程的任务之间没有顺序保证。

`workers` 表示当前仍存活的 worker 线程数量：创建线程前递增，线程闭包离开时通过 `WorkerGuard` 递减。`idle_workers` 表示已经取得空闲保留名额的 worker 数；它在线程进入复用阶段时增加、退出复用阶段时减少。`worker_count` 只读取前者，因此可能包含正在执行任务或正准备退出的线程，不表示队列长度或可用容量。

`waiting` 不是空闲 worker 总数的独立副本，而是 worker 持锁登记、正在条件变量等待流程中的数量；提交方仅用它决定“排队唤醒”还是“新建线程”。`maximum = 0` 会使每个线程完成首次任务后立即退出；`idle_timeout = Duration::ZERO` 则代表永久等待，而不是立即超时。

`StatsPool` 仅以 `Arc` 共享两个池。传入的 `SessionPool` 契约定义在 `pkg/statistics/handle/util/util.rs::SessionPool`：它负责借出会话、执行回调并保证归还；本文件不实现会话借还，也不改变会话池状态。

## 依赖与调用关系

下游依赖均来自标准库或同 crate：`VecDeque` 提供任务队列，`Arc` 共享池状态，`Mutex`/`Condvar` 协调提交者与 worker，`AtomicUsize` 维护无需状态锁读取的计数，`thread::spawn` 承载任务，`Duration` 配置等待策略；`crate::util::{SessionPool, StatsError}` 提供会话池契约和提交返回类型。`Cargo.toml` 没有为本文件引入第三方线程池依赖，和 Go 版本依赖 `github.com/tiancaiamao/gp` 的方式不同。

RustCodeGraph 将 `pool.rs` 识别为 31 个符号，并显示 `GoroutinePool` 的直接导入者为 `pool_test.rs`。对 `pool.rs::new_pool`、`pool.rs::g_pool`、`pool.rs::s_pool`、`pool.rs::submit` 的 callers 查询没有返回生产调用边；仓库级 Rust 搜索同样只找到本文件、`pool_test.rs` 以及接口重导出/trait 约束。这是“已提供接口但生产运行时尚未接线”的直接证据。

Go 对照链不同：`pkg/statistics/handle/handle.go::NewHandle` 调用 `util.NewPool(pool)` 并保存到 `Handle.Pool`；例如 `pkg/statistics/handle/storage/stats_read_writer.go::LoadStatsFromJSONConcurrently` 通过 `GPool().Go` 并发消费加载任务，统计 bootstrap、storage、history、usage、autoanalyze 等代码大量通过 `SPool()` 借用系统会话；`Handle.Close` 路径会调用 `Pool.Close()`。这些路径说明本抽象在完整 Go 应用中的目标位置，但不是 Rust 已接线的证据。

## 错误处理与边界

`submit` 的签名返回 `Result<(), StatsError>`，但当前所有正常分支都返回 `Ok(())`，包括池已经关闭的情况；`StatsError::PoolClosed` 在本函数中未使用。这有意对齐 Go `gp.Pool.Go` 的静默忽略语义，调用者不能依靠返回值判断任务是否真正排队或执行。

锁和条件变量操作使用 `unwrap()`：如果持锁线程 panic 导致 mutex poison，后续提交或 worker 会 panic，而不是转换为 `StatsError`。`thread::spawn` 失败也会 panic。任务闭包自身的 panic 没有在池内捕获；该 worker 会展开退出，但 `WorkerGuard` 仍会在展开时递减 `workers`。因此需要隔离任务 panic 时，应在提交的闭包内部使用 `catch_unwind` 或业务层恢复策略；Go 的 JSON 并发加载调用点就在任务体内用 `recover` 记录错误。

关闭与提交由同一状态锁串行化：观察到 `closed` 的提交会被忽略；已经入队的任务不会在 `close` 中被清空，等待 worker 被唤醒后仍会先取出队列任务，再在队列为空时因关闭退出。`close` 可重复调用且不等待任务完成。文件没有取消、背压、队列上限、join、任务返回值或错误汇聚机制。

## 并发与资源生命周期

池按需创建 OS 线程，没有预热。没有等待 worker 时，每次提交都可创建新线程，所以瞬时并发不受 `maximum` 限制；`maximum` 只控制任务结束后最多有多少 worker 留在复用阶段。`capacity_limits_idle_workers_not_concurrent_jobs` 用 `maximum = 1` 同时启动两个阻塞任务，验证了这一不变量。

共享任务队列和关闭状态由 `Mutex`/`Condvar` 协调；计数采用 Acquire/Release 或 AcqRel 顺序。计数主要用于生命周期观测与空闲名额竞争，不替代状态锁保护队列。`Arc` 让已启动线程在外部池句柄销毁后仍能持有共享状态；`Drop for GoroutinePool` 发出关闭通知，但因没有 join，析构返回时 worker 或任务仍可能运行。

显式关闭后，运行中的任务继续；队列中已经接受的任务由已存在的等待 worker 继续排空。没有等待 worker时不会出现已入队任务，因为 `submit` 仅在 `waiting != 0` 时入队，否则直接创建线程。关闭后提交的闭包在装箱并取得锁后被丢弃。`close_is_non_blocking_and_ignores_new_jobs` 验证了非阻塞关闭和新任务不运行；`zero_idle_timeout_keeps_worker_for_reuse` 通过线程 ID 相等验证永久等待模式会复用同一 worker。

## 与 Go 版本的对应关系

Rust `Pool` 对应 Go `Pool` 接口，`StatsPool` 对应未导出的 Go `pool`，`new_pool` 对应 `NewPool`，`g_pool`/`s_pool`/`close` 对应 `GPool`/`SPool`/`Close`。Rust 默认构造参数分别取 `i16::MAX` 和 60 秒，对齐 Go 的 `gp.New(math.MaxInt16, time.Minute)`；两边都只在 `Close` 中关闭任务池，不关闭外部传入的会话池。

关键实现差异是：Go 委托 `tiancaiamao/gp.Pool`，Rust 在本文件内用线程、队列和条件变量复刻当前所需语义。Rust 测试明确把“容量限制空闲 worker 而不是并发任务”“关闭不等待且关闭后提交静默忽略”“零超时永久复用”固定为兼容行为。

接口表示也有差异：Go 返回具体 `*gp.Pool` 和 `syssession.Pool`，Rust 返回各自的 `Arc`；Rust `submit` 形式上返回 `Result`，但当前不产生错误。Go 生产链已在 `NewHandle` 接入并由统计功能调用，Rust 当前仅完成资源池实现、重导出和 `StatsHandle` trait 组合约束，尚未发现构造或生产调用点。扩展时必须保持这一迁移状态表述，不能依据 Go 调用者推断 Rust 调用者存在。

## 扩展指南

- 若要把池接入 Rust 统计主链，应从 Rust `StatsHandle` 的实际构造/关闭生命周期增加最小接线，调用 `new_pool` 并确保关闭入口调用 `Pool::close`；同时在独立测试文件增加构造、使用和关闭的集成验证。不要仅新增接口而宣称生产接入。
- 修改容量或调度语义时，优先调整 `GoroutinePool::submit` 中“等待者优先/否则建线程”和 `idle_workers.fetch_update` 两处，并同步 `pkg/statistics/handle/util/pool_test.rs`。尤其要保留或明确改变 `maximum` 只限制空闲保留量这一 Go 兼容语义。
- 新增队列上限、背压或关闭错误时，需要决定是否打破关闭后 `Ok(())` 的现有行为，并核对 Go `gp.Pool.Go` 语义；若开始返回 `StatsError::PoolClosed`，所有调用方必须处理这一兼容变化。
- 新增等待关闭、任务错误收集或 panic 隔离时，不能把 join 放进当前 `close` 而不评估阻塞风险；现有测试要求 `close` 快速返回。可考虑新增独立方法而不是改变 `close` 契约。
- 调整 worker 计数时，应分别维护 `workers`（存活线程）和 `idle_workers`（取得保留名额），并为任务 panic、超时与关闭竞态增加确定性的独立测试。Rust 单元测试继续放在 `pool_test.rs`，不要内嵌进生产文件。
- 若改变 `Pool` trait 或 `SessionPool` 类型，应同步检查 `pkg/statistics/handle/types/interfaces.rs` 的重导出及 `interfaces_test.rs::public_traits_accept_the_real_dependency_contracts` 的组合约束。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/statistics/handle/util` 确认目标、Go 对照和测试均已索引；`node --file pkg/statistics/handle/util/pool.rs` 读取完整 201 行实现；`query GoroutinePool`、`query StatsPool`、`query new_pool`、`query submit` 定位主要符号；`callers`/`callees` 针对 `pool.rs::submit`、`pool.rs::new_pool`、`pool.rs::g_pool`、`pool.rs::s_pool` 未返回生产调用边。
- Rust 源与模块边界：`pkg/statistics/handle/util/pool.rs`、`pkg/statistics/handle/util/lib.rs`、`pkg/statistics/handle/util/util.rs::SessionPool`、`pkg/statistics/handle/types/interfaces.rs`、`pkg/statistics/handle/types/interfaces_test.rs`。
- crate 声明：`pkg/statistics/handle/util/Cargo.toml`，确认包名、`lib.rs` 入口、Go 包映射和依赖边界；本文件的并发实现只使用标准库及同 crate 类型。
- Rust 独立测试：`pkg/statistics/handle/util/pool_test.rs` 的三个测试分别覆盖空闲容量语义、非阻塞关闭/关闭后忽略任务、零超时 worker 复用。
- Go 对照与调用证据：`pkg/statistics/handle/util/pool.go`、`pkg/statistics/handle/handle.go::NewHandle`、`pkg/statistics/handle/storage/stats_read_writer.go::LoadStatsFromJSONConcurrently`，以及仓库搜索得到的 `SPool`/`GPool` 调用点。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核当前事实与 Go 目标链的界限。
