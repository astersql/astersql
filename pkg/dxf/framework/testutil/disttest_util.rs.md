# `pkg/dxf/framework/testutil/disttest_util.rs`

## 文件定位

[`disttest_util.rs`](./disttest_util.rs) 属于 `astersql-dxf-framework-testutil` crate，是 DXF（Distributed eXecution Framework）集成测试的适配层。它不实现生产调度循环或任务存储，而是把测试提供的闭包包装成步骤执行器、任务执行器扩展和清理器，并用 trait 抽象任务类型注册表与任务运行时，使测试可以用内存桩验证注册、提交、等待和回滚流程。

crate 入口 [`lib.rs`](./lib.rs) 声明 `disttest_util` 模块并再导出其公开 API；[`Cargo.toml`](./Cargo.toml) 将该 crate 映射到 Go 包 `pkg/dxf/framework/testutil`，没有 feature 开关。目标文件直接使用同 crate 的 `context`、`scheduler_util` 和 `task_util`，因此其类型最终连接到 DXF 的 proto、scheduler、taskexecutor、handle 与 storage 等工作区 crate，但本文件自身通过测试抽象隔离这些实现。

## 核心职责

该文件有两组职责：

1. 构造可注入的测试执行组件。`StepExecutor` 把固定 `Step` 与 `RunSubtaskFn` 绑定；`TaskExecutorExtension` 按任务创建步骤执行器并判断错误是否可重试；`Cleaner` 封装任务级清理回调。`GetCommon*` 系列提供与 Go mock 默认期望一致的成功/不可重试行为。
2. 编排测试任务。`RegisterTaskType` 依次注册 scheduler、cleaner、executor，并以 `RegistrationGuard` 保证作用域结束时清理；`SubmitAndWaitTask` 提交 `Example` 任务后等待完成或暂停；`WaitTaskDone*` 通过 `DistributedTaskRuntime` 统一查询历史任务并等待指定终止条件。

它是测试辅助而非生产实现：注册与等待的实际副作用完全由调用者提供的 `TaskTypeRegistry`、`DistributedTaskRuntime` 及回调决定。

## 主要符号

- `TASK_TYPE_EXAMPLE: &str = "Example"`：与 Go `proto.TaskTypeExample` 的公开字符串值对齐，是注册和提交的默认任务类型。
- `RunSubtaskFn`、`GetStepExecutorFn`、`RetryableErrorFn`：分别表示子任务执行、按任务构造执行器、错误可重试判断的线程安全动态回调；均要求 `Send + Sync`，由 `Arc` 共享。
- `StepExecutor { step, run_subtask }`：公开方法 `init` 和 `cleanup` 恒返回成功，`realtime_summary` 恒为 `None`，`run_subtask` 委托注入回调，`step` 返回绑定步骤。
- `TaskExecutorExtension { get_step_executor, is_retryable_error }`：`is_idempotent` 恒为 `true`；另外两个方法分别委托工厂和重试判定回调。
- `Cleaner`：私有回调字段的公开包装，`clean(task_id)` 直接传播回调结果；`GetCommonCleaner` 创建可重复调用的空操作清理器。
- `GetCommonTaskExecutorExt` / `GetTaskExecutorExt` / `GetCommonStepExecutor`：构造上述包装对象。通用任务扩展默认认为所有错误均不可重试。
- `TaskTypeRegistry`：要求实现 scheduler、cleanup、executor 三类注册及全量清空。它刻画本文件所需的最小注册能力，并不等同于某个具体生产注册表类型。
- `RegistrationGuard`：持有 `Arc<dyn TaskTypeRegistry>`，在 `Drop` 中调用 `clear_registrations`。
- `RegisterExampleTask` / `RegisterTaskType`：前者固定使用 `TASK_TYPE_EXAMPLE`，后者实现通用的三阶段注册及失败回滚。
- `RegisterTaskTypeForRollback`：构造会把每个子任务送入 `TestContext::CollectSubtask` 的执行器，用于回滚场景观察。
- `WaitCondition::{DoneOrPaused, Done}`：区分“完成或暂停即可返回”和“只接受真正结束”。
- `DistributedTaskRuntime`：抽象 `submit_task`、`get_task_by_key_with_history` 和 `wait_task`，让等待辅助函数可以由内存桩或真实适配器承载。
- `SubmitAndWaitTask` / `WaitTaskDoneOrPaused` / `WaitTaskDone` / `waitTaskUntil`：提交与等待调用链；所有失败均以 `DxfError` 返回。

