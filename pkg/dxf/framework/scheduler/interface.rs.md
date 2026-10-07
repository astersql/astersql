# `pkg/dxf/framework/scheduler/interface.rs`

## 文件定位

本文件是 `astersql-dxf-framework-scheduler` crate 的公共契约层，定义 DXF owner 侧调度所使用的状态名称、任务/子任务数据模型、存储边界、业务扩展点、单任务调度器接口和任务类型工厂注册表。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod interface` 和 `pub use interface::*` 将这些定义再导出；所属 crate 及路径依赖见 [`Cargo.toml`](Cargo.toml)。仓库级 [`pkg/dxf/framework/doc.go`](../doc.go) 将 scheduler manager、task scheduler、node manager 和 balancer 定义为 owner 节点组件，而 executor/slot manager 分布在所有节点，因此本文件连接“持久化任务状态—owner 调度循环—具体任务类型实现”，但自身不运行循环、访问数据库或执行子任务。

目标源码是 [`interface.rs`](interface.rs)。它不是桩或纯装配文件：除接口外还实现了任务排序、终态判断、运行时槽位计算、可取消等待、清理批次截断和并发安全的全局工厂注册表。不过完整状态迁移位于 [`scheduler.rs`](scheduler.rs)，多任务选择、资源预留和清理驱动位于 [`scheduler_manager.rs`](scheduler_manager.rs)。

## 核心职责

1. 用 `TaskState`、`SubtaskState`、`Step` 及常量表达调度协议。任务终态为 `succeed`、`reverted`、`failed`；框架保留 `STEP_INIT = -1`、`STEP_DONE = -2`、`STEP_PREPARED = -3`，业务步骤使用非负值。`PREPARE_MODE_REQUIRED` 控制正式规划前的准备阶段。
2. 用 `TaskBase`/`Task`、`SubtaskBase`/`Subtask`、`ExtraParams`、`Modification`、`ManagedNode`、`NodeResource`、`SubtaskSummary` 承载调度快照。基础结构保存排序、资源和状态字段，完整结构再附加业务 `meta` 与错误。
3. 用 `TaskManager` 把 scheduler 与任务表实现隔离。状态读取、状态迁移、步骤切换、子任务统计以及历史 meta/summary 查询都经该 trait 完成。
4. 用 `Extension` 把框架状态机与任务类型逻辑隔离；具体任务决定下一步、生成 subtask meta、筛选节点、识别可重试错误、准备和修改 meta。
5. 用 `Scheduler` 与 `Param` 定义 manager 构造、初始化、单次推进、关闭调度器所需的最小边界。
6. 用 scheduler/cleaner 两套注册表把字符串 `task_type` 映射到构造函数；清理接口还支持按类型批处理和 owner 侧过期外部文件清理。

## 主要符号

- `SchedulerError(String)` 与 `Result<T>`：本 crate 的轻量错误边界。错误只保留消息并实现 `Display`/`Error`，没有错误码、来源链或类型化分类。
- `Context`：持有共享的 `AtomicBool` 取消标志以及 `(Mutex<bool>, Condvar)`。`cancel` 先发布原子状态，再更新条件变量状态并唤醒全部等待者；`wait` 可被取消中断；`cancellation_flag` 让对象存储上下文共享同一取消生命周期。
- `TaskBase`：关键方法是 `is_done`、`compare`、`runtime_slots`。`compare` 按 `priority ASC, create_time ASC, id ASC` 排名；`runtime_slots` 仅在正数 `max_runtime_slots` 对当前步骤生效时取其与 `required_slots` 的较小值。默认值为 pending/init、普通优先级 `512`、Unix epoch 和空 keyspace/scope。
- `Task`：在 `TaskBase` 上增加 `meta`、`error`、`previous_state`、`modifications`；默认 meta 是 `{}`，用于状态机的业务负载和运行时变更。
- `TaskManager`：核心持久化方法包括 `top_unfinished_tasks`、`fail_task`、`switch_task_step(_in_batch)`、`switch_task_step_after_prepare`、`subtask_count_by_states`、`previous_subtask_metas`。默认方法 `cleanup_tasks` 查询三种终态并按 `proto::GetTaskCleanupBatchSize()` 截断。
- `TaskHandle`：只把前一步 subtask meta/summary 查询暴露给 `Extension`，避免业务扩展获得完整写接口。
- `Extension`：必须实现规划、结束、选节点、重试判断、步进、准备和 meta 修改。`on_tick_with_context`、`on_next_subtasks_batch_with_context`、`on_done_with_context`、`on_prepare_with_context` 默认转调旧版无 context 方法，使已有实现可渐进采用取消上下文。
- `Param`：向工厂注入 `Arc<dyn TaskManager>`、`NodeManager`、`SlotManager`、server ID、槽位是否已预留及可选节点资源；`node_resource` 返回借用而非复制。
- `Scheduler`：`init`、`schedule_once`、`close`、`task`、`extension` 是 manager 驱动一个任务所需的生命周期接口。
- `SchedulerFactory`/`CleanerFactory`：均为 `Arc<dyn Fn... + Send + Sync>`，让注册表快照和调用者安全共享构造器。
- `Cleaner`、`BatchCleaner`、`ExpiredFileCleaner`：普通清理按任务执行；批量清理按同类型任务组执行；过期文件清理额外接收可取消 `Context`、只读式任务信息边界与云存储 URI。
- `RegisterSchedulerFactory`、`get_scheduler_factory`、`ClearSchedulerFactory` 以及对应 cleaner 函数：操作由 `LazyLock<RwLock<HashMap<...>>>` 承载的进程级注册表。重复注册会覆盖同名类型，未知类型查询返回 `None`，锁中毒会 `expect` 触发 panic。

## 执行流程

1. 进程启动或任务子系统初始化时，具体任务类型调用 `RegisterSchedulerFactory`；例如 [`pkg/dxf/importinto/scheduler.rs`](../../importinto/scheduler.rs) 的 `RegisterImportSchedulerFactoryWithServices` 注册 import scheduler。需要终态清理的类型通过 [`pkg/dxf/importinto/clean_up.rs`](../../importinto/clean_up.rs) 注册 cleaner。
2. `Manager::tick` 从 `TaskManager` 获取候选任务。`get_schedulable_tasks` 先用 `get_scheduler_factory` 拒绝未知 `task_type`，将其持久化为 failed；已知类型继续进行并发和槽位筛选。
3. `Manager::start_scheduler` 读取完整 `Task`，构造 `Param`，调用工厂得到 `Arc<dyn Scheduler>`，再执行 `init`。初始化失败会通过 `TaskManager::fail_task` 持久化；成功后才预留槽位并进入运行表。
4. `Manager::drive_schedulers` 周期性调用 `schedule_once`。标准实现 `BaseScheduler` 根据 `Task.state` 分派 pending/running/cancelling/pausing/resuming/reverting/modifying 等分支：通过 `TaskManager`读写状态，通过 `Extension`规划下一步骤、生成 meta 或处理完成回调，通过 `TaskHandle`读取历史结果。
5. pending 且要求 prepare 时，`BaseScheduler` 调 `on_prepare_with_context`，再以 `switch_task_step_after_prepare` 做条件持久化并进入 `STEP_PREPARED`；正常步进由 `next_step`、节点筛选、`on_next_subtasks_batch_with_context` 和 `switch_task_step(_in_batch)`组成。到 `STEP_DONE` 时先 `on_done_with_context`，再标记 succeed。
6. manager 回收完成的 scheduler 时调用 `close` 并释放已预留槽位；`BaseScheduler::close` 同时取消本文件的 `Context`，令等待或共享对象存储操作观察到取消。
7. 清理阶段，`TaskManager::cleanup_tasks` 返回有界终态批次。manager 按 `task_type` 查询 cleaner：没有 cleaner 的任务直接进入历史迁移；普通 cleaner 逐项运行；暴露 `batch_cleaner` 能力的实例按类型成组运行。`get_cleaner_factories` 返回稳定克隆快照，因此 owner 的过期文件清理不会在回调期间持有注册表读锁。

## 数据与状态

任务状态常量是 `&'static str`，与持久化/Go 协议字符串直接对齐，不具有 Rust enum 的穷尽检查。`TaskBase::is_done` 只认 succeed/reverted/failed；paused、awaiting-resolution 等均不是终态。调度排名的不变量来自 `compare`：数值更小的 priority 更优，同优先级先创建者更优，最后以较小 ID 打破平局；[`slots.rs`](slots.rs) 和 `Manager::schedulers` 都复用该比较，不能单独改变某一处排序。

