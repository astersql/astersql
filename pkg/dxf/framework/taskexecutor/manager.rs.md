# `pkg/dxf/framework/taskexecutor/manager.rs`

## 文件定位

`manager.rs` 实现 DXF（Distributed eXecution Framework）在单个执行节点上的任务执行器管理器。仓库的 `pkg/dxf/framework/doc.go` 将 task executor manager、slot manager 和 task executor 列为所有 DXF 节点都运行的组件；其中本文件位于 `astersql-dxf-framework-taskexecutor` crate，由 `lib.rs` 的 `mod manager` 纳入并通过 `pub use manager::*` 对外再导出。

当前 Rust 主链中的直接生产入口是 `pkg/session/runtime/modify_column_dist_backfill.rs` 的 `NodeService::start` 路径：它先注册 Backfill 类型工厂，再调用 `NewManager`，随后依次调用 `InitMeta` 和 `Start`；`NodeService::stop`/`Drop` 最终调用 `Manager::Stop`。因此本文件负责“节点级调度与生命周期编排”，具体 subtask 的循环和业务执行仍由 `TaskExecutor`（通常为 `BaseTaskExecutor`）负责。

`pkg/dxf/framework/taskexecutor/Cargo.toml` 声明 crate 名为 `astersql-dxf-framework-taskexecutor`、库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/dxf/framework/taskexecutor"` 标明 Go 对照包。当前 manifest 中只有 `astersql-lightning-log` 是普通依赖，其余大量依赖位于 `target.'cfg(any())'`，该条件恒假；本文件自身实际使用 `std` 和 crate 内再导出的接口，不应根据那组禁用依赖推断运行时接线。

## 核心职责

1. `NewManager` 按节点 `NodeResource.TotalCPU` 创建本地 `slotManager`，保存任务表、节点 ID、取消上下文与资源快照。
2. `InitMeta` 初始化节点元数据；`recoverMetaLoop` 每 90 秒调用 `recoverMeta`，用于刷新或补回节点记录。
3. `handleTasksLoop` 按 `TaskCheckInterval`（默认 300 ms）轮询 `TaskTable::GetTaskExecInfoByExecID`，把任务状态分派为启动、暂停或回滚处理。
4. `handleExecutableTasks` 在任务表给定的顺序上询问 slot 管理器；需要抢占时先取消低排名执行器并结束本轮，资源无需抢占但暂时不足时继续尝试后续任务。
5. `startTaskExecutor` 完成完整任务加载、slot 占用、keyspace runtime 获取、工厂查找、初始化、登记、异步运行和退出清理。
6. `Stop` 取消管理器上下文、回收所有已登记工作线程，并对仍登记的执行器调用 `Close`。

该文件不决定任务的全局调度，也不直接执行业务 subtask；前者属于 scheduler，后者通过 `TaskExecutor`/`StepExecutor` 抽象下沉到具体任务类型。

## 主要符号

- `taskCheckIntervalNanos: AtomicU64`、`TaskCheckInterval()`、`SetTaskCheckIntervalForTest()`：以纳秒原子值保存可测试的轮询周期。读取使用 `Acquire`，写入使用 `Release`；setter 返回旧值，供 `main_test.rs` 的守卫恢复全局状态。
- `recoverMetaInterval: Duration`：固定为 90 秒的节点元数据恢复周期。
- `RuntimeLease(Option<Arc<dyn TaskRuntime>>)`：runtime holder 的 RAII 所有者。`Drop` 无条件对存在的 runtime 调用 `Release`，使未找到工厂、`Init` 失败和正常 `Run` 退出三条路径都只需依赖作用域释放。
- `Manager`：节点管理器。公开字段是 `taskTable`、`id` 和 `slotManager`；内部以 `Mutex<HashMap<i64, Arc<dyn TaskExecutor>>>` 维护任务 ID 到执行器的映射，以 `Mutex<Vec<JoinHandle<()>>>` 维护后台线程，以 `Context` 统一取消。
- `NewManager(...) -> Result<Arc<Manager>>`：构造共享管理器。当前实现没有可能返回错误的分支，但保留 `Result` 以匹配调用约定。
- `InitMeta`、`recoverMeta`、`runWithRetry`：元数据操作及其最多三次的重试框架。
- `Start`、`Cancel`、`Stop`、`waitForInterval`：管理器线程和取消生命周期。
- `handleTasksLoop`、`handleTasks`、`handleExecutableTasks`：轮询、状态分派和 slot 决策主链。
- `handlePausingTask`、`handleRevertingTask`、`cancelRunningSubtaskOf`、`cancelTaskExecutors`：暂停、回滚和抢占时的不同取消语义。
- `startTaskExecutor`：单任务启动事务的核心函数，返回值只表示本轮是否成功启动执行器。
- `addTaskExecutor`、`delTaskExecutor`、`isExecutorStarted`：执行器注册表操作与重复启动保护。
- `failSubtask`：只把不可重试的初始化/配置错误写为 subtask 失败。
- `DefaultExtension`：本文件构造 `Param` 时提供的默认扩展；它声明幂等、返回空操作 `BaseStepExecutor`、且所有错误均不可重试。具体工厂可以像 `modify_column_dist_backfill.rs` 那样替换 `Param.Extension`。

