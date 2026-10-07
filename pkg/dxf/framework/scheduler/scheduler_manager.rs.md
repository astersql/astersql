# `pkg/dxf/framework/scheduler/scheduler_manager.rs`

## 文件定位

本文件实现 DXF（Distributed eXecution Framework）在 owner 节点上的调度管理器，是 `astersql-dxf-framework-scheduler` crate 对多个单任务 `Scheduler` 的编排层。crate 入口 `pkg/dxf/framework/scheduler/lib.rs` 将本模块公开为 `scheduler_manager` 并再导出其公共项；`pkg/dxf/framework/scheduler/Cargo.toml` 表明它直接依赖 proto、schstatus、storage 与 dxfmetric 四个相邻 crate。文件自身不实现具体任务的状态机，而是借助 `interface.rs` 中的 `TaskManager`、`Scheduler`、`Cleaner` 及工厂注册表，把持久化任务、节点视图、slot 资源和具体调度器串起来。

当前 Rust 版本把一次 owner 调度循环抽象为同步的 `Manager::tick`，不在本文件中选择异步运行时或创建后台线程。仓库内 Rust 生产代码搜索未发现建立长期 owner 循环的直接调用者；当前明确的构造/驱动证据主要来自同 crate 独立测试以及 `pkg/dxf/framework/handle/handle_test.rs`、`pkg/dxf/importinto/clean_up_test.rs`。因此它是可复用且已测试的管理器实现，但“由完整 Rust 服务持续调用 `tick`”在本次证据范围内未验证。

## 核心职责

- `Manager::start` 刷新 managed 节点与 slot 容量，立即排空已有终态任务清理批次，然后以 `initialized` 发布可调度状态。
- `Manager::tick` 在已初始化时依次获取候选任务、启动新调度器、推进所有运行调度器，并再次排空终态任务；未初始化时是无副作用的成功返回。
- `get_schedulable_tasks`/`start_schedulers` 实现并发上限、未知任务类型处理、无需资源状态的快速通道，以及 slot/stripe 预留判断。
- `start_scheduler` 通过任务类型工厂构造 `Scheduler`，只在初始化成功后预留资源并登记到运行表；`drive_schedulers` 负责单步推进、正常完成时关闭并释放预留。
- `balance` 把按任务排名排序的调度器交给 `Balancer`；具体节点过滤和 pending subtask 重分配在 `balancer.rs` 中完成。
- `run_expired_file_clean`、`clean_finished_tasks`、`process_clean_task_batch` 与 `drain_clean_task_batches` 处理外部过期文件和终态任务历史迁移；`gc_subtasks`、`collect` 提供显式维护与观测入口。
- `max_concurrent_tasks`/`set_max_concurrent_tasks` 维护进程级并发配置，并通过 Go 风格的 `GetMaxConcurrentTask`/`SetMaxConcurrentTask` 兼容导出。

## 主要符号

- `DEFAULT_MAX_CONCURRENT_TASKS = 16`、`MAX_CONCURRENT_TASKS_UPPER_BOUND = 1000`：并发调度器数的默认值与合法上界。`MAX_CONCURRENT_TASKS: AtomicUsize` 保存进程级当前值，读取/写入分别使用 Acquire/Release。
- `max_concurrent_tasks() -> usize`：无锁读取当前上限。`set_max_concurrent_tasks(value) -> Result<()>` 只接受闭区间 `[16, 1000]`，越界返回 `SchedulerError` 且不改旧值。
- `RunningScheduler`：运行表的私有条目，持有 `Arc<dyn Scheduler>`、启动时的 `TaskBase` 快照、是否占用 slot，以及 stripe 失败后回退到的单节点 `reserved_exec_id`。保留快照是为了结束时按启动时的相同资源参数调用 `SlotManager::unreserve`。
- `Manager`：核心公开类型。其依赖为 `Arc<dyn TaskManager>`、`Arc<NodeManager>`、`Arc<SlotManager>`、受 `Mutex` 保护的 `Balancer`、受 `RwLock` 保护的任务 ID 到 `RunningScheduler` 映射，以及 server/resource 配置与原子初始化标志。
- `Manager::new`：创建全新的节点/slot 管理器，构造共享同一组依赖的 `Param` 与 `Balancer`；不会自动刷新节点或开始调度。
- `Manager::{start,cancel,stop,initialized,scheduler_count,schedulers}`：生命周期和只读查询 API。`schedulers` 每次从哈希表生成快照，再按 `TaskBase::compare` 排序，不能依赖哈希迭代顺序。
- `Manager::{tick,balance,run_expired_file_clean,clean_finished_tasks,process_clean_task_batch,drain_clean_task_batches,gc_subtasks,collect}`：由外层 owner 驱动的周期性动作。
- `MetricsSnapshot { tasks, subtasks, scheduled_tasks }`：`collect` 返回的值对象；`scheduled_tasks` 只统计 `Running` 或 `Modifying` 的任务。
- 本文件没有条件编译项；Windows 专属的大量 crate 依赖声明位于 Cargo manifest，并不改变此文件的源级 API。

