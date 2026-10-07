# [`pkg/dxf/framework/scheduler/scheduler.rs`](./scheduler.rs)

## 文件定位

本文件实现 DXF（Distributed eXecution Framework）Owner 侧的单任务调度状态机 `BaseScheduler`。crate 入口 `pkg/dxf/framework/scheduler/lib.rs` 将其作为 `scheduler` 模块公开并再导出；`pkg/dxf/framework/scheduler/Cargo.toml` 表明它属于 `astersql-dxf-framework-scheduler` crate，直接依赖协议、调度状态、存储和指标四个相邻 crate。

在完整调度链中，`scheduler_manager.rs` 的 `Manager::start_scheduler` 根据任务类型工厂构造 `Arc<dyn Scheduler>`，`Manager::drive_schedulers` 对每个实例调用一次 `Scheduler::schedule_once`。因此本文件不拥有常驻轮询线程：它只完成一个可持久化的状态迁移，循环、资源预留和实例回收由 `Manager` 负责；任务类型差异则由 `interface.rs` 中的 `Extension` trait 注入。

## 核心职责

1. 以持久层中的 `Task.state` 和 `Task.step` 为准，分派 Cancelling、Pausing、Resuming、Reverting、Pending、Running、Modifying 以及终态分支（`BaseScheduler::schedule_once`）。
2. 在 Pending/Running 阶段调用 `Extension` 完成准备、计算下一步、筛选执行节点、生成子任务元数据和结束回调（`on_pending`、`on_running`、`switch_to_next_step`）。
3. 用 `TaskManager` 原子化持久化任务状态、步骤和子任务；对大元数据选择批量接口，对一般存储错误作有限重试（`schedule_subtasks`）。
4. 处理取消、暂停/恢复、失败回滚、人工恢复以及运行中参数修改，并同步本地任务快照。
5. 在成功或回滚完成时按最终结果更新 `FinishedTaskCounter`，并向扩展暴露历史子任务元数据/摘要查询（`on_task_finished`、`TaskHandle for BaseScheduler`）。

## 主要符号

- `TASK_CANCEL_MESSAGE`：直接复用 storage crate 的取消标记；`IsCancelledErr` 同样委托 storage crate 识别可被外层文本包裹的取消错误。
- `RETRY_SQL_TIMES = 30`：`schedule_subtasks` 的最大持久化尝试次数。
- `DEFAULT_TXN_TOTAL_SIZE_LIMIT = 1 GiB`：当前 Rust 实现估算子任务元数据单事务大小的固定基准；达到其 80% 时切到 `switch_task_step_in_batch`。
- `BaseScheduler { context, param, task, extension, closed }`：核心对象。`task: Mutex<Task>` 保存可替换快照，`extension: Arc<dyn Extension>` 提供类型相关逻辑，`closed: AtomicBool` 提供跨线程关闭信号。
- `BaseScheduler::new`：创建独立 `Context`、保存注入依赖与初始任务，但不启动后台线程。
- `refresh_task_if_needed`：先读取轻量 `TaskBase`；仅当 state 或 step 变化时读取完整 `Task`，降低系统表读取成本。
- `on_cancelling`、`on_pausing`、`on_resuming`、`on_reverting`、`on_pending`、`on_running`、`on_modifying`：各状态处理器；返回的 `bool` 语义是本实例是否应由 Manager 回收/重建。
- `switch_to_next_step`：完成态收口或下一阶段规划的中心函数，串联节点选择、扩展规划、子任务落库及本地快照更新。
- `schedule_subtasks`：调整候选节点、轮询生成 `Subtask`、选择单事务/批量写入并重试。
- `handle_prepare_or_plan_error`、`revert_task`、`revert_or_manual_recover`：把扩展错误分类为“向上返回以便后续 tick 重试”“自动回滚”或“等待人工处理”。
- `on_task_finished`：把 Succeed/Failed/Reverted 映射到指标标签；Reverted 通过 storage crate 的 `ClassifyTaskErrorMessage` 细分取消、数据错误等类别。
- `state_count`、`is_step_succeed`、`should_pause_on_kv_disk_full`：私有判定辅助。步骤成功的定义是计数为空，或唯一状态为 Succeed；磁盘满暂停要求开关开启、没有 canceled、失败数与错误数一致且每条错误文本都含 `disk full`（大小写不敏感）。

## 执行流程

