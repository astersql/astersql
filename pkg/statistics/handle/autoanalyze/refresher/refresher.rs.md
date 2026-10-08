# `pkg/statistics/handle/autoanalyze/refresher/refresher.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-autoanalyze-refresher` crate，crate 根在同目录的 [`lib.rs`](lib.rs)，后者公开 `refresher`、`worker` 两个模块并重新导出其符号。该 crate 的 [`Cargo.toml`](Cargo.toml) 用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/statistics/handle/autoanalyze/refresher`，说明这里是自动 ANALYZE 刷新器的 Rust 移植面。

当前 Rust 文件实现的是一个可独立测试的调度核心：从抽象 `JobSource` 取得初始化作业和 DML 增量，把 `AnalysisJob` 放入优先队列，在时间窗口和并发上限允许时交给同 crate 的 `Worker`。它不是 SQL ANALYZE 的真实执行入口；具体执行由 `Worker` 持有的 `AnalysisExecutor` 完成（见 [`worker.rs`](worker.rs) 的 `AnalysisExecutor::analyze` 和 `Worker::submit_job`）。

需要特别注意当前接线状态：上层 Rust [`../autoanalyze.rs`](../autoanalyze.rs) 定义了另一套 `PriorityRefresher` 抽象和 `StatsAnalyze<R>` 门面，但仓库中没有本文件 `Refresher` 对该 trait 的实现，也没有生产调用 `Refresher::new`；`../Cargo.toml` 对本 crate 的依赖位于 `target.'cfg(any())'.dependencies`。因此，Rust 版本目前是独立移植/单元测试实现，尚不能据此声称已进入完整 Rust 应用主链。实际 TiDB 自动分析主链仍可在 Go 的 [`../autoanalyze.go`](../autoanalyze.go) 中看到：`NewStatsAnalyze` 构造 `refresher.NewRefresher`，`handleAutoAnalyze` 调用 `AnalyzeHighestPriorityTables`。

## 核心职责

1. **定义作业来源边界**：`JobSource` 把“首次扫描”“消费 DML 增量”“读取期望并发度”隔离为三个方法，使调度器不依赖真实统计句柄、DDL notifier 或系统变量。
2. **维护确定性的优先队列**：私有 `QueuedJob` 为 `AnalysisJob` 实现堆排序，优先级越大越先出队；优先级相同时，`table_id` 越小越先出队。
3. **控制一次调度轮次**：`analyze_highest_priority_tables` 依次完成关闭检查、惰性初始化、时间窗口判断、DML 增量吸收、失败作业回队、并发度同步和按槽位提交。
4. **协调可重试作业**：当预估槽位可用、但 `Worker::submit_job` 因竞争或停止状态拒绝提交时，将作业放入 `must_retry`，下一轮重新入堆。
5. **管理队列与 Worker 生命周期**：提供快照、长度、运行集合、等待完成、只清队列和整体关闭等接口。

本文件不负责生成真实分析优先级、不校验表是否仍适合分析，也不执行 SQL；这些在完整 Go 实现中分别属于 priority queue/job 和 worker/exec。Rust 当前的 `AnalysisJob` 只有 `table_id`、`priority`、`must_retry` 三个字段，职责范围明显更窄。

## 主要符号

- `QueuedJob(AnalysisJob)`：私有堆元素包装器。`Ord::cmp` 先比较 `priority`，再用反向 `table_id` 比较实现“小 ID 优先”；`PartialOrd` 直接复用全序。
- `pub trait JobSource: Send + Sync`：线程安全的数据源接口。
  - `initialize() -> Result<Vec<AnalysisJob>, String>`：提供首次全量候选作业。
  - `process_dml_changes() -> Result<Vec<AnalysisJob>, String>`：提供自上次消费后产生的增量作业。
  - `desired_concurrency() -> usize`：提供当前 Worker 上限。