## 执行流程

1. 调用方先以任务存储边界、server ID 和可选 `NodeResource` 构造 `Manager`。`new` 同时创建空的 `NodeManager`、`SlotManager`、`Balancer` 和运行表。
2. `start` 调用 `NodeManager::refresh_nodes`：通过 `TaskManager::all_nodes` 建立 managed 节点快照，并以第一个正 CPU 数刷新 slot capacity。随后 `drain_clean_task_batches` 处理 owner 接管前遗留的终态任务，最后以 Release 写入 `initialized=true`。任一步存储错误都会阻止初始化完成。
3. 外层循环调用 `tick`。若未初始化立即返回；否则 `get_schedulable_tasks` 根据当前运行数选择 `top_unfinished_tasks`，达到并发上限时改取 `top_no_need_resource_tasks`。已有运行条目的任务被跳过；没有注册 scheduler factory 的任务被标记失败，并调用 `on_task_finished` 更新完成指标。
4. `start_schedulers` 先整体刷新节点的已用 slot 快照。对 `Pending`、`Running`、`Resuming` 任务，它再次核对并发上限并调用 `SlotManager::can_reserve`；其他状态（如 cancelling/reverting/pausing/modifying）不占 slot，因而即使普通并发额度已满仍可启动。
5. `start_scheduler` 重新按 ID读取完整 `Task`，由 `get_scheduler_factory(task_type)` 取得工厂并注入 `Param`。`Scheduler::init` 失败时将任务标记失败且不登记；成功时先登记 slot 预留，再把 `RunningScheduler` 写入映射。
6. `drive_schedulers` 取得按任务排名排序的 `Arc` 快照，对每个实例执行 `schedule_once`。`Ok(true)` 表示这一轮后调度器完成，随后从运行表移除、调用 `close` 并释放预留；`Ok(false)` 与 `Err(_)` 都保留实例到下一次 `tick`，后者对应 Go 的可重试调度错误语义。
7. 每次 `tick` 最后排空清理批次。每个批次先按 task type 复用一个具有 `BatchCleaner` 能力的 cleaner；普通 cleaner 顺序执行。没有 cleaner 的任务直接视为已清理。遇到首个清理错误即停止后续清理，但仍把此前成功项迁入历史表；只有本批全部迁移时 drain 才读取下一批。
8. `balance`、`gc_subtasks`、`collect` 和 `run_expired_file_clean` 不由 `tick` 自动调用（清理终态任务除外），需要 owner 驱动层按自己的周期显式触发。

## 数据与状态

- `schedulers: RwLock<HashMap<i64, RunningScheduler>>` 是本节点正在驱动的调度器真相源；写锁只覆盖插入、删除或 `stop` 的 drain，调用 `init`、`schedule_once`、`close` 时不持有该锁。`schedulers()` 返回克隆的 `Arc` 快照，因此推进期间即使映射随后变化，实例仍有稳定引用。
- `initialized: AtomicBool` 仅表示 `start` 是否成功完成/是否已 `cancel`，不是线程退出信号。`cancel` 不关闭调度器；`stop` 才会清空运行表、逐个关闭并释放资源。
- slot 预留由 `RunningScheduler::{reservation,allocated_slots,reserved_exec_id}` 与 `SlotManager` 的内部预留表共同维护。只有三个需要执行资源的状态占用 slot；其他状态允许超过 `max_concurrent_tasks`，这是快速响应取消/回滚/暂停/修改的有意行为。
- 全局最大并发是进程级静态原子值，不属于某个 `Manager`，因此测试或运行时修改会影响同进程所有 manager。设置 API 的最小值不是 1，而是默认值 16。
- 清理分组的 `HashMap<String, (Arc<dyn Cleaner>, Vec<Task>)>` 只保证同类型聚合，不保证不同 batch 类型之间的执行顺序。清理副作用与 `transfer_tasks_to_history` 不构成事务；`interface.rs::BatchCleaner` 明确要求实现对部分失败后的重试保持幂等。
- `collect` 每次从存储读取完整任务/子任务快照；任务读取失败会直接返回错误，不会继续读取子任务。其结果不包含 Go 版 required/current worker gauge 的计算。

