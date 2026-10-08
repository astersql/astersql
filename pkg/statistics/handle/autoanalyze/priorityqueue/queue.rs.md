# [`pkg/statistics/handle/autoanalyze/priorityqueue/queue.rs`](queue.rs)

## 文件定位

本文件实现 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 中的自动 ANALYZE 优先队列状态机。crate 入口 `pkg/statistics/handle/autoanalyze/priorityqueue/lib.rs` 以 `mod queue` 装配本文件并通过 `pub use queue::*` 导出其 API；底层堆、权重计算和作业协议分别来自同 crate 的 `heap.rs`、`calculator.rs` 与 `job.rs`。

从当前 Rust 接线看，它是一个可独立使用的队列实现，而不是已经接入完整服务主链的唯一实现：`pkg/statistics/handle/autoanalyze/refresher/refresher.rs` 目前维护自己的 `BinaryHeap`，其 `Cargo.toml` 对本 crate 的依赖还在 `[target.'cfg(any())'.dependencies]` 下，恒不启用。仓库内可执行的直接使用者主要是 `queue_test.rs` 和 `queue_ddl_handler_test.rs`；`queue_ddl_handler.rs` 则在同一类型上补充 DDL 事件入口。因而本文区分“本文件已经实现的行为”和“Go 版本已有、Rust 尚未完成的生产接线”。

## 核心职责

- `AnalysisPriorityQueue` 把 `QueueSource` 提供的候选作业装入 `PqHeapImpl`，通过 `PriorityCalculator::CalculateWeight` 计算优先级，并以表或分区 ID 去重/更新。
- 队列维护三类互斥或相关状态：仍在堆中的待执行作业、已经 `Pop` 的 `running_jobs`、执行失败后需要重建的 `must_retry_jobs`（`QueueState`）。
- `Initialize` 完成首次全量构建并启动一个后台线程；线程周期性调用 `ProcessDMLChanges`、`RequeueMustRetryJobs` 和 `RefreshLastAnalysisDuration`。
- 作业完成钩子负责把 ID 从运行集合移除，并在失败且 `must_retry == true` 时加入重试集合（`hooks`）。
- `DeleteByTableID` 与 `RecreateAndPushJob` 为 `queue_ddl_handler.rs` 的 DDL 处理提供删除和重建原语。
- `Close`/`Drop` 管理后台线程退出和共享状态清理，避免最后一个队列句柄释放后遗留线程。

## 主要符号

- 常量 `NOT_INITIALIZED_ERR_MSG`：需要初始化的 API 的统一错误文本。三个刷新间隔分别是 `LAST_ANALYSIS_DURATION_REFRESH_INTERVAL`（10 分钟）、`DML_CHANGES_FETCH_INTERVAL`（2 分钟）和 `MUST_RETRY_JOB_REQUEUE_INTERVAL`（5 分钟）；`SLOW_LOG_THRESHOLD` 只为与 Go 常量对齐，本文件没有日志计时逻辑。
- trait `QueueSource: Send + Sync + 'static`：把完整 TiDB 统计元数据访问隔离到队列外。`build_analysis_jobs` 是唯一必需实现；`changed_analysis_jobs`、`recreate_job`、`refreshed_indicators` 默认返回空结果，便于测试和渐进接线。
- `QueueState`：由 `Mutex` 保护的可变数据，包括 `heap`、`initialized`、`running_jobs` 和 `must_retry_jobs`。其 `Default` 创建空堆并保持未初始化。
- `WorkerControl`：保存退出信道 `Sender<()>` 和 `JoinHandle<()>`，二者共同决定 worker 是否已经启动以及如何等待退出。
- `AnalysisPriorityQueue`：外部可克隆句柄。`source`、`calculator`、`state`、`worker` 均由 `Arc` 共享；克隆句柄操作的是同一队列和同一个 worker。
- `NewAnalysisPriorityQueue(Arc<dyn QueueSource>)`：只构造空队列，不加载作业；调用方仍须调用 `Initialize`，或在明确已完成状态构建后调用 `Run`。
- 初始化 API：`Initialize` 幂等地拉取全量作业、调用 `RebuildWithoutLock` 并启动 worker；`Rebuild` 只允许在已初始化状态全量替换；`FetchAllTablesAndBuildAnalysisJobs` 直接委托数据源。
- 入队 API：`Push`、`PushWithoutLock`、`TryCreateJob`、`TryUpdateJob`、`ProcessTableStats` 最终汇聚到 `push_locked`。尽管命名保留 Go 形状，`PushWithoutLock` 在 Rust 中仍会自行加锁。
- 后台刷新 API：`ProcessDMLChanges` 获取 running 快照后请求增量作业；`RequeueMustRetryJobs` 消耗重试 ID 并尝试重建；`RefreshLastAnalysisDuration` 仅替换现有指标的 `LastAnalysisDuration` 后重算权重。
- 调度与观察 API：`Pop`、`GetRunningJobs`、`Len`、`Snapshot`，以及测试用途的 `PeekForTest`、`IsEmptyForTest`。
- 生命周期 API：`Close` 退出并 join worker 后清空状态；`ResetSyncFields` 只清状态、不停止 worker；`Drop` 在最后一个 `worker` 强引用释放时调用 `Close`。
- `RunningJobs = HashMap<i64, ()>`：与 Go 的 `map[int64]struct{}` 形状对齐的公开别名；内部实际使用 `HashSet<i64>`。