## 执行流程

启动路径如下：

1. 上层创建 `TaskTable` 和 `NodeResource`，调用 `NewManager`。
2. 上层调用 `InitMeta`；它把 `TaskTable::InitMeta(ctx, id, serviceScope)` 放入 `runWithRetry`。随后 `Start` 各启动一个任务轮询线程和元数据恢复线程。
3. `handleTasksLoop` 等待轮询周期。`waitForInterval` 最多睡眠 10 ms 一次，因此 `Context::Cancel` 不需要等完整轮询周期才生效。
4. `handleTasks` 查询本节点的 `TaskExecInfo`。查询失败时本轮直接返回；`Running` 且尚未登记的任务进入可执行列表，`Pausing` 调用 `handlePausingTask`，`Reverting` 调用 `handleRevertingTask`，其他状态忽略。
5. `handleExecutableTasks` 对每个候选调用 `slotManager::canAlloc`：若返回需释放的低排名任务，调用 `cancelTaskExecutors` 后立即结束本轮，等待它们退出并真正释放 slot；若 `can == false` 且无需抢占，继续检查后续候选；若可直接分配，则调用 `startTaskExecutor`，启动失败时为保持任务顺序而结束本轮。
6. `startTaskExecutor` 重新用 ID 加载完整 `Task`，再原子占用所需 slot。之后调用 `TaskTable::AcquireTaskRuntime` 获取目标 keyspace runtime，并用 `RuntimeLease` 接管 holder 生命周期。
7. 它按 `TaskBase.Type` 调用 `GetTaskExecutorFactory`。类型未注册时写失败状态并释放 slot；找到工厂后组装 `Param`，构造 executor 并调用 `Init`。初始化失败时按可重试性决定是否 `FailSubtask`，然后释放 slot。
8. 初始化成功后先把 executor 登记到 `taskExecutors`，再创建线程调用 `Run`。线程退出顺序是 `Close`、注销执行器、释放 slot；线程局部 `_runtime` 最后离开作用域并调用 `Release`。
9. `Stop` 先取消上下文，再反复取空并 `join` 当前 `workers`。之所以循环，是因为已在运行的管理线程可能在停止期间把新 executor 线程追加到同一向量。所有线程回收后，再对注册表快照中的残余执行器调用 `Close`。

暂停与回滚有意不同：`handlePausingTask` 取消整个 executor，并且无论 executor 是否已经启动都调用 `PauseSubtasks`；`handleRevertingTask` 只调用 `CancelRunningSubtask`，随后用任务表把当前节点的 subtask 取消。抢占的 `cancelTaskExecutors` 只发出 executor 取消信号，不直接改 subtask 状态。

## 数据与状态

- `taskExecutors` 的键是任务 ID。执行器在 `Init` 成功后、线程启动前登记，在 `Run` 退出并 `Close` 后删除；`isExecutorStarted` 利用该映射防止同一轮询任务被重复启动。
- `workers` 同时保存两个长期管理线程和每个 executor 的运行线程。它不是线程池；每个已启动任务拥有独立 OS 线程。
- `slotManager` 的容量在构造时固定为 `nodeResource.TotalCPU`。`startTaskExecutor` 只有在 `alloc` 成功后才继续，所有后续失败路径都显式 `free`，正常路径由运行线程退出时释放。
- `nodeResource` 在 `getNodeResource` 中克隆后传入 `Param`，工厂收到的是快照而非对 `Manager` 内部字段的可变引用。
- `serviceScope` 当前在 `NewManager` 中固定为空字符串，`InitMeta` 和 `RecoverMeta` 都使用该值；文件内没有动态刷新 scope 的逻辑。
- `ctx` 被两个管理循环及各 executor 工厂共享。`Cancel` 是协作式取消信号；能否让一个具体 executor 的 `Run` 退出仍取决于实现是否响应其上下文或 `Cancel`。
- `TaskCheckInterval` 是进程级全局原子状态，测试通过 `main_test.rs` 的 interval guard 修改并恢复；并发修改会影响该 crate 内所有管理器的后续等待周期。

