# [`pkg/dxf/framework/scheduler/test_support.rs`](./test_support.rs)

## 文件定位

本文件是 `astersql-dxf-framework-scheduler` crate 的共享测试支撑模块。`lib.rs` 仅在 `#[cfg(test)]` 下声明 `mod test_support`，文件内所有对外符号也都限制为 `pub(super)`；因此它不会进入正常库 API，更不属于线上 Owner 调度链。它存在的目的，是让同 crate 的独立 `*_test.rs` 文件用确定性的内存对象驱动真实 `BaseScheduler`、`Manager`、`NodeManager`、`SlotManager`、balancer 和 slots 逻辑。

`Cargo.toml` 将该 crate 映射到 Go 包 `pkg/dxf/framework/scheduler`，并无专门的 `test_support.go`。Rust 侧把 Go 测试中分散的 gomock 期望与装配代码集中成此模块，以降低状态机测试的重复配置；生产持久化实现仍是 `storage_adapter.rs`，不能把 `TestTaskManager` 当作其等价替代。

## 核心职责

1. `TestTaskManager` 完整实现 `interface.rs::TaskManager`，用 `Mutex<HashMap/Vec>` 提供可预置、可读取的任务、节点、子任务、统计和错误视图。
2. 在状态迁移方法中同时改变内存快照并保留副作用记录，例如 `persisted_subtasks`、`updated_subtasks`、`deleted_nodes`、`transferred_tasks`、`failed_tasks`，使测试能区分“调用发生”与“最终状态正确”。
3. `TestExtension` 实现 `Extension`，以字段脚本化下一步骤、候选节点、子任务 meta、错误分类和一次性回调失败，并用原子计数器记录生命周期回调。
4. `task` 与 `scheduler` 两个构造辅助函数生成最小任务，并装配固定的一节点、八槽位 `BaseScheduler`，供状态机用例聚焦业务分支而非依赖初始化。

## 主要符号

- `TestTaskManager`：线程安全内存桩。`tasks` 是完整任务的权威测试视图；`top_unfinished`、`top_no_resource`、`nodes`、`subtasks`、`active_subtasks`、`state_counts`、`task_errors`、`used_slots` 和 previous-result 字段分别为对应 trait 查询提供脚本数据。
- `TestTaskManager::insert_task` / `task`：测试直接预置和读取完整任务；`task` 对缺失 ID 直接 panic，表达夹具应已正确装配的不变量。
- `TestTaskManager::change_state`：多个状态写接口共用的内部辅助；缺失任务时返回 `SchedulerError`。
- `TaskManager for TestTaskManager`：覆盖 trait 的查询、清理、历史迁移、失败/回滚/暂停/恢复、步骤切换、slot 查询、子任务更新及上一阶段结果读取。
- `TestExtension`：可脚本化扩展。`next_step`、`eligible`、`metas` 决定正常规划结果；`prepare_error`、`plan_error`、`done_error` 注入下一次回调错误；`retryable` 控制错误分类；三个 `*_calls` 统计回调次数。
- `TestExtension::on_prepare`：成功时把 `-prepared` 追加到 `Task.meta`；错误通过 `Option::take` 只触发一次。
- `TestExtension::modify_meta`：按输入顺序把每个修改编码为 `:<kind>=<to>`，提供稳定、可精确断言的测试表示，而非生产序列化协议。
- `task(id, state)`：创建 `required_slots = 1` 的默认任务，其他字段沿用 `Task::default`。
- `scheduler(task, manager, extension, allocated_slots)`：先把任务插入 manager，再创建节点 `n1`（8 CPU）、容量为 8 且已用槽位为 0 的资源视图，最后调用 `BaseScheduler::new`；`allocated_slots` 原样保留以覆盖资源已分配/未分配分支。

## 执行流程

典型测试先构造 `Arc<TestTaskManager>` 与 `Arc<TestExtension>`，通过公开字段预置所需状态，再调用 `task` 和 `scheduler` 获得真实 `BaseScheduler`。`scheduler` 会把初始任务复制到 `TestTaskManager.tasks`，并提供单节点资源环境；随后测试调用 `init` 或 `schedule_once`，真实调度代码只通过 `TaskManager`/`Extension` trait 观察这些夹具。

