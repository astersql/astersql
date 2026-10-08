# `pkg/statistics/handle/autoanalyze/refresher/worker.rs`

## 文件定位

本文件是 `astersql-statistics-handle-autoanalyze-refresher` crate 的并发执行层。crate 入口 `pkg/statistics/handle/autoanalyze/refresher/lib.rs` 声明并重新导出 `worker` 模块；同 crate 的 `Refresher` 负责生成、排序和重试作业，`Worker` 只负责并发准入、异步调用执行器、维护运行中表 ID，以及等待或停止。

直接生产调用链是 `Refresher::analyze_highest_priority_tables` → `Worker::submit_job` → `AnalysisExecutor::analyze`。当前 Rust crate 的 `Cargo.toml` 将 Go 包映射记录为 `pkg/statistics/handle/autoanalyze/refresher`，但所有迁移期依赖都位于 `target.'cfg(any())'` 下；本文件实际只使用标准库，因此它当前没有接入 Go 版本使用的统计句柄、系统进程跟踪器和日志组件。

## 核心职责

- `Worker::submit_job` 以原子计数器实现无阻塞并发准入：已停止或活跃数达到上限时返回 `false`，成功占槽后为作业创建独立 OS 线程。
- `Worker::running_jobs` 返回运行中物理表 ID 的快照，供 `Refresher::analyze_highest_priority_tables` 避免再次选择已运行的表，也供测试和诊断读取。
- 后台线程无论执行器返回 `Ok`、返回 `Err` 还是 panic，都会删除运行标记、释放活跃槽位并唤醒等待者；线程创建失败也走同样的同步回滚路径。
- `Worker::update_concurrency` 支持运行时替换并发上限；`Worker::wait_finished` 和 `Worker::stop` 提供作业收尾等待，其中 `stop` 还设置永久拒绝新提交的标志。

该文件不负责优先级排序、时间窗口、初始化扫描或 must-retry 入队；这些职责位于 `refresher.rs`。`AnalysisJob.must_retry` 在本文件中不参与分支，只作为作业描述的一部分传给执行器；真正的提交失败重试容器是 `Refresher::must_retry`。

## 主要符号

- `AnalysisJob { table_id, priority, must_retry }`：可克隆的作业值。`table_id` 同时是执行参数和 `running` 集合键；`priority` 由 `refresher.rs` 的 `QueuedJob` 排序；`must_retry` 由上层语义使用，本文件不读取后两者。
- `AnalysisExecutor: Send + Sync`：执行边界 trait。`analyze(&AnalysisJob) -> Result<(), String>` 允许一个执行器被多个后台线程共享。`Send + Sync` 与 `Arc<dyn AnalysisExecutor>` 一起构成跨线程调用的类型保证。
- `Worker`：持有共享执行器和全部并发状态。它没有实现 `Clone`；提交后通过逐字段克隆 `Arc` 将状态交给后台线程。
- `Worker::new`：初始化给定并发上限、零活跃数、空运行集合、条件变量和未停止状态。
- `Worker::update_concurrency` / `Worker::max_concurrency`：分别以 `Release` 写和 `Acquire` 读并发上限。值 `0` 是有效配置，表示拒绝所有作业。
- `Worker::submit_job`：核心入口。使用 `active.compare_exchange` 循环原子占槽，随后登记表 ID并创建线程。
- `Worker::running_jobs`：在互斥锁内克隆 `HashSet<i64>`，调用者不会持有内部锁。
- `Worker::stop` / `Worker::wait_finished`：前者先写 `stopped` 再等待，后者在 `running` 非空期间通过 `Condvar::wait` 休眠。

## 执行流程

1. `Refresher::analyze_highest_priority_tables` 初始化队列、检查时间窗口、处理 DML 变化、重新入队 must-retry 作业并同步并发度。
2. Refresher 取得 `Worker::running_jobs` 与 `Worker::max_concurrency` 快照，计算可提交数量，从优先队列弹出作业；已在快照中的 `table_id` 被跳过。
3. `Worker::submit_job` 先以 `Acquire` 检查 `stopped`。若为真立即返回 `false`。
4. 方法读取 `active`，循环比较 `active` 与当时的 `max_concurrency`。容量不足返回 `false`；否则以 `compare_exchange` 将活跃数加一。CAS 失败会用实际值重试。
5. 占槽成功后，在 `running: Mutex<HashSet<i64>>` 中插入 `job.table_id`，再克隆执行器和共享状态，调用 `std::thread::Builder::spawn`。
6. 后台闭包用 `catch_unwind(AssertUnwindSafe(...))` 包裹 `executor.analyze(&job)`；结果被有意丢弃。随后删除表 ID、将 `active` 减一，并 `notify_all`。
7. 若线程创建失败，提交线程撤销相同状态并返回 `false`；成功创建则返回 `true`。上层 Refresher 对 `false` 的作业放入 `must_retry`，下一轮重新入堆。
8. `wait_finished` 持有 `running` 锁检查集合；非空时通过条件变量原子释放锁并等待，醒来后循环复查，以处理伪唤醒。