1. `Manager::drive_schedulers` 调用 `schedule_once`。若 `closed` 已以 Acquire 读到 true，立即返回 `Ok(true)`。
2. `refresh_task_if_needed` 从 `TaskManager` 对比持久化 state/step，必要时替换完整快照，避免用户操作、Owner 切换或其他调度器修改后继续使用旧状态。
3. `schedule_once` 按状态分派：Cancelling 记录统一取消错误并进入 Reverting；Pausing 等活跃子任务清空后进入 Paused；Resuming 恢复暂停子任务或转回 Running；Reverting 等活跃子任务结束后执行 `on_done` 并落为 Reverted；终态和 Paused 返回 `true`；未知状态保持实例并返回 `false`。
4. Pending 若要求 Prepare 且仍处于 `STEP_INIT`，先调用 `Extension::on_prepare_with_context`，再由 `switch_task_step_after_prepare` 竞争式确认准备状态；确认成功后设置 `STEP_PREPARED`，同一 tick 继续规划下一阶段。
5. `switch_to_next_step` 调用 `Extension::next_step`。若为 `STEP_DONE`，依次执行 `on_done_with_context`、`succeed_task`、更新本地 step/state 和完成指标；否则先取得托管节点，再优先采用扩展返回的 eligible instances，为空时才按 `target_scope` 过滤托管节点，并在规划前应用 `max_node_count`。
6. 扩展通过 `on_next_subtasks_batch_with_context` 生成 meta；`schedule_subtasks` 更新 slot 视图、按所需并发调整节点，并用 `index % eligible.len()` 轮询分配执行节点，ordinal 从 1 开始。任务步骤与全部子任务由 `TaskManager` 一起持久化成功后，本地状态才更新为 Running/next step。
7. Running 汇总当前步骤子任务状态：存在 failed/canceled 时优先判断磁盘满自动暂停，否则取第一条错误进入回滚或 AwaitingResolution；全部成功时进入下一步；仍在执行时只触发 `Extension::on_tick_with_context`。
8. Modifying 直接处理 `modify_concurrency` 和 `modify_max_node_count`，其余修改交给 `Extension::modify_meta`；落库后恢复 `previous_state` 并清空 modifications。并发数变化返回 `true`，要求 Manager 回收并按新 slot 需求重建实例。

## 数据与状态

任务的权威状态在 `TaskManager` 背后的系统表中；`BaseScheduler.task` 只是受 `Mutex` 保护的本地快照。写操作遵循“先持久化、后替换快照”的基本顺序，例如 `revert_task`、`on_pausing`、`on_resuming`、`switch_to_next_step` 和 `on_modifying`。扩展回调可修改传入的 `&mut Task`（尤其是 meta），只有后续持久化成功后这些变化才成为有效调度状态。

关键不变量包括：Pending/Running/Resuming 必须已有 `Param.allocated_slots`，否则返回 `Ok(true)` 让 Manager 重新建立有资源预留的调度器；Pausing/Reverting 只有在 pending 与 running 子任务数均为零时才能收口；子任务创建时 task_id、step、required_slots/concurrency 一致，ordinal 连续且从 1 开始；`max_node_count` 在扩展按节点数规划子任务前截断候选集。

`Context` 属于单个调度器生命周期，`close` 会取消它；`TaskHandle` 查询始终转发到 `TaskManager`，不会从本地快照推导历史结果。`Param.server_id` 和 `node_resource` 虽属于注入结构，但本文件当前不直接读取它们。

## 依赖与调用关系

- 上游：`scheduler_manager.rs::Manager::start_scheduler` 通过注册工厂获得实现，调用 `init` 后登记；`Manager::drive_schedulers` 调用 `schedule_once`，当其返回 true 时关闭实例并释放 slot。`scheduler_test.rs`、`scheduler_nokit_test.rs`、`scheduler_manager_nokit_test.rs` 和 `integrationtests/framework_test.rs` 直接覆盖或组合使用该实现。
- 协议与接口：`interface.rs` 提供 `Task`、`Subtask`、状态/步骤常量、`Scheduler`、`Extension`、`TaskHandle`、`TaskManager`、`Param`、`Context` 和 `SchedulerError`。
- 节点与资源：`nodes.rs::filter_by_scope` 负责 scope 过滤；`Param.node_manager` 提供托管节点；`Param.slot_manager` 更新容量并调整 eligible nodes。
- 持久化：所有状态迁移、子任务计数/错误读取、历史查询和步骤切换均通过 `Param.task_manager`。本文件不直接执行 SQL。
- 指标与错误分类：`astersql-dxf-framework-dxfmetric::InitDistTaskMetrics` 更新完成计数；storage crate提供取消文案、取消识别和完成错误分类。
- crate 边界：`Cargo.toml` 的四个无条件依赖与上述职责对应；大量仅 Windows 条件下的依赖服务于同 crate 的其他迁移模块，本文件没有直接引用它们。