## 依赖与调用关系

上游入口：

- `pkg/dxf/framework/scheduler/lib.rs` 公开模块并 `pub use scheduler_manager::*`。
- 独立 Rust 测试 `scheduler_manager_test.rs`、`scheduler_manager_nokit_test.rs` 和 `scheduler_test.rs` 直接构造 `Manager` 并调用 `start`/`tick`/清理 API；`pkg/dxf/framework/handle/handle_test.rs` 通过 crate 完整路径构造它，`pkg/dxf/importinto/clean_up_test.rs` 用它验证导入清理器。
- RustCodeGraph 的文件节点报告本文件被 12 个文件使用，但精确方法级 `Manager`/`tick` 查询受到同名符号歧义影响；本次以文件节点、精确函数查询和 `rg` 调用点补足。未发现 Rust 生产 owner 循环直接驱动本类型，因此服务级接线标为未验证。

下游调用：

- `interface.rs`：`TaskManager` 提供任务/节点/slot/历史/GC 存储边界，scheduler/cleaner 工厂按 `task_type` 注册；`Scheduler` 定义 `init`、`schedule_once`、`close`、`task` 和 `extension`。
- `nodes.rs`：`start` 使用 `NodeManager::refresh_nodes` 建立节点视图并刷新容量。
- `slots.rs`：启动前 `update` 和 `can_reserve`，启动后 `reserve`，完成或停止时 `unreserve`；它先尝试 stripe，再回退到单执行节点资源。
- `scheduler.rs`：未知类型或初始化失败时调用 `on_task_finished(TASK_STATE_FAILED, ...)` 更新任务完成指标。
- `balancer.rs`：`balance` 使用排序后的 scheduler 快照，按 scope、扩展给出的 eligible nodes、slot 与 max-node-count 重排 pending subtasks。
- `astersql-dxf-framework-dxfmetric`：过期文件清理失败增加 `EventExpiredFileCleanupFailed`，终态清理部分失败增加 `EventCleanupFailed`。

## 错误处理与边界

- `set_max_concurrent_tasks` 对小于 16 或大于 1000 的值返回带合法区间的 `SchedulerError`；原子值保持不变。
- `start`、`tick`、`balance`、`clean_finished_tasks`、`gc_subtasks`、`collect` 将必要的存储/依赖错误返回调用方。`tick` 若候选读取或 slot 用量刷新失败，会在启动/推进之前返回。
- 未知任务类型在候选过滤阶段被持久化为 failed；若 `fail_task` 本身失败，整个候选读取返回错误。`start_scheduler` 中理论上的第二次未知工厂检查则返回错误，防御注册表在两阶段间变化。
- scheduler 初始化失败会尝试持久化 failed；成功标记后返回 `Ok(())`，不会把实例加入运行表。`schedule_once` 的所有错误当前都被保留重试，不在此层区分永久/可重试错误，也不记录错误内容。
- Rust 锁中毒统一通过 `expect("... lock poisoned")` 转为 panic，而不是 `SchedulerError`。因此调用者不能通过 `Result` 恢复锁中毒。
- `run_expired_file_clean` 对空 URI 直接返回；只调用声明 `ExpiredFileCleaner` 能力的 cleaner。单个清理失败不会阻止后续工厂，除非 `Context` 已取消；错误不向调用者传播，仅累计失败指标。
- `clean_task_batch` 在单项或批量 cleaner 首次失败后停止，但会迁移无 cleaner 和此前清理成功的任务。历史迁移失败完整传播；由于前置清理可能已有外部副作用，重试依赖 cleaner 幂等。
- `stop`、`Drop`、调度完成都会调用 `close`；接口没有返回错误，因此关闭失败不可表达。`Drop` 再次调用 `stop` 是安全的，因为运行表已 drain 且 `unreserve` 对缺失任务无操作。