## 数据与状态

`executor` 是不可变的 `Arc<dyn AnalysisExecutor>`。`max_concurrency`、`active` 和 `stopped` 分别以 `Arc<AtomicUsize>`、`Arc<AtomicUsize>` 和 `Arc<AtomicBool>` 在 Worker 与后台线程之间共享；`running` 与 `finished` 分别用 `Arc<Mutex<HashSet<i64>>>` 和 `Arc<Condvar>` 配对。

并发准入的权威计数是 `active`，不是 `running.len()`：CAS 保证多个提交者不会同时越过同一槽位。`running` 是按表 ID 去重的诊断/等待状态。正常约束下，每个已接受作业先占一个 `active` 槽并登记 ID，终结路径按相反顺序删除 ID、减槽并通知。

降低 `max_concurrency` 不会取消或阻塞已经运行的作业；它只影响之后的 `submit_job`。因此短时间内 `active` 可以高于新上限，新作业会持续被拒绝，直到活跃数降到上限以下。

## 依赖与调用关系

- 上游生产调用者：`pkg/statistics/handle/autoanalyze/refresher/refresher.rs` 的 `Refresher::update_concurrency`、`analyze_highest_priority_tables`、`running_jobs`、`wait_finished` 和 `close` 分别转发或组合 Worker API。
- 下游执行边界：`Worker::submit_job` 调用 `AnalysisExecutor::analyze`；具体生产执行器尚未在本 crate 中提供。仓库搜索到的 Rust trait 实现位于独立测试 `worker_test.rs` 与 `refresher_test.rs`。
- 标准库依赖：`HashSet` 保存表 ID；原子类型处理准入、配置和停止状态；`Mutex`/`Condvar` 处理集合与等待；`thread::Builder` 创建任务线程；`catch_unwind` 隔离执行器 panic。
- crate 边界：`pkg/statistics/handle/autoanalyze/refresher/Cargo.toml` 指定 `lib.rs` 为库入口并记录 Go 包迁移来源。列出的跨 crate 依赖均受恒假条件 `cfg(any())` 屏蔽，所以不能据此声称当前 Worker 已直接连接完整统计子系统。
- Go 应用位置证据：`worker.go` 被 `refresher.go`、`pkg/statistics/handle/bootstrap.go` 和 `pkg/statistics/handle/globalstats/global_stats.go` 使用；Rust 侧当前可确认的直接生产接线仅到同 crate 的 `refresher.rs`。

## 错误处理与边界

`AnalysisExecutor::analyze` 的 `Err(String)` 不从后台线程传播，也不被本文件记录；与 panic 一样，它只触发清理。调用者从 `submit_job` 得到的 `false` 仅表示停止、容量不足或线程创建失败，无法区分原因。若需要可观测错误或失败策略，应扩展执行器/完成通道或上层 hook，而不能把布尔值解释为分析执行结果。

所有互斥锁使用 `expect("... mutex poisoned")`。执行器 panic 被限制在不持有 `running` 锁的区域，因此该已覆盖路径不会毒化此锁；但其他持锁代码若 panic，后续访问会继续 panic，而不是返回可恢复错误。

`catch_unwind(AssertUnwindSafe(...))` 确保清理闭包继续执行，但 `AssertUnwindSafe` 是显式承诺，不证明任意执行器内部状态都具备 unwind safety。它也不会处理进程 abort。

按当前实现，同一个 `table_id` 若被并发接受多次，`HashSet` 只保留一个键；任一作业先完成都会删除该键，使 `running_jobs` 和 `wait_finished` 可能早于另一个同表作业完成。Refresher 尝试通过运行快照避免重复，但该快照在一次提交循环内不会随新提交更新，且直接调用 Worker 也没有去重保护。因此“同表最多一个活跃作业”是调用侧需要维持的前置条件，不是 Worker 自身保证。

## 并发与资源生命周期

每个成功提交创建一个未保存 `JoinHandle` 的独立 OS 线程。Worker 不直接 join；资源完成由 `running` 集合和条件变量观测。最后一个正常登记的表 ID 被删除后，所有等待者被唤醒并在 while 条件中重新确认。

`active` 的 `AcqRel` CAS/减法与 `max_concurrency`、`stopped` 的 Acquire/Release 配对提供跨线程可见性；`running` 的一致性由互斥锁保证。条件变量通知发生在删除 ID 和释放槽位之后，但调用 `notify_all` 时不持有 `running` 锁，等待方仍通过循环条件避免丢失通知造成错误完成判断。

`stop` 在设置 `stopped` 后等待当时可见的 `running` 变空；它不是与并发 `submit_job` 完全线性化的关闭协议。提交者可能已通过停止检查但尚未登记作业，此时 `stop` 可能先观察到空集合并返回。因此安全关闭要求上层先停止产生/调用提交（`Refresher::close` 先设置自身 `closed`），再调用 `Worker::stop`。