资源字段分为声明值与运行值：`required_slots` 是任务声明并用于预留/并发，`ExtraParams.max_runtime_slots` 是可按 `target_steps` 限制的运行上限；上限为非正数时不生效，目标步骤列表为空表示所有步骤。`max_node_count == 0` 表示节点数不限。`target_scope` 与 `ManagedNode.role` 参与节点范围过滤；具体 fallback 规则在 `nodes.rs` 和框架包文档中实现/说明。

`Task.meta` 与 `Subtask.meta` 是不透明字节，不在此文件解析。`previous_state` 支持 modifying 完成后返回原状态，`modifications` 保存待应用变更。错误保存在 `Option<SchedulerError>` 中。所有 trait 方法接收或返回快照/切片，真正的原子性由 `TaskManager` 实现保证；尤其 `switch_task_step` 约定任务步进与 subtask 插入同一事务，而 batch 版本允许拆分事务，因此其 subtask 数量、顺序、内容必须在重试间稳定（这一约束在 Go 对照接口注释中更完整）。

## 依赖与调用关系

上游调用者主要是 [`scheduler_manager.rs`](scheduler_manager.rs)：它查询 scheduler/cleaner 注册表、构造 `Param`、按 `TaskBase::compare` 排序并驱动 trait。[`scheduler.rs`](scheduler.rs) 实现 `Scheduler` 与 `TaskHandle`，消费 `TaskManager`、`Extension`、状态常量和 `Context`。[`nodes.rs`](nodes.rs)、[`slots.rs`](slots.rs) 和 [`balancer.rs`](balancer.rs) 使用任务、节点及存储边界完成存活视图、资源预留和负载均衡。具体业务实现以 [`pkg/dxf/importinto/scheduler.rs`](../../importinto/scheduler.rs) 的 `ImportSchedulerExtension` 和工厂注册为代表。