关键不变量是：只有成功分配 slot 的任务才进入 runtime/factory/init 流程；只有 `Init` 成功的执行器才加入注册表；任何已分配 slot 最终必须由失败分支或运行线程释放；runtime holder 即使被工厂保存到 `Param` 的克隆中，也由 `RuntimeLease` 负责恰好触发一次 `Release`。

## 依赖与调用关系

上游调用关系：

- `pkg/session/runtime/modify_column_dist_backfill.rs`：生产侧创建管理器、初始化元数据、启动，并在节点服务停止或析构时停止管理器；同一位置注册 Backfill factory，并根据 `TaskRuntime::AsAny` 绑定目标 domain。
- `pkg/dxf/framework/taskexecutor/manager_test.rs`：直接驱动内部方法，验证白盒状态机和资源生命周期。
- `pkg/dxf/framework/taskexecutor/main_test.rs`：使用 `SetTaskCheckIntervalForTest` 缩短并恢复轮询周期。

主要下游边：

- `TaskTable`（`interface.rs`）：`GetTaskExecInfoByExecID`、`GetTaskByID`、`AcquireTaskRuntime`、`InitMeta`、`RecoverMeta`、`PauseSubtasks`、`CancelSubtask` 和 `FailSubtask` 是本文件的持久化/运行时边界。
- `slotManager`（`slot.rs`）：`canAlloc` 决定直接分配或抢占候选，`alloc/free` 维护本节点 CPU slot。
- 工厂注册表（`register.rs`）：`GetTaskExecutorFactory` 把任务类型映射为 `FactoryFn`。
- `TaskExecutor`（`interface.rs`）：管理器调用 `Init`、`Run`、`Cancel`、`CancelRunningSubtask`、`Close`、`GetTaskBase` 和 `IsRetryableError`。
- `Param`、`TaskRuntime`、`Extension`（`task_executor.rs`/`interface.rs`）：向工厂传递任务表、slot、节点资源、节点 ID、扩展和 keyspace runtime。
- `std::thread`、`Mutex`、`Arc`、`AtomicU64`：分别承担执行并发、共享状态互斥、跨线程所有权和全局周期设置。

RustCodeGraph 对 `manager.rs` 的文件节点报告一个文件级使用方 `pkg/dxf/importinto/write_ingest_backend.rs`，但该文件实际引用的是 task executor execute 接口，并未形成对 `Manager` 符号的直接调用；精确生产调用以 `NewManager` 的仓库搜索结果和 `modify_column_dist_backfill.rs` 源码为准。图查询确认的内部边包括 `Start -> handleTasksLoop/recoverMetaLoop`、`handleTasksLoop -> waitForInterval/handleTasks`、`handleTasks -> handleExecutableTasks/handlePausingTask/handleRevertingTask/isExecutorStarted`，以及 `startTaskExecutor -> GetTaskExecutorFactory/getNodeResource/addTaskExecutor/delTaskExecutor/failSubtask`。

## 错误处理与边界