## 执行流程

1. 调用方用 `NewAnalysisPriorityQueue` 注入 `QueueSource`。此时 `initialized == false`，堆和两个 ID 集合为空。
2. `Initialize` 先在锁内检查幂等条件，然后在锁外调用 `QueueSource::build_analysis_jobs`。成功后 `RebuildWithoutLock` 获取状态锁，重置旧堆和集合，并对每个作业执行 `push_locked`：计算权重、设置权重、注册成功/失败钩子、交给 `PqHeapImpl::AddOrUpdate`。全部成功后才写入 `initialized = true`，最后启动 worker。
3. `start_worker` 在 `WorkerControl` 锁下防止重复启动，建立 MPSC 退出信道，并每秒通过 `recv_timeout` 醒来累计三组 elapsed 时间。到期时分别调用 DML 增量、失败重试和时长刷新；这些后台调用的错误当前被显式丢弃。
4. 调度方调用 `Pop`：在状态锁内验证初始化、从最大堆取最高权重作业、把 ID 加入 `running_jobs`，再返回拥有作业的 `Box`。作业在入堆时已装好钩子；成功结束会清除 running ID，失败结束会清除 running ID，并按 `must_retry` 参数决定是否加入重试集合。
5. `ProcessDMLChanges` 先复制运行集合，再让数据源构造增量作业；重新持锁并确认仍处于初始化状态后，只把不在 running 集合中的作业交给 `push_locked`。相同 key 的待执行作业由堆的 `AddOrUpdate` 覆盖更新。
6. `RequeueMustRetryJobs` 先复制重试 ID。每个 ID 在尝试 `recreate_job` 之前就从重试集合移除；数据源返回作业且 ID 当前不在运行集合时重新入堆。缺表、返回 `None` 或重建报错都不会永久保留该标记，后续只能由新的 DML/DDL/失败事件再次产生。
7. `RefreshLastAnalysisDuration` 复制当前堆 key，逐个向数据源取新指标。找到作业后先 `DeleteByKey`，只复制新值的 `LastAnalysisDuration`，保留变化比例、表大小和既有钩子，再重算权重并 `Update` 回堆。
8. DDL 路径由 `queue_ddl_handler.rs::HandleDDLEvent` 进入：删除类事件调用 `DeleteByTableID`，需重建的事件经 `RecreateAndPushJobForTable` 到达 `RecreateAndPushJob`，后者先删旧项再向 `QueueSource` 请求新作业。
9. `Close` 先从 `WorkerControl` 取走并 drop 发送端，使 worker 在下一次信道检查时退出；它在不持有 worker 锁的情况下 join，然后清空堆和集合并设为未初始化。之后允许再次 `Initialize`。

## 数据与状态

`QueueState` 是队列的一致性边界，所有读写都经 `lock_state` 或持有等价的 `MutexGuard`：

- `heap` 保存可调度作业，key 由作业的表/分区 ID 提供，权重决定弹出次序。
- `running_jobs` 记录已离开堆但尚未通过作业钩子报告完成的 ID。`Push` 和 DML 刷新会跳过这些 ID，避免同一对象并发分析。
- `must_retry_jobs` 只保存 ID，不保存旧作业；重试时必须由 `QueueSource::recreate_job` 根据当前元数据重建。
- `initialized` 是 API 可用性门闩，不等同于 worker 一定存在：`RebuildWithoutLock` 可直接置真，而 `Run` 也可单独尝试启动线程。正常公共路径应使用 `Initialize` 保持两者一致。