查询路径通常直接克隆预置容器：例如 `tasks_in_states` 过滤后按任务 ID 排序，`active_subtasks`、统计、错误和上一阶段结果在未预置时返回空集合。写路径则模拟最小必要语义：`switch_task_step` 记录新子任务并写入更新过 state/step 的任务；batch 版本委托相同实现；`fail_task` 同时记录调用、置 Failed 并保存错误；回滚、等待人工解决、暂停、恢复、成功分别写对应状态。

Prepare 流程由两个夹具配合：`TestExtension::on_prepare` 修改传入任务 meta，`TestTaskManager::switch_task_step_after_prepare` 由 `switch_after_prepare` 决定是否保存 `STEP_PREPARED` 快照。规划流程可先消费一次 `plan_error`，否则克隆 `metas`；完成流程递增 `done_calls` 后消费一次 `done_error`。这使失败后的下一次 tick 可自然回到正常路径。

Manager 清理测试通过 `cleanup_tasks` 按终态筛选并截断到 `GetTaskCleanupBatchSize`，再由 `transfer_tasks_to_history` 记录并删除活动任务。节点与负载均衡测试则通过 `delete_dead_nodes` 和 `update_subtask_exec_ids` 同时核对写请求以及内存视图的实际改变。

## 数据与状态

`TestTaskManager` 的字段可分为三类：输入脚本（如 `top_unfinished`、`state_counts`、`cleanup_infos`）、可变领域快照（`tasks`、`nodes`、`active_subtasks`）和副作用日志/计数（`persisted_subtasks`、`deleted_nodes`、`cleanup_info_calls`、`gc_calls`）。测试可以在调用前写入输入，在调用后分别断言领域结果和调用记录。

重要状态约束如下：`tasks_in_states` 保证按 ID 稳定排序；`cleanup_tasks` 只返回 Failed/Reverted/Succeed 并受全局测试批大小限制；`transfer_tasks_to_history` 仅在无注入错误时记录并从活动表移除；`pause_task_on_error` 要求当前状态仍等于调用方传入值，否则返回 `task changed`，并只把指定 step 中 Failed 的活动子任务改为 Paused；`resume_subtasks` 只把 Paused 改为 Pending；`update_subtask_exec_ids` 按子任务 ID 更新所有活动视图。

`TestExtension` 的错误槽使用 `Mutex<Option<SchedulerError>>`，而正常返回值使用克隆，所以错误是一次性的、正常脚本可重复使用。步骤、重试分类和调用计数使用原子变量；默认 `next_step` 为 0、`retryable` 为 false，调用者必须按目标分支显式设置。`modify_meta` 的文本追加格式只是确定性夹具协议，不能作为生产 meta 兼容格式。

## 依赖与调用关系

- 模块边界：`lib.rs` 的 `#[cfg(test)] mod test_support` 是唯一装配入口；`use crate::*` 取得 `TaskManager`、`Extension`、`BaseScheduler`、任务/子任务类型、状态常量、节点与槽位管理器。
- crate 依赖：本文件通过 `TaskCleanupInfo` 直接引用 `astersql-dxf-framework-storage`，并通过 crate 再导出使用 proto 的清理批大小配置；这些依赖均由相邻 `Cargo.toml` 声明。
- 直接消费者：`scheduler_test.rs`、`scheduler_nokit_test.rs`、`scheduler_manager_test.rs`、`scheduler_manager_nokit_test.rs`、`nodes_test.rs`、`slots_test.rs`、`balancer_test.rs`。RustCodeGraph 的文件节点只报告一个模块级使用文件，是因为这些测试模块由 `lib.rs` 装配；`rg` 对 `crate::test_support` 的直接导入补出了上述七个真实消费者。
- 下游：两个 trait 实现由 `scheduler.rs::BaseScheduler`、`scheduler_manager.rs::Manager`、`nodes.rs`、`slots.rs` 与 `balancer.rs` 的真实逻辑调用；辅助 `scheduler` 直接构造 `NodeManager`、`SlotManager`、`Param` 和 `BaseScheduler`。
- 相关测试证据：`scheduler_nokit_test.rs` 使用该夹具覆盖步骤推进、一次性错误、候选节点、暂停/恢复/回滚、Prepare、资源未分配、刷新与修改；`scheduler_manager_test.rs` 覆盖清理批次和历史迁移；其余消费者覆盖 manager 驱动、死亡节点、slot 和执行节点再平衡。