- `handleTasks` 获取任务列表失败时静默跳过本轮；暂停/回滚处理的错误也在分派处被丢弃。当前 Rust 文件没有 Go 版本对应的日志、采样日志、trace 或指标更新，因此故障可观测性较弱。
- `GetTaskByID`、slot 分配和 runtime 获取失败都会让 `startTaskExecutor` 返回 `false`。runtime 获取失败会输出一条 `eprintln!`，不调用 `FailSubtask`，从而保留持久化 subtask 供后续轮询重试。
- 未注册类型属于不可执行配置错误：`failSubtask` 写入失败；工厂构造后的 `Init` 错误仅在 `TaskExecutor::IsRetryableError` 返回 `false` 时写失败。无论哪种启动失败，slot 都会释放。
- `runWithRetry` 最多调用三次，退避为 10、20、40 ms；若一次失败后发现上下文已取消，立即返回 `Context::Cause`，没有 cause 时返回 `"context canceled"`。第三次失败后的睡眠仍会执行，然后才返回最后错误，这是当前源码的实际行为。
- `Mutex::lock` 多数通过 `expect(... poisoned)` 处理；任一持锁线程 panic 导致锁中毒后，后续访问会 panic。executor 线程中的 `Run` 也没有 `catch_unwind`，若它 panic，顺序写在闭包尾部的 `Close`、注销和 slot 释放不会执行；这是当前实现边界，不应假定具备 Go `defer`/recover 的清理保证。
- `Duration::as_nanos() as u64` 在测试 setter 中是截断转换，极端大 duration 会截断；正常测试周期不触及该边界。
- `DefaultExtension` 是安全占位接线而非具体业务实现；若工厂未替换它，步骤执行器为 no-op。新增真实任务类型必须在 factory 中设置正确的 `Extension`，不能把默认实现当作业务已支持。

## 并发与资源生命周期

`Manager` 以 `Arc<Self>` 跨线程共享。`Start` 创建两个后台线程；每次成功启动 executor 又创建一个线程。`taskExecutors` 和 `workers` 分别有独立互斥锁，slot 管理器内部还有自己的锁。代码不会在持有 `taskExecutors` 锁时进入 slot 分配，但取消函数会在持有注册表锁时调用 executor 的取消方法；因此 executor 的 `Cancel`/`CancelRunningSubtask` 实现不应反向等待需要同一注册表锁的路径。

抢占是两阶段的：`canAlloc` 只给出应释放的低排名任务，Manager 调用它们的 `Cancel` 后结束当前扫描；slot 仍由旧 executor 占有，直到其 `Run` 退出并执行清理。下一轮扫描才可能给高排名任务分配资源。`manager_test.rs::test_slot_manager_preemption_in_manager` 明确验证取消后旧任务仍登记，只有测试驱动其退出后新任务才能取得 slot。

`RuntimeLease` 和运行线程的捕获关系把 runtime holder 的释放延迟到 executor `Run`、`Close`、注销和 slot 释放之后。测试 `test_runtime_holder_released_after_failed_start_and_executor_close` 覆盖 missing-factory、init-error 和 success 三条路径，并验证即使 factory 保留了 `Param.TaskRuntime`，holder 仍只释放一次。