`Snapshot` 返回 `(Vec<AnalysisJobJSON>, running, must_retry)` 的克隆快照，不暴露内部锁和作业对象。其作业向量来自 `heap.List()`，本文件没有像 Go `Snapshot` 那样额外按权重排序，因此调用者不应把向量顺序当成稳定接口。`GetRunningJobs` 同样返回集合副本。

`QueueSource` 的默认空实现意味着“成功但无变化”，而不是“能力未实现”的错误。生产适配器若遗漏可选方法，后台更新会静默不做事；这是接线时必须显式审查的状态语义。

## 依赖与调用关系

下游依赖均在同 crate 内：

- `calculator::PriorityCalculator` 为入队和时长刷新计算权重。
- `heap::PqHeapImpl` 提供 `AddOrUpdate`、`Pop`、`Peek`、按 key 删除/查找以及列表快照。
- `job::AnalysisJob` 提供 ID、指标、权重、JSON 视图和完成钩子；`SuccessJobHook`/`FailureJobHook` 是跨作业生命周期回调类型。
- 标准库的 `Arc`/`Weak`/`Mutex`、`mpsc` 和 `JoinHandle` 实现共享所有权、无循环钩子引用、退出通知和线程回收。

上游边界如下：

- `priorityqueue/lib.rs` 公开导出本文件符号；根 `pkg/lib.rs` 又通过 statistics facade 暴露该 crate。
- `queue_ddl_handler.rs` 在 `AnalysisPriorityQueue` 上调用 `DeleteByTableID` 和 `RecreateAndPushJob`，形成已实现的 DDL 局部调用链。
- `queue_test.rs` 直接覆盖构造、初始化、重建、Pop/钩子、重试、指标刷新与关闭。
- RustCodeGraph 的精确查询确认 `NewAnalysisPriorityQueue`、`ProcessDMLChanges`、`RecreateAndPushJob` 等符号位于本文件；其 caller 图查询在本次检查中未在 30 秒内返回结果，因此上游 Rust 接线另以仓库 `rg` 搜索和 Cargo 声明交叉核验。
- 当前 `refresher` crate 对 priorityqueue 的依赖被 `cfg(any())` 禁用，且 `refresher.rs` 使用自己的队列类型；所以不能宣称本文件已被自动 ANALYZE 生产循环调用。

`priorityqueue/Cargo.toml` 仅声明一个常规依赖 `astersql-statistics-handle-logutil`，但本文件本身未直接引用它；本文件直接使用的业务类型都来自 crate 内模块和标准库。Cargo 元数据把 Go 对照包标为 `pkg/statistics/handle/autoanalyze/priorityqueue`。

## 错误处理与边界

- `Rebuild`、`Push`、`ProcessDMLChanges`（取回数据后）、`RequeueMustRetryJobs`、`RefreshLastAnalysisDuration`、`Pop`、`PeekForTest`、`IsEmptyForTest`、`Len`、`Snapshot` 和删除 API 在未初始化时返回 `NOT_INITIALIZED_ERR_MSG`。`IsInitialized` 与 `GetRunningJobs` 可在未初始化时读取空/当前快照。
- `Initialize` 在数据源全量构建或任一入堆动作失败时返回 `String` 错误；由于 `RebuildWithoutLock` 会先清空状态，入队到一半失败会留下部分堆但 `initialized` 仍为 false。调用方不能把失败后的内部内容视为可用队列。
- `ProcessDMLChanges` 在调用数据源时不持有状态锁，减少外部工作占锁时间；随后会重新检查初始化。其 running 快照可能在外部调用期间过时，但入队前还会以锁内最新集合再检查一次。
- `RequeueMustRetryJobs` 对一个 ID 的数据源错误会立即中止剩余循环，且该 ID 已从集合删除；这与源码注释所述 Go 的“先消费标记”一致，但意味着错误不会自动重试。
- `RefreshLastAnalysisDuration` 对数据源返回 `None`、或并发状态变化导致 `DeleteByKey` 失败的项直接跳过；`Update` 失败则向上传播。数据源调用发生在未持锁阶段，删除、修改和恢复在锁内完成。
- `lock_state` 及 worker 锁在 mutex poison 时通过 `into_inner` 继续使用旧数据，选择可恢复性而非 panic；这并不证明被 panic 中断的不变量一定完整，诊断层仍应记录先前 panic。
- worker 对三个周期任务的 `Result` 均执行 `let _ = ...`，没有日志、退避或停止策略；`SLOW_LOG_THRESHOLD` 也未使用。生产接线应补充可观察性，不能依赖调用者看到这些后台错误。
- 空堆时的 `Pop`/`Peek` 错误由 `PqHeapImpl` 定义；本文件只传播字符串。权重为零、负数或异常值时，本文件不做 Go 版本的告警或过滤检查，而是直接写入堆。