直接标准库依赖为 `Arc`、`Mutex`、`RwLock`、`Condvar`、`LazyLock`、原子布尔、`HashMap`、`SystemTime` 和 `Duration`。直接 crate 内依赖是 `crate::nodes::NodeManager`、`crate::slots::SlotManager`、`crate::proto::GetTaskCleanupBatchSize`；`TaskCleanupInfo` 来自 `astersql-dxf-framework-storage`。Cargo 声明的核心无条件依赖还包括 proto、schstatus、storage 和 dxfmetric；大量生产/测试依赖受 `cfg(target_os = "windows")` 限制，这是当前 crate 的实际构建边界，不能从接口存在推断所有目标平台都编译全部 scheduler 实现。

RustCodeGraph 对目标文件给出 14 个直接使用文件，并确认关键边：`get_scheduler_factory` 被 manager 的 `get_schedulable_tasks`/`start_scheduler` 及 import 测试调用；`get_cleaner_factory` 被 `clean_task_batch` 调用；`get_cleaner_factories` 被 `run_expired_file_clean` 调用；Rust `RegisterSchedulerFactory` 的调用者包括 import 注册和 scheduler manager 回归测试。

## 错误处理与边界

接口层把可恢复业务失败统一为 `Result<T, SchedulerError>` 并向调用者传播。`Context::wait` 在等待前或被唤醒后发现取消时返回消息为 `context canceled` 的错误；超时本身返回 `Ok(())`。互斥锁和读写锁中毒不是 `Result` 分支，而是通过 `expect` panic，因此注册表或 context 临界区中的 panic 会把后续访问升级成进程级失败风险。

工厂查询缺失由 `Option` 表示。manager 对候选任务缺少 scheduler 工厂的情况会将任务标记 failed；在实际启动阶段再次缺失则返回 `unknown task type`，应保留双重检查以覆盖注册表变化。注册 API 没有重复检测，后注册者覆盖前者；清空函数注释和调用点均表明只应用于测试，生产并发清空会使正在选择任务的 manager 看到未知类型。

`Cleaner` 的副作用与历史表迁移没有事务原子性。`BatchCleaner` 注释明确要求实现可幂等重试：清理可能部分成功后报错，也可能清理成功但迁移失败。manager 只迁移成功清理的集合；相同类型批次失败时该组不迁移。`ExpiredFileCleaner` 必须及时观察 `Context`，否则 owner 丢失或 manager 停止时无法及时结束回调。