## 并发与资源生命周期

- `Manager` 的共享依赖均通过 `Arc` 持有；运行表用 `RwLock`，balancer 用 `Mutex` 串行化每次 balance。锁获取顺序在本文件中短且固定，外部 trait 方法均在释放运行表锁后调用，降低重入死锁风险。
- `MAX_CONCURRENT_TASKS` 和 `initialized` 使用 Release 写/Acquire 读，分别发布配置与初始化完成状态。`scheduler_count` 与后续插入之间不是单个原子临界区；如果多个线程并发调用 `tick`，它们可能都通过上限检查。本文件的设计对应单 owner 循环，调用方应串行驱动 `tick`。
- scheduler 生命周期为：工厂构造 → `init` →（可选）`reserve` → 运行表持有 → 多次 `schedule_once` → `close` →（可选）`unreserve`。初始化失败不会 reserve；正常完成和 `stop` 都成对释放。
- `drive_schedulers` 先对所有实例推进，再统一移除完成项。由于快照持有 `Arc`，`stop` 若与其并发可能导致同一 scheduler 的 `close` 被调用两次；同理 `tick` 与 `stop` 的并发协议未由类型系统强制。外层 owner 应把生命周期操作串行化，或保证具体 `close` 幂等。
- 终态清理 drain 在每批“全部迁移”时继续，空批、查询失败、迁移失败或部分清理都会停止，避免错误条件下紧密自旋。若持续有新批次产生且每批全成功，单次 drain 没有总批次数上限。
- Rust 版没有 Go 版的 `context.CancelFunc`、wait group、ticker、`finishCh` 或任务运行时 release 回调；`cancel` 仅翻转标志，`stop` 同步关闭本地实例，不等待任何由 `Scheduler` 内部创建的后台工作。

## 与 Go 版本的对应关系

共同语义（依据 `pkg/dxf/framework/scheduler/scheduler_manager.go`）：

- 两版都按 `TaskBase` 排名维护调度顺序；达到并发上限后都改取无需资源的任务，保证取消/回滚/暂停/修改能继续推进。
- 两版都仅为 Pending/Running/Resuming 预留 slot，启动前刷新已用量并再次核对上限；调度器初始化失败都将任务标记 failed。
- 两版终态清理都按 task type 聚合 batch cleaner、顺序运行普通 cleaner、在首错处停止，并把已经成功清理或无需清理的子集迁入历史；迁移计数决定是否继续 drain。
- 两版都支持 owner 范围的 `ExpiredFileCleaner`，取消后停止处理，并以 task ID `"-"` 记录非任务特定失败指标。

当前差异与迁移状态：

- Go `Start` 启动 schedule、GC、cleanup、expired-file、metrics、节点维护/刷新和 balance 多个 goroutine；Rust `start` 只刷新节点、同步 drain 清理并设置标志，周期性动作由调用方显式驱动。
- Go 为每个任务获取 `TaskRuntime`，在 scheduler goroutine 退出时 release，并通过 `finishCh` 唤醒清理；Rust `Param` 没有 `TaskRuntime`，`tick` 在同一调用线程上执行 `schedule_once`，末尾直接 drain。
- Go `Stop` 先取消 context、等待 scheduler/管理 goroutine，再清空映射和指标；Rust `stop` 不等待后台任务，也不重置 worker/finished metrics。
- Go 的 scheduler 运行 `ScheduleTask` 直到退出；Rust 每次调用 `schedule_once`，并把任何错误留到下次 tick。该差异要求 Rust 外层提供稳定的 tick 周期和关闭串行化。
- Go `collect` 更新注册到 Prometheus 的 collector，并在 next-gen 模式计算 required/current workers；Rust `collect` 只返回 `MetricsSnapshot`，没有指标注册生命周期。
- Go `runExpiredFileClean` 自行从 store/handle 获取 cloud URI；Rust API 要求调用方提供 `Context` 与 URI，更易测试但把配置发现责任上移。