## 并发与资源生命周期

`AnalysisPriorityQueue` 可跨线程克隆，`QueueSource` 被要求 `Send + Sync + 'static`。一个 `Mutex<QueueState>` 串行化堆与两个集合的所有状态转换，另一个 `Mutex<WorkerControl>` 串行化线程启停。外部数据源操作通常放在状态锁外；实际堆修改在状态锁内。

`Initialize` 的“检查 initialized”与后续全量拉取/重建不是由同一次持锁覆盖，因此它对顺序重复调用幂等，但不能据此推断并发 `Initialize` 与 `Close` 已被完整串行化。若生产层允许这两类生命周期操作并发，需要在调用层互斥或为本类型增加专门的生命周期状态；Go 源码也明确警告避免 `Initialize`/`Close` 并发。

worker 使用 1 秒 tick 累计时间，而不是三个独立 ticker。退出有两种触发：`Close` drop 发送端导致 `Disconnected`，或（内部若持有发送者发送）收到 `Ok(())`；当前代码只通过 drop 使用它。`Close` 在取走句柄后才 join，不持有 worker 控制锁等待；worker 周期函数所需的 state 锁也不会与 join 时的锁形成直接闭环。

完成钩子捕获 `Weak<Mutex<QueueState>>`，不会让作业反向延长队列寿命。队列已 drop 时 `upgrade` 失败，钩子安全地不做事；`queue_test.rs::complete_job_hooks_clear_running_and_requeue_failures` 还覆盖了队列 drop 后再次触发成功回调的场景。