线程创建失败时，`spawn` 返回的 `Err` 不含后台线程，提交线程负责同步撤销槽位和运行标记。Worker 被丢弃不会自动调用 `stop`，所以所有权方需要显式收尾；当前 `Refresher::close` 在首次关闭时清空队列并调用它。

## 与 Go 版本的对应关系

`worker.go` 的 `worker`、`NewWorker`、`UpdateConcurrency`、`SubmitJob`、`GetRunningJobs`、`GetMaxConcurrency`、`Stop` 和测试等待函数，分别对应 Rust 的 `Worker`、`new`、`update_concurrency`、`submit_job`、`running_jobs`、`max_concurrency`、`stop` 和 `wait_finished`。两边都允许并发度为 `0`，按表 ID维护运行集合，在容量满时拒绝提交，并在错误或 panic 后清理运行状态。

实现机制不同：Go 用一个 mutex 同时保护运行集合和并发配置，以 `util.WaitGroupWrapper.RunWithRecover` 启动 goroutine，并由 `processJob` 调用 `priorityqueue.AnalysisJob.Analyze(statsHandle, sysProcTracker)`；Rust 用原子槽位、互斥集合、条件变量、独立线程和 `AnalysisExecutor` trait。Go 还记录并发变更、提交、执行错误与 panic 日志；Rust 当前不记录，也把执行错误丢弃。

Rust 的 `AnalysisJob` 是仅含三个字段的值类型，并非 Go `priorityqueue.AnalysisJob` 的完整行为接口；实际 Analyze 行为被移到 `AnalysisExecutor`。Rust `stop` 会永久设置拒绝标志，而 Go `Stop` 只等待 WaitGroup；Rust `wait_finished` 是生产可见方法，Go 的轮询等待方法明确仅供测试。上述差异说明当前 Rust 文件是保持核心并发语义的迁移实现，不能视为 Go worker 所有集成能力已移植。

## 扩展指南

- 增加生产执行器时，实现 `AnalysisExecutor`，并在组装 `Refresher`/`Worker` 的上层显式注入；若要对齐 Go，应同时设计 StatsHandle、sysproc tracker、日志和作业成功/失败 hook，而不是把这些职责隐式塞进 `Worker`。
- 改动并发准入时，保持“先原子占槽、再登记、所有退出路径对称撤销”的不变量，并同步 `worker_test.rs` 中并发上限、零并发、错误和 panic 测试。
- 若要保证同表去重，应在 `submit_job` 内以单一临界区或独立保留集合协调“检查并插入”，并让等待条件基于可靠的每作业计数；仅检查 `HashSet::contains` 不能解决并发竞态。还应添加两个同 ID 作业同时提交与等待不提前返回的独立回归测试。
- 若要强化关闭语义，应使停止检查、占槽和登记与关闭建立线性化顺序，或明确要求并验证上游先停止生产；添加“提交与 stop 竞态”测试。不要在 `stop` 后悄悄允许重新启动，除非 API 和 Go 对齐目标同时更新。
- 若要传播执行失败，需定义结果接收、重试或 hook 的所有权和背压策略；当前 `submit_job: bool` 只表达准入结果，修改其含义会影响 `Refresher::analyze_highest_priority_tables` 的 must-retry 分支。
- 测试逻辑应继续放在独立的 `pkg/statistics/handle/autoanalyze/refresher/worker_test.rs`，不要内嵌回 `worker.rs`；涉及调度协作时再同步 `refresher_test.rs`，并参考 `worker_test.go` 保持 Go 语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边；目标目录列出 `worker.rs`、`refresher.rs`、两份 Rust 测试及对应 Go 文件。
- RustCodeGraph `node --file pkg/statistics/handle/autoanalyze/refresher/worker.rs`：核对了文件全部 151 行、`AnalysisJob`、`AnalysisExecutor`、`Worker` 字段及七个公开方法的实际实现。
- RustCodeGraph `callees submit_job`：目标 `worker.rs:74` 的 `submit_job` 调用边指向 `worker.rs:39` 的 `analyze`；同名符号存在歧义，因此上游边另由精确源码与仓库搜索交叉核验。
- RustCodeGraph 文件节点：读取 `refresher.rs` 全部 248 行，确认第 147 行提交调用及并发、等待、关闭转发；读取 `worker_test.rs` 全部 188 行，确认并发上限、运行快照、动态/零并发、panic 和错误释放槽位；读取 `worker.go` 全部 152 行与 `worker_test.go` 全部 180 行，核对 Go 对照语义；读取 `lib.rs` 全部 20 行，核对模块声明、再导出和独立测试挂接。
- 配置与接线：直接读取 `pkg/statistics/handle/autoanalyze/refresher/Cargo.toml` 和 `BUILD.bazel`；用 `rg` 核对 Rust 中 `Worker::new`、`AnalysisExecutor` 与 `submit_job` 的实际引用范围。目标路径及其上层统计目录未发现更近的 `doc.go` 或嵌套 `AGENTS.md`。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试。交付前另运行任务指定的 11 章节结构验证，并人工复查唯一新增产物、无源码/Cargo/Go/总计划修改。