本文件只声明状态和值域，不验证负槽位、非法状态字符串、业务 meta 格式或状态迁移合法性。新增实现必须在构造/存储适配器/独立测试中补足这些约束，不能把字符串别名当作类型安全状态机。

## 并发与资源生命周期

`Context` 可克隆；所有 clone 共享同一取消原子量和条件变量。Release/Acquire 保证取消发布与观察的顺序，条件变量避免 `wait` 只能轮询。`cancel` 可重复调用并会再次 `notify_all`；取消不可复位，因此一个 context 只适合一个 scheduler 生命周期。`BaseScheduler::close` 触发取消，对象存储适配可通过 `cancellation_flag` 复用这一信号。

两个工厂表通过进程级 `LazyLock<RwLock<HashMap<...>>>` 延迟初始化：注册/清空独占写锁，查询持读锁并克隆 `Arc` 后立即释放。`get_cleaner_factories` 克隆键和工厂形成快照，回调执行期间既不持锁，也不受随后注册/清空影响。注册表没有作用域或自动注销，测试若使用全局清空需避免并行测试之间互相污染。

trait 均要求 `Send + Sync`，工厂闭包也要求 `Send + Sync + 'static`，使 manager 能跨线程共享接口对象；但该文件没有创建线程或 async task。`Param` 通过 `Arc` 共享 manager/node/slot 管理器，`Task`/`TaskBase` 则以 clone 快照在边界间传递。槽位的所有权属于 manager：工厂只收到 `allocated_slots` 事实，启动成功后 manager 记录预留，scheduler 完成、停止或 manager `Drop` 时关闭并释放。

## 与 Go 版本的对应关系

直接对照文件为 [`interface.go`](interface.go)。两版都定义 `TaskManager`、`Extension`、`Param`、scheduler factory、scheduler/cleaner 注册表以及 `Cleaner`/`BatchCleaner`/`ExpiredFileCleaner`，注册表都以读写锁保护，按任务类型覆盖注册并提供测试清空入口；清理批次失败后的幂等要求一致。Rust 的状态模型内聚在本文件，而 Go 主要复用 `proto.Task`/`proto.TaskBase`。

Rust 并非逐签名复刻 Go。Go `TaskManager` 的方法普遍显式接收 `context.Context`，并包含 `CancelTask`、`PauseTask`、按 step/state 读取完整 subtasks、`WithNewSession`/`WithNewTxn` 等能力；Rust trait 采用同步方法和更窄的 scheduler 所需集合，另以 `Context` 只覆盖扩展回调/退避/外部清理取消。Go `Param` 还有非拥有的 keyspace `TaskRuntime`，Rust `Param` 当前没有该字段。Go scheduler factory 接收 context 与指针任务，Rust 工厂接收拥有的 `Task` 和 `Param`。因此扩展 Rust 接口前必须验证实际 storage adapter 与 keyspace runtime 的归属，不能仅按 Go 名称机械添加方法。

Go `Extension` 直接把 context 放入每个回调；Rust 为兼容旧实现保留无 context 必选方法，并提供默认 `*_with_context` 包装。Go 的方法注释还明确：`OnTick` 不应做重工作、规划回调不得修改 task state、到 `StepDone` 应返回空 meta、`GetNextStep` 不应依赖 meta、`OnPrepare` 只可改 meta/required slots/max node count。这些仍是 Rust 实现应遵循的移植契约，但 Rust 类型系统没有强制执行。

Rust 还提供 `Context::cancellation_flag` 以连接 Rust 对象存储上下文，并将 `TaskBase::compare`/`runtime_slots` 等 proto 语义移到本 crate。当前 Rust `Scheduler` trait 对应 Go [`scheduler.go`](scheduler.go) 中的生命周期接口；不要误认为它对应 `interface.go` 的所有内容。

## 扩展指南