RustCodeGraph 的文件节点显示 `scheduler.rs` 被 11 个文件使用，并明确列出 `scheduler_manager.rs`、两类 scheduler 测试、`test_support.rs` 和框架集成测试等；精确 `callers/callees` 命令未产出符号级边，因此调用方向又由这些已索引文件中的具体调用点交叉核对。

## 错误处理与边界

本文件统一返回 `interface::Result<T>`/`SchedulerError`，大部分 TaskManager/Extension 错误以 `?` 原样上抛，由 Manager 保留实例并在后续 tick 再尝试。Prepare/规划错误是例外：`Extension::is_retryable_error` 为 true 才上抛；否则立即持久化 Reverting。子任务失败若没有对应错误，会合成包含 task/step 的诊断错误，避免无原因回滚。

节点集合为空会明确报错 `no available TiDB node to dispatch subtasks`，且不会推进本地任务。`schedule_subtasks` 对包含 `unstable subtasks` 的错误立即停止重试；其他错误最多尝试 30 次，但当前实现没有退避等待。若 30 次均失败，返回最后一次错误；理论上的无错误退出则产生 `failed to persist subtasks`。

`Mutex` 中毒时使用 `expect`，属于进程内不可恢复错误而非 `SchedulerError`。节点经 slot 调整后代码直接用取模索引；其安全性依赖 `SlotManager::adjust_eligible_nodes` 对原先非空候选集不返回空集合，扩展该资源策略时必须维持或显式处理这一契约。`init` 当前只拒绝 keyspace 中的 NUL 字符，不等同于 Go 版本完整的任务运行时校验。

磁盘满识别当前依赖错误文本中包含 `disk full`，这比 Go 的 `errdef.IsKVDiskFullError` 类型/错误链判断更窄，改变错误文本或引入结构化错误时需要同步修改判定和测试。固定 1 GiB 事务基准也不同于 Go 从运行时 `kv.TxnTotalSizeLimit` 读取的行为。

## 并发与资源生命周期

`BaseScheduler` 满足 `Scheduler: Send + Sync`：任务快照由 `Mutex` 串行访问，扩展点用 `Arc` 共享，关闭位使用 Acquire/Release 原子序。每个方法都克隆任务快照后操作，避免把锁跨 TaskManager I/O 或 Extension 回调持有；替换时才短暂重新加锁。

`close` 先以 Release 写入关闭位，再取消 `Context`；后续 tick 以 Acquire 观察关闭并请求回收。实际调度实例集合、并发驱动和 slot reserve/unreserve 由 `Manager` 管理，本文件只消费 `allocated_slots` 标记并在派发前刷新 slot 视图。没有本文件自行创建的线程、异步任务或通道，也没有显式事务对象；原子持久化边界由 `TaskManager::switch_task_step*`、`pause_task_on_error` 等接口实现。