RustCodeGraph 对 `TestTaskManager`、`TestExtension`、`task`、`scheduler` 的精确 callers/callees 未返回可用符号边，所以本节没有据此臆造函数级调用图，而是用索引的文件关系、trait 契约和直接导入/调用位置交叉确认。

## 错误处理与边界

可预期的领域失败通过 `SchedulerError` 返回：任务不存在、暂停时状态已改变，以及 `transfer_error`、`cleanup_error` 和扩展的三个注入错误。扩展错误使用 `take()`，只让紧接着的一次回调失败；清理/迁移错误不会提前改变相应任务集合。`is_retryable_error` 不分析错误内容，只返回原子开关，目的是精确控制状态机分支。

绝大多数测试夹具锁使用 `expect("... lock poisoned")` 或 `unwrap()`；锁中毒、`task()` 读取缺失 ID，以及若调用方破坏夹具前置条件导致的内部 `expect` 都会 panic，而不会转成 `SchedulerError`。这是测试失败策略，不应复制到生产适配器。部分实现有意简化事务性：例如 `switch_task_step` 先记录子任务再写任务快照，`fail_task` 分两次加锁更新状态与错误；它们适合单进程确定性测试，但不证明生产数据库事务具有相同行为。

未预置的集合查询通常返回空值，这既便于最小装配，也可能掩盖遗漏；新增测试若需要证明某查询真的发生，应同时断言对应记录字段或新增显式计数，而不能只依赖最终空结果。`scheduler` 固定单节点且角色为空，不覆盖多节点公平性、scope 过滤或真实容量发现，这些场景需由调用测试显式覆盖或使用其他夹具。

## 并发与资源生命周期

两个 trait 都要求 `Send + Sync`。复合数据以 `Mutex` 保护，简单开关和计数使用 `AtomicBool`、`AtomicI64`、`AtomicUsize`；状态读多采用 Acquire，写和计数采用 Release/AcqRel，使跨线程测试能观察到一致的开关与次数。代码不创建线程、异步任务、通道或运行时，实际并发由被测 `Manager`/调度器驱动。

方法只在克隆、过滤或局部更新时持锁，没有把 guard 返回给调用者。`pause_task_on_error` 在释放任务锁后再取得子任务锁，`update_subtask_exec_ids` 先释放调用记录锁再更新活动视图，避免同时持有两把内部锁；但多容器更新仍不是跨锁原子事务。扩展回调计数先递增再检查错误，因此失败调用也计入次数，这是相关测试判断重试次数的重要语义。

所有共享对象由测试侧 `Arc` 管理；`scheduler` 把 manager/extension 所有权交给 `Param`/`BaseScheduler`，测试通常保留克隆以查看副作用。固定的 `NodeManager` 和 `SlotManager` 随 `BaseScheduler` 释放，无外部资源清理。全局清理批大小由 proto 测试 setter 的恢复闭包管理，使用该夹具的测试仍需及时调用恢复函数，避免跨测试污染。

## 与 Go 版本的对应关系

Go 同目录没有与本文件一一对应的 `test_support.go`。对应意图分布在 `scheduler_nokit_test.go`、`scheduler_manager_nokit_test.go`、`scheduler_test.go`、`nodes_test.go`、`slots_test.go`、`balancer_test.go`，以及生成的 `mock/scheduler_mock.go`：Go 测试用 `mock.NewMockTaskManager`、`schmock.NewMockExtension` 和 `EXPECT()` 逐次规定返回值/调用；Rust 则用 `TestTaskManager`/`TestExtension` 的可变字段与副作用日志表达这些期望。