- `pub struct Refresher`：调度状态持有者。它拥有 `Worker`、`Arc<dyn JobSource>`、`queue`、`must_retry`、初始化/关闭原子标志、初始化互斥锁和日内时间窗口。
- `Refresher::new`：构造空队列、未初始化且未关闭的实例，默认时间窗口为 `[0, 1440)` 分钟。
- `update_concurrency`：把 `JobSource::desired_concurrency` 同步到 `Worker::update_concurrency`。
- `set_auto_analysis_time_window` / `is_within_time_window`：设置并判断普通或跨午夜窗口。允许 `end_minute == 1440`，拒绝 `start_minute >= 1440` 或 `end_minute > 1440`。
- `analyze_highest_priority_tables`：公开的核心调度入口，成功提交至少一个作业时返回 `Ok(true)`；关闭、窗外、无槽位或无作业等情况返回 `Ok(false)`；数据源错误返回 `Err(String)`。
- `initialize_queue`：私有双重检查初始化入口；成功后才以 Release 顺序写入 `initialized = true`。
- `process_dml_changes`：只有已初始化后才向堆追加 DML 作业；初始化前为空操作。
- `requeue_must_retry`：用 `mem::take` 原子式取空重试向量，再把作业逐个压回堆。
- `priority_queue_snapshot`、`running_jobs`、`wait_finished`、`is_queue_initialized`、`len`：诊断和测试接口；其中堆快照的迭代次序不是出队次序。
- `close_priority_queue`：清空普通队列和重试队列并重置初始化状态，但不停止已交给 Worker 的作业。
- `close`：用原子 swap 保证幂等；第一次调用时先清队列，再调用 `Worker::stop` 等待运行作业结束。

## 执行流程

核心入口 `analyze_highest_priority_tables(minute)` 的顺序具有行为意义：

1. 以 Acquire 读取 `closed`；已关闭则立即返回 `Ok(false)`。
2. 调用 `initialize_queue`。该函数先无锁读取 `initialized`，必要时取得 `initialize_lock` 后再次检查；只有 `source.initialize()` 成功、所有作业入堆后才发布初始化完成状态。初始化失败直接向上传播，下一轮仍可重试。
3. 初始化之后才检查时间窗口。这保证实例即使在窗口外启动，也会建立队列状态；对应 Rust 测试 `test_queue_initializes_outside_time_window`，也对应 Go `AnalyzeHighestPriorityTables` 中“先 Initialize/Rebuild、后检查窗口”的注释和实现。
4. 若在窗口内，调用 `process_dml_changes` 追加增量，再调用 `requeue_must_retry` 恢复上轮提交失败的作业，随后从数据源刷新 Worker 并发上限。
5. 取得一次 `running_jobs` 快照，以 `max_concurrency.saturating_sub(running.len())` 算出本轮最多提交数。`saturating_sub` 避免并发上限动态降低时发生无符号下溢。
6. 循环从最大堆弹出作业。若其 `table_id` 已在本轮开始时的运行快照中，则丢弃该队列项并继续；否则调用 `Worker::submit_job`。
7. 提交成功时增加本轮计数；提交失败时将作业加入 `must_retry` 并结束循环，避免在已出现槽位竞争或停止状态后继续空转。
8. 返回本轮是否至少提交了一个作业。实际执行异步发生；需要同步观察结果的调用方必须显式调用 `wait_finished`。

同优先级的作业通过 `QueuedJob::cmp` 以较小 `table_id` 先提交；不同优先级则以数值较大的 `priority` 先提交。独立测试 `test_analyze_submits_all_available_concurrency_slots_in_priority_order` 验证并发度为 2 时只选择三个候选中的两个最高优先级作业。

## 数据与状态

- `worker: Worker`：真正拥有执行器、活跃计数、运行中表集合、停止标志与完成条件变量。本文件只做提交与状态转发。
- `source: Arc<dyn JobSource>`：不可变共享的数据源。`Send + Sync` 约束允许 `Refresher` 在多线程间共享。
- `queue: Mutex<BinaryHeap<QueuedJob>>`：待提交作业。代码没有按 `table_id` 去重；数据源若重复产生同一表作业，除“当前已经运行”的检查外，重复项仍可能在不同轮次被提交。
- `must_retry: Mutex<Vec<AnalysisJob>>`：仅保存 `submit_job` 返回 `false` 的作业。`AnalysisJob.must_retry` 字段在本文件中既不读取也不修改；重试资格由“提交失败”这一控制流决定。
- `initialized: AtomicBool` 与 `initialize_lock: Mutex<()>`：组成一次成功初始化语义。错误不会设置标志，后续调用可重试；`close_priority_queue` 在持有同一初始化锁时重置标志。
- `closed: AtomicBool`：整体关闭的单向状态。`close_priority_queue` 不设置它，所以之后可以重新初始化；`close` 设置后不可恢复。
- `time_window: Mutex<(u32, u32)>`：日内分钟区间。`start <= end` 时采用左闭右开 `[start, end)`；`start > end` 时表示跨午夜，采用 `minute >= start || minute < end`。代码未单独验证传入 `is_within_time_window` 的 `minute < 1440`，公开调用方应传入合法日内分钟。