`Stop` 依赖 `Context` 和各 executor 的合作式终止。`waitForInterval` 的 10 ms 分片保证两个管理循环能快速响应取消，`test_manager_stop_interrupts_background_interval_waits` 要求停止不等待 90 秒。但若具体 executor 的 `Run` 不响应取消且不自行返回，`join` 会一直等待；`Stop` 在 join 之后才对注册表快照调用 `Close`，因此不能依靠这个最终 `Close` 去解除一个仍阻塞的 `Run`。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/dxf/framework/taskexecutor/manager.go` 的 `Manager`、`NewManager`、元数据循环、任务分派、抢占、启动和失败处理，并由同目录的 `manager_test.go`/`manager_test.rs` 保持主要测试意图。共同语义包括：任务按 rank 顺序处理；需要抢占时取消低排名任务并中断本轮；不能直接分配但无需抢占时允许继续尝试低排名小任务；暂停与回滚走不同取消接口；未注册类型和不可重试初始化错误标记失败；成功 executor 退出时关闭、注销并释放 slot。

已确认的 Rust 差异如下：

- Go 构造器还保存 KV store、logger、sample logger、trace，并从全局配置读取 service scope；Rust 没有这些字段，`serviceScope` 固定为空。
- Go 的轮询循环更新 trace、slot gauge 和 global-sort worker 指标，并用 `Recover` 包装循环；Rust 没有对应可观测性和 panic 恢复。
- Go 的 runtime 通过 `dxfutil.AcquireTaskRuntime` 返回显式 `releaseFn`；Rust 把获取抽象为 `TaskTable::AcquireTaskRuntime`，用 `RuntimeLease::Drop` 释放。
- Go `Param` 在此处不设置默认 Extension；Rust 明确传入 `DefaultExtension`，允许 factory 再替换。
- Go 使用框架统一的 `handle.RunWithRetry`、scheduler 重试次数和指数退避上限；Rust 固定为三次以及 10/20/40 ms 睡眠。
- Go `Stop` 先等待 executor group 再等待管理循环，正常 executor 清理由 goroutine 内 defer 完成；Rust 把所有线程句柄放在一个向量中循环 join，随后再关闭残余登记项。
- Go `TaskCheckInterval` 是可直接重赋值的包变量；Rust 用 `AtomicU64` 和 getter/setter 模拟可跨测试修改的语义。

这些差异应视为当前移植状态，而不是自动判定为缺陷。若未来要求严格行为对齐，应分别补测试证明可观测性、重试时序、service scope 和 panic/停止清理语义，再修改生产代码。

## 扩展指南

- 新增任务类型：通过 `RegisterTaskType` 注册 factory，在 factory 中构造真实 `TaskExecutor`，并按需要替换 `Param.Extension`；同步扩展独立的 `register_test.rs`、`manager_test.rs` 和具体 task executor 测试，不要把测试嵌入 `manager.rs`。
- 改变任务状态处理：优先修改 `handleTasks`，并在 `manager_test.rs::test_manager_handle_tasks_full_lifecycle` 增加状态、重复启动和任务表副作用断言。要先确认 scheduler 返回列表的排序契约，因为本文件不自行排序。
- 改变抢占策略：`handleExecutableTasks` 只负责执行决策，排名和可释放集合由 `slotManager::canAlloc` 提供；需同时更新 `slot.rs` 及独立 `slot_test.rs`，并保留“先 Cancel、退出后 free、下一轮再分配”的两阶段不变量。
- 增加启动步骤：把资源取得放在工厂/`Init` 之前，把对应清理覆盖到完整任务加载失败、runtime 失败、missing factory、`Init` 失败、`Run` 正常退出和 panic 等路径；`manager_test.rs` 现有 lifecycle/runtime 测试是最近的回归入口。
- 改变停止语义：必须明确管理循环、executor 线程、`Close` 和 join 的先后关系，避免让 `Stop` 等待只能由稍后 `Close` 才能解除的线程。建议新增一个只在 `Close` 后退出的测试 executor 来验证设计。
- 对齐 Go 行为：以 `manager.go` 与 `manager_test.go` 的具体差异为依据，不能只补桩或删减逻辑。尤其关注 service scope、日志/指标/trace、panic 恢复和统一退避策略。
- 性能风险：当前每个任务一个 OS 线程，轮询最短可被测试 setter 降得很低，注册表和 workers 都是单锁；扩展时应避免在持锁区做 I/O 或长时间等待，并用压测或并发测试评估任务数增长后的线程与锁开销。

## 验证依据

- 目标源码：`pkg/dxf/framework/taskexecutor/manager.rs`，已核对全部 378 行，包括常量、`RuntimeLease`、`Manager`、全部方法和 `DefaultExtension`。
- 包边界：`pkg/dxf/framework/taskexecutor/Cargo.toml`、`pkg/dxf/framework/taskexecutor/lib.rs`；框架定位：`pkg/dxf/framework/doc.go`。
- 直接依赖：`interface.rs` 的 `TaskTable`/`TaskExecutor`/`Extension`，`slot.rs` 的 slot 分配与抢占，`register.rs` 的工厂注册表，`task_executor.rs` 的 `TaskRuntime`/`Param`。
- Rust 生产入口：`pkg/session/runtime/modify_column_dist_backfill.rs` 的 `NodeService` 构造、初始化、启动和停止路径。
- Go 对照：`pkg/dxf/framework/taskexecutor/manager.go`；Go 测试：`manager_test.go` 中的管理、状态处理、slot 抢占、cross-keyspace runtime 和 InitMeta 重试用例。
- Rust 独立测试：`pkg/dxf/framework/taskexecutor/manager_test.rs`，覆盖注册表操作、暂停/回滚、未知类型、可重试与不可重试 Init 失败、完整 Run/Close 生命周期、无法分配时继续扫描、去重、抢占、三次重试、取消中断、Stop 唤醒、runtime 获取失败重试和 holder 释放；`main_test.rs` 覆盖测试周期守卫。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file .../manager.rs` 返回完整源码；`query` 找到 `NewManager`、`InitMeta`、`handleTasks`、`handleExecutableTasks`、`startTaskExecutor`、`failSubtask`、`runWithRetry`；`callees` 确认了前述内部主链。精确 `callers` 对这些 Rust impl 方法未返回调用者，因此上游入口另以源码搜索和调用点读取核验，未把空结果解释为“无人调用”。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核上述结论均可回溯到列出的符号或文件。