`TestExtension` 对应 Go `MockExtension` 的关键可控面：下一步骤、eligible instances、子任务规划、Prepare、Done、错误是否可重试和 meta 修改。`TestTaskManager` 对应 Go `MockTaskManager` 的存储边界，但额外维护足够真实的内存状态，使同一对象可跨多个 `schedule_once` 使用。Rust `scheduler` 辅助函数则集中替代 Go 测试中反复构造 `BaseScheduler`、mock manager、节点和资源池的代码。

两者不能机械等同：Go gomock 能严格校验调用顺序与次数，Rust 夹具多数方法只记录结果或返回克隆，未使用的预置值不会自动导致测试失败；Rust 的 `modify_meta` 字符串追加、单节点八槽位、一次性错误和非事务多容器更新也都是本地测试约定。对齐 Go 新用例时，应移植被验证的状态与调用契约，而不是照搬 gomock 语法或把夹具行为提升为生产规范。

## 扩展指南

- `TaskManager` trait 新增方法时，应在本文件增加最小但可观察的实现：为输入、最终状态和调用记录选择清晰字段，并同步直接依赖该行为的独立 `*_test.rs`；不要把测试写回此生产候选文件。
- 新增状态迁移时，优先复用 `change_state`，但需要错误字段、子任务联动或调用顺序证据时应显式实现并记录副作用。保持缺失任务返回错误、状态竞争可检测、无关子任务不被改写。
- 扩展 `TestExtension` 时，明确返回值是重复脚本还是一次性脚本；若采用 `Option::take`，应补充“首次失败、下次恢复”的测试，并决定失败回调是否计数。
- 改动 `scheduler` 默认环境会影响大量用例。节点 ID、CPU/容量、已用槽位、server ID 或默认 `allocated_slots` 语义变化前，要检查七个消费者；多节点/特殊 scope 场景更适合由测试在构造后显式配置，避免扩大共享默认值。
- 增强调用验证时，可增加专用原子计数或参数日志，但不要引入依赖真实 SQL、时间或线程调度的非确定性。若需要验证真实事务边界，应测试 `storage_adapter.rs`，而不是让本夹具伪装成数据库。
- 与 Go 新回归对齐时，同时阅读对应 `*_test.go` 与生成 mock 接口，保留实际错误、重试、状态和副作用意图；注意 gomock 的严格期望在字段式夹具中可能需要额外调用日志才能等价证明。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`files --filter pkg/dxf/framework/scheduler` 列出目标、模块、Rust/Go 测试和相邻实现；`node --file pkg/dxf/framework/scheduler/test_support.rs --offset 1 --limit 500` 与 `--offset 501 --limit 120` 覆盖目标 582 行；`query TestTaskManager --kind struct`、`query TestExtension --kind struct` 定位两个主要类型。精确 callers/callees 无输出，故以直接引用搜索补证。
- crate 与模块：`pkg/dxf/framework/scheduler/Cargo.toml`、`pkg/dxf/framework/scheduler/lib.rs`；包内未发现 `doc.go`，因此没有更近的 Go 包契约文件可读。
- trait 与真实实现边界：`pkg/dxf/framework/scheduler/interface.rs`、`pkg/dxf/framework/scheduler/scheduler.rs`、`pkg/dxf/framework/scheduler/scheduler_manager.rs`、`pkg/dxf/framework/scheduler/storage_adapter.rs`。
- Rust 独立测试：`scheduler_test.rs`、`scheduler_nokit_test.rs`、`scheduler_manager_test.rs`、`scheduler_manager_nokit_test.rs`、`nodes_test.rs`、`slots_test.rs`、`balancer_test.rs`。
- Go 对照：`scheduler_nokit_test.go`、`scheduler_manager_nokit_test.go`、`scheduler_test.go`、`nodes_test.go`、`slots_test.go`、`balancer_test.go` 和 `mock/scheduler_mock.go`；确认没有同路径 `test_support.go`。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前运行任务指定结构命令，确认文件存在且恰有 11 个固定二级标题，并人工检查本文没有把测试夹具、Go gomock 能力或简化的内存写入描述成生产保证。