重要状态不变量是：`initialized == true` 只表示首次 `source.initialize` 已成功并把结果入堆，不表示队列非空；队列为空也不表示没有运行中的作业。`close_priority_queue` 清理的是尚未提交的两类队列状态，已提交作业的所有权已经转移给 Worker。

## 依赖与调用关系

RustCodeGraph 对 `analyze_highest_priority_tables` 给出的直接下游边包括：`initialize_queue`、`is_within_time_window`、`process_dml_changes`、`requeue_must_retry`、`update_concurrency` 以及运行集合查询；`initialize_queue` 调用 `JobSource::initialize` 并构造 `QueuedJob`，`process_dml_changes` 调用同名数据源方法并构造 `QueuedJob`。

本文件的直接 Rust 依赖全部来自标准库和同 crate 再导出：

- `std::collections::BinaryHeap` 提供最大堆，`HashSet` 表示运行中表 ID 快照。
- `Arc` 承载 trait object，共享所有权；`Mutex` 保护堆、重试列表、初始化临界区和时间窗口。
- `AtomicBool` 保护初始化/关闭快速路径，并使用 Acquire/Release/AcqRel 建立跨线程可见性。
- `crate::AnalysisJob` 与 `crate::Worker` 来自 [`worker.rs`](worker.rs)。`Worker::submit_job` 使用 CAS 占用槽位、启动独立线程，并在成功、执行器返回错误或 panic 后清理运行状态；`Worker::stop` 设置停止标志并等待运行集合为空。

上游方面，RustCodeGraph 的精确 callers 查询在本次分析中未能在限定时间内返回；用仓库文本检索补核后，生产 Rust 代码中没有 `Refresher::new` 或 `analyze_highest_priority_tables` 调用，调用者仅见于 [`refresher_test.rs`](refresher_test.rs)。上层 [`../autoanalyze.rs`](../autoanalyze.rs) 的 `StatsAnalyze<R>` 目前只依赖其本地 `PriorityRefresher` trait，因此与本实现之间仍缺适配层。作为 Go 对照，生产上游位于 [`../autoanalyze.go`](../autoanalyze.go)：`NewStatsAnalyze` 构造刷新器，`handleAutoAnalyze` 在优先队列功能开启时驱动它。

## 错误处理与边界

- `JobSource::initialize` 和 `JobSource::process_dml_changes` 的 `String` 错误由调度入口原样传播。初始化错误发生时不设置 `initialized`，测试 `test_initialization_error_can_be_retried` 证明第二次调用会重新执行初始化。
- 时间窗口设置对上界做显式校验，失败返回固定错误字符串 `invalid auto analyze time window`，且不会改写原窗口。测试覆盖 `start == 1440` 和 `end > 1440`。
- 所有 `Mutex` 获取都使用 `expect("... mutex poisoned")`。因此锁中 panic 会使后续访问继续 panic，而不是转换为 `Result`；这是当前明确的失败策略。
- 关闭后的调度是无错误空操作。`close` 本身幂等，但没有实现 `Drop`，调用者若不显式关闭，仍需自行保证 Worker 生命周期被妥善收束。
- 对运行中同表的队列项，调度器直接跳过且不重新入队。这和“提交竞争失败进入 `must_retry`”是不同路径；若未来要求保留这类作业，需要明确其新鲜度和去重规则。
- `Worker` 只以布尔值报告提交成功；线程创建失败与槽位/停止拒绝在此处统一进入重试列表。本文件无法区分永久错误与瞬时错误。
- 当前 Rust 实现没有 Go 版本的 `ValidateAndPrepare`、队列 `Rebuild`、DDL handler、解析 session 参数、日志与断言路径。扩展时不能把这些遗漏当作已经由本文件隐式覆盖。