## 扩展指南

- 新增任务类型时，应在任务模块注册 `SchedulerFactory`，并在需要终态/过期文件清理时注册对应 `CleanerFactory`；不要在 `Manager` 内按 task type 写分支。同步增加独立 `*_test.rs`，验证未知类型、初始化失败和实际状态转换。
- 修改可占资源状态或并发策略时，重点检查 `get_schedulable_tasks`、`start_schedulers` 和 `RunningScheduler` 的 reserve/unreserve 对称性；同步更新 `scheduler_manager_nokit_test.rs` 中达到上限仍调度无需资源任务、非资源状态不分配 slot 的用例，并对照 Go 同名函数。
- 改变 scheduler 返回语义时，应同时修改 `drive_schedulers` 与 `Scheduler::schedule_once` 契约，明确错误是否重试、何时 close。需要增加独立测试覆盖一次失败后保留、后续成功后释放 slot，不能把测试嵌入生产源文件。
- 新增周期性维护动作时，优先提供类似 `balance`/`gc_subtasks`/`collect` 的显式单次入口，由 owner/runtime 层决定定时器；若要移植 Go 后台循环，应连同取消、等待、唤醒和 Drop 并发协议设计，不能只在 `start` 中无管理地 spawn。
- 新增 cleaner 能力时，要维持“工厂快照不持注册表锁执行”“同 task type 一个 batch cleaner 实例”“清理与历史迁移非原子”的约束，并要求外部副作用幂等。相关测试应放在 `scheduler_manager_test.rs` 或 `scheduler_manager_nokit_test.rs`。
- 扩展 `MetricsSnapshot` 时，需要核对 Go `collect`/`collectWorkerMetrics`、dxfmetric label 契约和所有构造点；大表全量读取可能带来存储与内存成本，必要时先定义采样/分页边界。
- 若把本管理器接入 Rust 生产服务，应先确定唯一 owner、串行 tick/balance/stop、tick 触发机制和 owner 迁移后的清理策略，并为真实接线新增集成测试；当前文档没有把尚未找到的生产调用链描述为已支持。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/dxf/framework/scheduler/scheduler_manager.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 1200` 返回完整 500 行源码并报告 41 个符号及 12 个使用文件。`query max_concurrent_tasks --kind function --json` 精确定位本文件第 42 行和第 47 行的 getter/setter。方法级 callers/callees 因 `Manager`/`tick` 同名歧义未产生可靠输出，故未将宽泛结果当作证据。
- 生产源码：`pkg/dxf/framework/scheduler/scheduler_manager.rs`（管理流程）；`interface.rs`（TaskManager/Scheduler/Cleaner/Param 契约和注册表）；`nodes.rs`（节点刷新）；`slots.rs`（预留与释放）；`balancer.rs`（balance 下游）；`scheduler.rs`（`on_task_finished`）。
- crate 边界：`pkg/dxf/framework/scheduler/Cargo.toml` 与 `lib.rs`。目标目录没有 `doc.go`，因此不存在额外 Go package contract 文件可读。
- Go 对照：`pkg/dxf/framework/scheduler/scheduler_manager.go`；重点核对 `NewManager`、`Start`/`Stop`、`scheduleTaskLoop`、`getSchedulableTasks`、`startSchedulers`/`startScheduler`、清理循环/分组、expired-file 与 metrics collect 流程。
- 独立 Rust 测试：`scheduler_manager_test.rs` 覆盖 cleaner 改写后迁移、启动即 drain、停止条件和 tick 后清理；`scheduler_manager_nokit_test.rs` 覆盖排名、无需 slot 状态、非法 keyspace 初始化失败、失败指标、并发上限快速通道、bounded cleanup、single/batch cleaner 部分失败与历史迁移重试；`scheduler_test.rs` 覆盖 manager 驱动真实基础调度器。
- Go 测试：`scheduler_manager_test.go`、`scheduler_manager_nokit_test.go`，包含对应 cleaner、排序、过期文件清理、无需资源状态、跨 keyspace 与达到并发上限用例。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构检查要求目标文件存在且恰有 11 个固定二级标题；运行结果在交付时报告。