- 新增任务类型：实现独立的 `Extension`（通常复用 `BaseScheduler`），在启动接线处调用 `RegisterSchedulerFactory`。新增 scheduler 行为测试应放在独立 `*_test.rs`，优先扩展 [`scheduler_test.rs`](scheduler_test.rs)、[`scheduler_nokit_test.rs`](scheduler_nokit_test.rs) 或业务目录测试，不要把测试内嵌进 `interface.rs`。
- 新增持久化能力：先确认它是 scheduler 的最小必要边界，再向 `TaskManager` 添加方法，并同步生产 storage adapter、[`test_support.rs`](test_support.rs) 的 `TestTaskManager`、业务测试中的 mock，以及 Go 对照差异说明。涉及多表写入时必须明确事务/重试稳定性。
- 新增状态或步骤：同步状态常量、`TaskBase::is_done`（若为终态）、`BaseScheduler::schedule_once` 分派、存储查询/状态转换、指标与独立回归测试；字符串拼写会进入持久化协议，兼容风险高。
- 修改资源规则：`runtime_slots`、`slots.rs` 的排序/预留逻辑和 `ExtraParams` 必须一起审查。测试应覆盖上限非正、目标步骤为空/命中/不命中、required slots 小于上限等边界。
- 新增带取消扩展回调：优先覆盖相应 `*_with_context`，保留旧方法的兼容实现；耗时操作应转接 `Context::cancellation_flag` 或周期检查 `is_cancelled`。不要复用已取消 context。
- 新增 cleaner 能力：普通清理实现 `Cleaner`；需要同类型批处理时同时实现 `BatchCleaner` 并从 `batch_cleaner` 返回 `Some(self)`；需要 owner 侧外部文件回收时类似暴露 `ExpiredFileCleaner`。所有副作用必须幂等，并测试“清理成功、历史迁移失败后重试”及部分失败。
- 修改注册表：保留查询时克隆 `Arc`、回调时不持锁的性质；若引入注销或重复注册检查，要补并发与测试隔离用例。全局注册会影响同进程测试，测试类型名应唯一或串行保护。
- 性能关注点：`Task`/meta 与工厂快照会 clone；大 meta、高频 tick 或大量注册 cleaner 时应测量分配。不要为减少 clone 而让回调持有注册表锁或 manager 内部锁。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/framework/scheduler` 覆盖目标及独立测试；`node --file pkg/dxf/framework/scheduler/interface.rs --offset 1/487` 读取完整 626 行；`query` 核对 `TaskManager`、`Extension`、`Scheduler`、`SchedulerFactory`；`explore` 核对 scheduler/cleaner 注册和查询调用边。
- 源码：[`interface.rs`](interface.rs)（全部常量、数据类型、trait、注册表及辅助逻辑）、[`lib.rs`](lib.rs)（模块声明与再导出）、[`scheduler.rs`](scheduler.rs)（`BaseScheduler` 对接口的消费）、[`scheduler_manager.rs`](scheduler_manager.rs)（工厂查询、生命周期、清理与排序）、[`nodes.rs`](nodes.rs)、[`slots.rs`](slots.rs)。
- crate 与架构：[`Cargo.toml`](Cargo.toml)（crate 名、无条件依赖及 Windows 条件依赖）、[`pkg/dxf/framework/doc.go`](../doc.go)（owner/follower、slot、scope、任务顺序和状态机契约）。
- Go 对照：[`interface.go`](interface.go)（TaskManager/Extension/Param/注册表/cleaner 契约）及 [`scheduler.go`](scheduler.go)（Go Scheduler 生命周期接口）。
- Rust 独立测试：[`scheduler_manager_nokit_test.rs`](scheduler_manager_nokit_test.rs) 覆盖排名、未知/已知工厂、无需资源状态、批量/单项/过期文件清理及失败重试；[`scheduler_manager_test.rs`](scheduler_manager_test.rs) 覆盖清理后 meta 迁移和有界批次；[`scheduler_nokit_test.rs`](scheduler_nokit_test.rs) 覆盖 prepare 与状态迁移；[`scheduler_test.rs`](scheduler_test.rs) 覆盖工厂注册、取消、暂停、错误和 manager 驱动；[`pkg/dxf/importinto/scheduler_test.rs`](../../importinto/scheduler_test.rs) 覆盖真实业务扩展、运行时槽位和已注册工厂复用；[`pkg/dxf/importinto/clean_up_test.rs`](../../importinto/clean_up_test.rs) 覆盖 import cleaner 注册与能力。
- 本任务为纯文档分析，按计划不运行 Cargo；最终只运行固定 11 章节的文件结构校验，并人工核对链接、符号名、调用方向以及 Rust/Go 差异。