`Drop` 通过 `Arc::strong_count(&self.worker) == 1` 判断是否为共享 worker 的最后一个句柄，并调用 `Close`。显式并发 `Close` 会由 worker 锁串行取得同一个句柄，后来的调用只做状态清理；不过 `ResetSyncFields` 不停止线程，若用于生产重置，后台线程仍会继续周期调用并得到未初始化错误。该方法应限制在测试或受控重建场景。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/autoanalyze/priorityqueue/queue.go`。Rust 保留了队列类型、主要公开方法名、三个周期、堆/running/must-retry 三类状态，以及“重试前先删除标记”“Close 后可重新 Initialize”等核心意图，但不是逐语句等价移植：

- Go 持有真实 `StatsHandle`、session context 和 DML 版本游标；Rust 把全量构建、增量选择、单项重建、指标刷新抽象为 `QueueSource`，自身没有 `lastDMLUpdateFetchTimestamp`，是否漏/重放 DML 由适配器负责。
- Go `Initialize(ctx)` 检查取消、保存可取消 context 并在出错时 Close；Rust `Initialize()` 没有 context 或取消传播，只在数据源错误时返回。
- Go 在 `Pop` 时注册完成钩子；Rust 在每次 `push_locked` 时注册。Rust 的钩子使用 `Weak` 处理队列释放，达到 Go 在 owner switch 后允许运行中作业结束的相似安全目标。
- Go `pushWithoutLock` 遇到 running ID 时还会写入 `mustRetryJobs`，并会跳过已在 must-retry 集合的作业；Rust `Push`/`ProcessDMLChanges` 只跳过 running ID，不新增重试标记，也不因 ID 已在 must-retry 集合而拒绝普通入队。这是实际行为差异，不应由文档掩盖。
- Go DML 处理扫描统计缓存、过滤版本/锁表并逐项记录错误；Rust 只消费 `QueueSource::changed_analysis_jobs` 的结果，首个入堆错误会终止本批。
- Go 刷新时若表统计消失会删除作业；Rust 的 `refreshed_indicators == None` 只保留原作业。两者都只更新 `LastAnalysisDuration` 并重算权重。
- Go `Snapshot` 返回当前作业和 must-retry ID，并显式按权重降序排序；Rust 还返回 running 集合，但未在本文件排序 JSON 列表。
- Go 有慢操作日志、采样错误日志、panic recovery 与 failpoint 并发验证；Rust worker 当前吞掉周期错误，且没有对应 failpoint。Rust 测试文件中大段注释保留了尚未接线的 Go 集成测试意图，不能算可执行覆盖。

可执行 Rust 测试已确认的语义包括：初始化前 `Rebuild` 报统一错误，空源可初始化/关闭/再初始化；缺失表的重试标记被消费；刷新只改变上次分析时长并重算权重；成功/失败钩子清理 running，失败可重建入队，以及队列 drop 后弱引用钩子安全。

## 扩展指南

- 接入真实统计系统时，优先新增独立的 `QueueSource` 实现，不要把 session、InfoSchema 或 StatsHandle 访问重新塞入状态机。适配器必须明确实现 DML 游标推进、锁表过滤、分区模式、缺表语义和错误可观察性，并在独立测试文件覆盖。
- 若要与 Go 完全对齐 running/must-retry 行为，应聚焦修改 `Push`/`push_locked`/`ProcessDMLChanges`，先确定 running 作业遇到新索引或新 DML 时是否必须设置重试标记；同步扩展 `queue_test.rs`，不要把测试内嵌进 `queue.rs`。
- 调整优先级或指标时，权重公式应改在 `calculator.rs`，作业指标协议改在 `job.rs`；本文件只负责在 `push_locked` 和 `RefreshLastAnalysisDuration` 的正确状态转换点重算，并同步 `calculator_test.rs`、`heap_test.rs` 与 `queue_test.rs`。
- 新增周期任务需同时考虑 `start_worker` 的计时、错误处理、Close 延迟和测试可控性。当前常量固定且 tick 为一秒，直接加入耗时任务会串行阻塞其他三个维护动作。
- 扩展快照对外契约时，应先决定稳定排序和兼容格式；若要求与 Go 一致，应显式排序而非依赖 heap 内部布局，并为 `Snapshot` 增加独立断言。
- 修改 DDL 行为时，接入点是 `DeleteByTableID`/`RecreateAndPushJob` 与 `queue_ddl_handler.rs`；需同步 `queue_ddl_handler_test.rs`，并验证未初始化时 DDL 事件的 readiness 语义。
- 修改线程生命周期时，必须保持“等待 worker 时不持有 worker/state 锁”、钩子只持有 `Weak`、重复 `Close` 不 panic、Close 后可再初始化等不变量。相关测试仍放在 `queue_test.rs`；Go 注释中的并发 Close 用例目前不可执行，若补齐应转成真正的 Rust 测试而不是据注释宣称已覆盖。
- 正式把本 crate 接入 `refresher` 时，需要先处理当前两套优先队列的职责重叠，并移除或替换 `cfg(any())` 依赖门。此工作超出本单文件文档任务，不能仅靠调用 `NewAnalysisPriorityQueue` 视为完成。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/statistics/handle/autoanalyze/priorityqueue` 显示目标 Rust/Go/测试文件均被索引。
- RustCodeGraph 文件节点：完整读取 `queue.rs` 第 1–474 行；查询确认 `queue.rs::NewAnalysisPriorityQueue`、`queue.rs::ProcessDMLChanges`、`queue.rs::RecreateAndPushJob` 的签名和位置；读取 `queue_ddl_handler.rs` 第 60–228 行核对 DDL 调用链。精确 caller/callee 命令曾执行，但未在 30 秒限制内产出边，故未把其缺失结果当作“无调用者”。
- crate 与接线证据：读取 `priorityqueue/Cargo.toml`、`priorityqueue/lib.rs`、`refresher/Cargo.toml` 和 `refresher/refresher.rs`；仓库搜索核对 facade 导出、`cfg(any())` 依赖及实际 Rust 引用。
- Go 对照：通过 RustCodeGraph 读取 `queue.go` 第 380–789、960–1274 行，核对初始化、全量构建、后台循环、DML 游标、重试、指标刷新、钩子、快照和关闭语义。
- 测试证据：通过 RustCodeGraph 读取 `queue_test.rs` 第 1–520、760–1064 行。可执行测试位于文件末部；前部 Go 形状的大段内容为注释迁移记录，本文没有将其当成已经执行的 Rust 测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg -c` 命令校验本文恰有十一个固定二级标题，并人工复核唯一生产物、源码链接、已实现/未接线边界和独立测试位置。