## 执行流程

通用执行器路径如下：调用者把 `RunSubtaskFn` 交给 `GetCommonStepExecutor`；`TaskExecutorExtension::get_step_executor` 根据当前 `Task` 选择 `task.base.step` 并返回该执行器；相邻文件 [`executor_util.rs`](./executor_util.rs) 的 `run_registered_subtask` 随后按 `init → run_subtask → cleanup` 执行。目标文件只定义各阶段行为，不主动启动任务或线程。

注册路径从 `RegisterExampleTask` 进入 `RegisterTaskType`。后者严格按 `register_scheduler → register_cleanup → register_executor` 顺序执行：任一步返回错误，就调用一次 `clear_registrations` 并原样返回错误；全部成功则返回 `RegistrationGuard`。守卫离开作用域时再次执行全量清理，从而避免全局式测试注册污染后续用例。`RegisterTaskTypeForRollback` 在此路径前增加一个回调：每次运行子任务先由 `TestContext::CollectSubtask` 记录，再返回成功。

提交等待路径从 `SubmitAndWaitTask` 开始。它用任务 key、`Example` 类型、`getTaskKS(next_generation_kernel)` 生成的 keyspace、并发度、目标 scope 和空 meta 调用 `submit_task`；提交成功后进入 `WaitTaskDoneOrPaused`。两个公开等待函数都委托 `waitTaskUntil`：先调用 `get_task_by_key_with_history` 获得 ID，再把 ID 和 `WaitCondition` 交给 `wait_task`。即使查询结果已经暂停，`Done` 条件也不会把暂停误判为完成，最终判定仍由 runtime 的 `wait_task` 实现。

## 数据与状态

`StepExecutor` 和 `TaskExecutorExtension` 本身没有可变字段；克隆只会复制 `Step` 或增加回调 `Arc` 的引用计数。`Cleaner` 同样共享一个闭包。闭包捕获的数据是否可变及其一致性由调用者负责，但 `Send + Sync` 约束保证包装对象可跨线程共享。

注册状态不保存在本文件，而存于 `TaskTypeRegistry` 实现。`RegistrationGuard` 只保存注册表引用，没有记录“已成功注册到第几步”；因此失败路径和正常析构都采用全量清空，而不是逐项撤销。

任务状态使用 `context::TaskBase`、`TaskState` 和 `Step`。`SubmitAndWaitTask` 不使用 `submit_task` 返回的 ID，而是按 key 再查一次含历史记录的任务，以兼容任务已转入历史表的情况。`WaitCondition` 仅描述等待合同；何谓 done 以及如何阻塞/轮询由 runtime 决定。Rust 回滚测试 [`framework_rollback_test.rs`](../integrationtests/framework_rollback_test.rs) 的内存 runtime 明确将 `DoneOrPaused` 实现为 `task.is_done() || state == Paused`，将 `Done` 实现为仅 `task.is_done()`。

## 依赖与调用关系

上游方面，`lib.rs` 将本模块 API 再导出给集成测试。RustCodeGraph 的文件关系显示 [`framework_test.rs`](../integrationtests/framework_test.rs) 使用本文件；仓库引用搜索还确认 [`framework_rollback_test.rs`](../integrationtests/framework_rollback_test.rs) 实现 `DistributedTaskRuntime` 并调用 `SubmitAndWaitTask`。同 crate 的 [`executor_util.rs`](./executor_util.rs) 调用 `GetCommonTaskExecutorExt` 与 `GetCommonStepExecutor`，而 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 通过 `run_registered_subtask` 间接验证这些包装器。

下游方面，目标文件依赖 `context::{DxfError, Step, Subtask, Task, TaskBase, TaskState, TestContext}` 表达错误、任务和收集状态，依赖 `scheduler_util::SchedulerExtension` 表达调度扩展，依赖 `task_util::getTaskKS` 选择普通或下一代内核 keyspace。注册器和 runtime 只以 trait 形式被调用，因此这里没有直接访问数据库、全局注册表或生产 `handle` API。