回调生命周期需要特别注意：`on_tick` 可在 Running/Reverting 等待期重复调用；`on_done` 在成功或回滚完成时调用，失败则不提前写终态；Prepare 可能因持久化竞争或重试在多个 tick 中再次执行，所以扩展实现应保持可重入/幂等。`TaskHandle` 允许这些回调读取历史子任务结果，但不授予直接修改调度器快照的能力。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/scheduler/scheduler.go`。Rust 的 `BaseScheduler`、`refresh_task_if_needed`、各 `on_*`、`switch_to_next_step`、`schedule_subtasks`、回滚辅助和 `TaskHandle` 实现，分别对应 Go 的 `BaseScheduler`、`refreshTaskIfNeeded`、各状态处理器、`switch2NextStep`、`scheduleSubTask`、`revertTask*` 与 `GetPreviousSubtask*`；核心状态迁移、Prepare 后同 tick 继续、节点上限、轮询分派、批量持久化、人工恢复和结束指标意图保持一致。

当前可验证差异如下：

- Go 的 `ScheduleTask` 自带 ticker/context 循环；Rust 将循环拆到 `Manager::tick`，`schedule_once` 每次只推进一轮，更适合确定性测试。
- Go 用 `atomic.Pointer[Task]` 保存不可变指针快照；Rust 用 `Mutex<Task>` 加 clone/replace 达到相同的快照更新边界。
- Go `Init` 调用 `dxfutil.CheckTaskRuntime`；Rust 目前只检查 keyspace NUL 字符，属于较窄的已移植校验。
- Go 的事务阈值读取动态 `kv.TxnTotalSizeLimit`，重试带指数退避且用 `storage.ErrUnstableSubtasks` 类型判断；Rust 使用固定 1 GiB、紧循环重试和错误消息匹配。
- Go 的磁盘满判断使用 `errdef.IsKVDiskFullError`；Rust 使用大小写不敏感的字符串包含判断。
- Go 文件还承载 logger/sample logger、failpoint、live executor discovery、session/transaction helper 等能力；这些不在当前 Rust `scheduler.rs` 的 API 中，不能据 Go 实现宣称 Rust 已支持。

Go 测试 `scheduler_test.go` 与 `scheduler_nokit_test.go` 是移植语义参考；Rust 对应独立测试位于同目录的 `scheduler_test.rs` 和 `scheduler_nokit_test.rs`，没有把测试内嵌进生产源文件。

## 扩展指南

- 新增任务状态：在 `interface.rs` 定义/对齐状态常量及转换契约，在 `schedule_once` 添加显式分支，并同步 `state_transform.rs`；至少在 `scheduler_test.rs` 或 `scheduler_nokit_test.rs` 增加状态进入、等待、落库失败和终态回收测试。
- 新增任务类型行为：优先实现/扩展 `Extension`，不要把类型判断写进 `BaseScheduler`。规划函数必须处理重复调用，并确保对 `Task.meta` 的修改与步骤/子任务持久化一起生效。
- 改变节点分配：修改 `switch_to_next_step`/`schedule_subtasks` 前同时检查 scope、`max_node_count`、slot 调整和非空不变量；同步轮询、多节点、无节点及资源不足测试，关注任务公平性和除零风险。
- 改变错误策略：保持“可重试错误不改任务状态、不可重试错误先持久化再改快照”的顺序；结构化 `unstable subtasks` 或磁盘满错误时，应消除字符串匹配并补充包装错误、混合 failed/canceled、错误数不一致测试。
- 改变并发或修改协议：`modify_concurrency` 必须继续触发调度器重建，以便 Manager 释放旧 slot 并按新值预留；修改 `max_node_count` 或自定义 meta 时验证 previous_state 恢复和 modifications 清空。
- 调整批量阈值/重试：要评估单事务限制、数据库压力和忙等性能；如向 Go 行为收敛，应引入可配置阈值、类型化不可重试错误和可取消退避，并将测试放在独立 `*_test.rs` 文件。
- 任一 Rust 行为变更都应与 `scheduler.go` 和相应 Go 测试重新对照；若有意偏离，需记录原因与兼容性影响，而不是只以编译或零测试证明完成。

## 验证依据

- RustCodeGraph：`status` 显示本地索引包含目标目录；`files --filter pkg/dxf/framework/scheduler` 列出目标、模块、Go 对照与独立测试；`node --file pkg/dxf/framework/scheduler/scheduler.rs --offset 1 --limit 500` 及 `--offset 495 --limit 100` 覆盖目标 545 行全貌，并报告 11 个使用文件；`node` 还读取了 `scheduler_manager.rs`、`interface.rs`、`scheduler_test.rs`、`scheduler_nokit_test.rs` 的直接调用与契约。针对 `BaseScheduler`、`schedule_once`、`switch_to_next_step` 执行了 query/callers/callees；query 定位了目标符号，callers/callees 未返回可用边，故以文件使用关系和具体调用点补证。
- crate/模块：`pkg/dxf/framework/scheduler/Cargo.toml`、`pkg/dxf/framework/scheduler/lib.rs`。
- Go 对照：`pkg/dxf/framework/scheduler/scheduler.go`；测试名称与覆盖面来自 `pkg/dxf/framework/scheduler/scheduler_test.go`、`pkg/dxf/framework/scheduler/scheduler_nokit_test.go`。
- Rust 独立测试：`pkg/dxf/framework/scheduler/scheduler_test.rs` 覆盖完成、取消、回滚、人工恢复、轮询分派、暂停和 Manager 生命周期；`pkg/dxf/framework/scheduler/scheduler_nokit_test.rs` 覆盖初始化、准备、节点选择、磁盘满、暂停/恢复/回滚、无 slot、刷新、修改与完成指标。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰好包含 11 个固定二级标题，并人工核对本文未把 Go 独有能力描述成 Rust 现状。