## 并发与资源生命周期

虽然 Go 文件明确标注其 `Refresher` 非线程安全，本 Rust 类型通过 `Arc<dyn JobSource + Send + Sync>`、互斥锁和原子变量提供了可共享的并发结构。首次初始化采用双重检查：快速路径和锁内复检都用 Acquire，成功发布使用 Release；关闭队列也持有 `initialize_lock`，避免和首次初始化同时修改初始化状态。

队列锁的持有范围较短：每次弹出后即释放，提交 Worker 时不持有 `queue`；重试回队先释放 `must_retry` 锁，再取得 `queue` 锁，避免同时持有两把数据锁。`close_priority_queue` 则在初始化锁下依次清普通队列、重试队列并重置状态。当前代码没有显式规定多个线程同时调用核心调度入口的串行语义；它们可能各自基于不同时间点取得运行集合和剩余槽位，最终由 Worker 的 CAS 兜底，失败作业进入重试列表。

`Worker::submit_job` 成功后在独立线程执行 `AnalysisExecutor::analyze`；无论执行器返回 `Err` 还是 panic，Worker 都会移除表 ID、递减活跃数并通知条件变量。`wait_finished` 阻塞到运行集合为空。`close_priority_queue` 不等待也不取消运行作业，Rust 测试 `test_close_priority_queue_resets_queue_but_not_worker` 明确验证已提交作业仍会完成；`close` 则调用 `Worker::stop` 并等待清空。

一个需要维护者关注的竞态边界是：核心入口在循环前只获取一次 `running_jobs` 快照，不会把本轮刚提交的表 ID 写回该本地集合。因此同一轮队列内若存在同表重复作业，第二个重复项仍会交给 Worker 尝试提交；Worker 的 `running` 是 `HashSet`，但 `submit_job` 本身不以插入结果拒绝重复表。当前数据源和测试没有为此建立去重保证。

## 与 Go 版本的对应关系

Rust 与 [`refresher.go`](refresher.go) 的对应骨架如下：`Refresher::new` 对应 `NewRefresher`，`update_concurrency` 对应 `UpdateConcurrency`，`analyze_highest_priority_tables` 对应 `AnalyzeHighestPriorityTables`，快照/运行集合/等待/长度/关闭接口也有同名或同义方法。两边都坚持“初始化队列先于时间窗口判断”，都按剩余并发槽位从优先队列取作业，并跳过当前正在运行的表；“只关闭队列不停止 Worker”也保持一致。

但这不是完整的一比一移植：

- Go 构造器直接接收 `StatsHandle`、`sysproctrack.Tracker`、DDL notifier，创建真实 priority queue/worker，并注册 DDL handler；Rust 用 `JobSource`/`AnalysisExecutor` 抽象，未接真实依赖。
- Go 每轮从 session 读取自动分析比例、分区裁剪模式和时间参数；配置变化会 `Rebuild` 队列。Rust 时间窗口由调用方以分钟显式设置，没有比例/裁剪模式状态。
- Go 作业出队后调用 `ValidateAndPrepare(sctx)`，可丢弃表已删除或当前不适合分析的作业；Rust 只检查同表是否正在运行。
- Go 的 DML 消费和 must-retry 回队主要是测试时由上层 `handleAutoAnalyze` 显式调用；Rust 核心入口每个窗内轮次都会调用两者。
- Go 提交失败只记录日志和断言预期，不在 `Refresher` 自身保存失败作业；Rust 使用本地 `must_retry` 向量补偿槽位竞态。
- Go `Close` 的顺序是先停止 Worker、再关闭队列；Rust 是先清队列、再停止并等待 Worker。
- Go 错误通常记录日志并返回 `false`；Rust 数据源和窗口设置暴露 `Result<_, String>`，锁中毒则 panic。

Go 测试 [`refresher_test.go`](refresher_test.go) 覆盖真实 mock store、统计 delta、分区优先级、并发执行、删除表和失败分析历史等集成语义；Rust 测试只用内存 `QueueSource` 与 `RecordingExecutor` 验证调度核心。维护时应把 Go 测试看作完整产品语义的对照，但不能据其通过情况推断 Rust 已具备对应能力。

## 扩展指南