RustCodeGraph 对目标文件的精确 `callers`/`callees` 命令未产出函数级边；上述调用关系因此由它的文件使用关系、符号查询和 `rg` 引用结果交叉确认，不能把缺失的图边解释为“没有调用者”。

## 错误处理与边界

所有可失败公开操作统一返回 `Result<_, DxfError>`，且使用 `?` 保留第一个错误。`StepExecutor::run_subtask`、执行器工厂、重试判定、清理器、注册表和 runtime 的错误均不在本文件内转换或吞掉。

`RegisterTaskType` 的边界是全量清理：scheduler 或 cleaner 注册失败时后续阶段不会执行；executor 注册失败时此前两项已产生的状态也由 `clear_registrations` 清除。清理本身无返回值，所以无法向调用者报告回滚失败。正常 `Drop` 也会清空同一注册表中的全部项目；若一个 registry 同时承载其他测试注册，守卫会一并清除，调用者必须隔离作用域或提供隔离实现。

`GetCommonTaskExecutorExt` 默认所有错误不可重试，不能用于需要重试策略的用例；这类测试应显式使用 `GetTaskExecutorExt`。`StepExecutor::init`、`cleanup`、`realtime_summary` 与 `Cleaner` 是刻意的中性测试桩，不能据此推断生产实现没有初始化、清理或摘要逻辑。

`waitTaskUntil` 总是先按 key 查询，再调用 `wait_task`；不存在任务、key 不匹配、提交失败或等待条件不满足时均依赖 runtime 返回错误。源码中对“`Done` 且当前为 `Paused`”的分支只有注释而没有额外操作，其有效语义来自继续调用 `wait_task(Done)`，不是本地循环。等待是否支持取消、超时或轮询间隔也不由此文件保证。

## 并发与资源生命周期

动态回调及两个外部抽象均要求 `Send + Sync`，并用 `Arc` 管理共享所有权；这允许多线程测试克隆执行器扩展或清理器。目标文件不创建线程、任务、通道、锁或事务，也不为闭包捕获的共享状态提供同步，调用者通常需自行使用 `Mutex`、原子量等机制。

资源清理的核心是 RAII：成功注册返回的 `RegistrationGuard` 在最后一个守卫值离开作用域时执行 `Drop`，但 `RegistrationGuard` 不实现 `Clone`，因此生命周期边界清晰。注册过程中失败则在返回前立即清理。`Cleaner` 的生命周期由注册表或调用者持有的 `Arc` 决定；默认 cleaner 无状态且可重复调用，`framework_cleanup_routine_is_repeatable_after_task_transfer_error` 用两个不同 task ID 验证了这一点。

等待生命周期完全委托给 `DistributedTaskRuntime::wait_task`。本文件既不忙等也不持锁跨调用；runtime 必须自行保证任务对象并发更新时的可见性与终止条件判断。

## 与 Go 版本的对应关系

Go 对照文件是 [`disttest_util.go`](./disttest_util.go)。两端保留了相同的助手分组和主意图：通用 executor 默认幂等且错误不可重试，step executor 的 init/cleanup 成功且摘要为空，rollback executor 收集子任务，提交 `Example` 后等待 done-or-paused，`WaitTaskDone` 只接受 done。

Rust 并非对 Go 机制的逐行复刻，主要差异如下：

- Go 通过 gomock 对生产接口设置 `AnyTimes` 期望；Rust 用具体包装结构和 `Arc<dyn Fn>` 实现同样的可注入行为。
- Go 直接注册到 scheduler/taskexecutor 的进程级全局 factory，并用 `testing.TB.Cleanup` 清空；Rust 通过 `TaskTypeRegistry` 注入注册动作，以 `RegistrationGuard::drop` 承担清理。
- Go 直接调用 `storage.GetDXFSvcTaskMgr`、`handle.SubmitTask` 和 `handle.WaitTask`，并用 `require.NoError` 终止当前测试；Rust 通过 `DistributedTaskRuntime` 注入这些能力并返回 `DxfError`，方便内存测试断言失败。
- Rust `SubmitAndWaitTask` 多出 `next_generation_kernel` 参数，并通过 `getTaskKS` 在 `""` 与 `"SYSTEM"` 间选择 keyspace；Go 当前对照调用无参 `getTaskKS()`。Rust 还将 Go 的等待谓词显式建模为 `WaitCondition`。
- Go 的 step 回调接收 `context.Context`，cleaner 接收 context 和 task；当前 Rust 回调只接收 `&Subtask`，cleaner 只接收 task ID。取消传播与上下文相关行为因此不在本文件的 Rust 抽象范围内。

这些差异说明当前 Rust 文件是可测试的迁移适配层，不应把 trait 的存在写成已经接入全部 Go 全局运行时的证明。

## 扩展指南

新增步骤执行行为时，优先扩展 `StepExecutor` 或构造它的闭包，并在独立测试文件中经 `run_registered_subtask` 覆盖 `init/run/cleanup` 顺序、错误传播和摘要；不要把测试内嵌回生产 `.rs`。改变默认重试语义应修改 `GetCommonTaskExecutorExt`，同时补充错误分类测试，确认不会造成无限重试或与 Go 默认 `false` 偏离。

新增注册类别或修改顺序时，应更新 `TaskTypeRegistry`、`RegisterTaskType` 及其独立测试，至少覆盖每个阶段失败时的短路、全量清理次数和守卫析构；要评估“清空全部注册”对并行测试的兼容风险。若接入具体生产注册表，适配应放在拥有生产依赖的模块，避免让 testutil 重新实现 scheduler/taskexecutor 内部逻辑。

新增等待条件时，应同时修改 `WaitCondition`、两个 runtime 适配面和 Go 对照语义，并为“初始任务已经处于目标状态”“暂停后等待 Done”“任务已进入历史表”“不存在 key”“runtime 返回错误”建立独立测试。若引入超时或取消，需在 `DistributedTaskRuntime` 合同中显式表达，而不是在 `waitTaskUntil` 中隐式忙等。改变提交参数时要同步检查 `TASK_TYPE_EXAMPLE`、`getTaskKS`、并发度、scope 与 meta 的传递，不得把 `submit_task` 返回 ID 和按 key 回查所得 ID 混为一谈。

性能上，本文件只有 `Arc` 克隆和 trait/闭包动态分派，通常不是测试瓶颈；真正的等待和存储开销在 runtime 实现。兼容性风险主要来自公开的 Go 风格函数名、固定任务类型字符串、注册顺序、默认不可重试策略和 RAII 清理范围。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标工程；`node --file pkg/dxf/framework/testutil/disttest_util.rs` 核对了完整 303 行及所有常量、别名、结构、trait、函数和 `Drop` 实现；`query` 分别定位 Rust/Go 的 `RegisterTaskType`、`RegisterTaskTypeForRollback`、`SubmitAndWaitTask`、`waitTaskUntil`；文件使用关系指向 `pkg/dxf/framework/integrationtests/framework_test.rs`。精确 callers/callees 查询没有返回函数级边，因此未据此作否定结论。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs) 证明 crate 名、Go 包映射、模块声明和公开再导出；未发现条件 feature。
- Go 对照：[`disttest_util.go`](./disttest_util.go) 核对 gomock 默认行为、全局 factory 注册/清理、rollback 收集、提交参数以及 done/done-or-paused 谓词。
- Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的 `executor_helpers_run_and_cleanup_with_raii_registration` 验证执行器回调与相邻注册守卫；[`framework_test.rs`](../integrationtests/framework_test.rs) 的 `framework_executor_failure_is_returned_without_hiding_cleanup`、`framework_cleanup_routine_is_repeatable_after_task_transfer_error`、`framework_executor_factory_error_is_not_replaced_by_success` 验证错误与默认清理行为；[`framework_rollback_test.rs`](../integrationtests/framework_rollback_test.rs) 的 `RollbackRuntime` 与 `test_framework_rollback` 验证 `DistributedTaskRuntime` 合同、提交参数后的状态流和 `DoneOrPaused` 判定。
- 引用搜索：`rg` 确认 `executor_util.rs`、上述 Rust 测试及多个 Go 集成测试对这些助手的调用。当前未发现直接覆盖 `RegisterTaskType` 三个注册阶段分别失败、`RegistrationGuard` 全注册清理或 `WaitTaskDone` 暂停分支的 Rust 测试，这些属于扩展时应补的验证缺口，而不是已验证能力。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务给定命令验证文档恰有十一个固定二级章节。