- **接入 Rust 主链**：最可能需要在本 crate或上层为 `Refresher` 提供 `PriorityRefresher` 适配，并实现真实 `JobSource`/`AnalysisExecutor`；同时把相关 Cargo 依赖移出 `cfg(any())`。这属于跨文件接线，必须同步核对 [`../autoanalyze.rs`](../autoanalyze.rs)、两个 crate 的 `Cargo.toml` 和独立测试，不能只改本文件。
- **增加配置重建语义**：若对齐 Go 的 ratio/prune mode 变化，应在初始化状态旁记录最后配置，并为重建失败、窗外重建和 DDL 进度增加独立测试；不要用简单清堆代替 priority queue 的真实 `Rebuild`。
- **增加作业合法性校验**：应在出队与提交之间引入明确接口，区分永久丢弃、可重试和可执行，避免所有失败都进入无界重试。需要覆盖表删除、统计变化和失败分析历史等 Go 测试场景。
- **修改优先级/去重**：修改 `QueuedJob::cmp` 时同步扩展 [`refresher_test.rs`](refresher_test.rs)，至少覆盖优先级相同的 `table_id` 次序和重复表；若加入去重索引，要保持入队、出队、关闭、重试回队之间的一致性。
- **修改并发策略**：关注“运行集合快照—计算槽位—提交”的竞态。若支持并发调用核心入口，宜在 Worker 层原子保证同表不重复，而不只依赖刷新器快照。同步检查 [`worker.rs`](worker.rs) 与 [`worker_test.rs`](worker_test.rs)。
- **修改时间窗口**：保留左闭右开和跨午夜语义，并明确 `start == end` 的含义（当前是空窗口）；测试应覆盖 0、1440、相等边界与非法 minute 输入策略。
- **修改关闭行为**：维持 `close_priority_queue` 与 `close` 的区别；新增可重开或取消语义时，需明确原子状态机、正在运行作业的归属及等待规则。

性能风险主要来自锁竞争、重复作业和无界队列增长；兼容风险主要来自改变同优先级顺序、时间窗口边界或错误返回语义；正确性风险主要来自把当前隔离 Rust 实现误接成完整 Go 等价实现而遗漏配置重建、DDL 事件和作业校验。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件；目标目录的 `refresher.rs`、`worker.rs`、对应 Rust/Go 测试及实现均在索引中。
- RustCodeGraph `node --file ... --symbols-only`：确认本文件包含 `QueuedJob`、`JobSource`、`Refresher` 及 17 个相关 impl 方法；完整文件读取确认源码共 248 行、无条件编译项。
- RustCodeGraph `callees analyze_highest_priority_tables`：确认核心入口的直接内部调用边；对 `initialize_queue`、`process_dml_changes`、`update_concurrency`、`requeue_must_retry` 的查询进一步确认 `JobSource` 和 `QueuedJob` 边。
- RustCodeGraph 精确 callers 查询未在限定时间内返回，随后用 `rg` 复核：Rust 的构造与核心调度调用仅位于 [`refresher_test.rs`](refresher_test.rs)，未发现生产接线；这一限制已在“文件定位”和“依赖与调用关系”中明确记录。
- 已读 Rust 路径：[`refresher.rs`](refresher.rs)、[`worker.rs`](worker.rs)、[`lib.rs`](lib.rs)、[`refresher_test.rs`](refresher_test.rs)、上层 [`../autoanalyze.rs`](../autoanalyze.rs)。
- 已读配置路径：本 crate [`Cargo.toml`](Cargo.toml) 与上层 [`../Cargo.toml`](../Cargo.toml)，用于确认 crate 边界、移植元数据和 `cfg(any())` 接线状态。
- 已读 Go 对照：[`refresher.go`](refresher.go)、[`refresher_test.go`](refresher_test.go)、[`../autoanalyze.go`](../autoanalyze.go)，用于核对真实生产入口、初始化时序、并发提交、关闭行为及未移植能力。
- Rust 独立测试提供的事实包括：窗外仍初始化、初始化前忽略 DML、初始化错误可重试、按优先级填满动态并发槽位、初始化后同轮消费 DML、跨午夜窗口与非法边界、只关队列不停止 Worker。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰好包含 11 个固定二级章节，并人工检查所有“已支持”表述均限定在当前源码能够证明的范围内。
