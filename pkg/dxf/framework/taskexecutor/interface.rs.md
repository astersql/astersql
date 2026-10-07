# `pkg/dxf/framework/taskexecutor/interface.rs`

## 文件定位

本文件是 `astersql-dxf-framework-taskexecutor` crate 的执行侧契约层。`lib.rs` 将 `interface` 声明为私有模块后用 `pub use interface::*` 公开其类型；同 crate 的 `manager.rs`、`slot.rs`、`task_executor.rs` 和 `register.rs` 分别消费这些契约，完成节点轮询、槽位分配、单任务执行和工厂注册。它位于 DXF（Distributed eXecution Framework）执行节点一侧：调度侧决定任务及步骤，本文件描述执行节点看到的任务数据、取消信号、持久化访问接口和业务扩展点，本身不实现轮询或数据库访问。

`Cargo.toml` 将 crate 名定义为 `astersql-dxf-framework-taskexecutor`，Go 对照包为 `pkg/dxf/framework/taskexecutor`。当前正常构建依赖只有 `astersql-lightning-log`；大量未来依赖被放在 `target.'cfg(any())'`（恒假条件）下，说明当前 Rust crate 仍是聚焦移植边界，不能仅因 trait 存在就推断真实存储、会话或指标组件已经接入。

## 核心职责

1. 统一错误和取消语义：`ExecutorError`、`Result<T>`、`Context`/`ContextState` 为执行器生命周期提供可克隆的父子取消树及可选原因。
2. 定义任务模型：`TaskType`、`Step`、`TaskState`、`SubtaskState`、`TaskBase`/`Task`、`SubtaskBase`/`Subtask` 和 `TaskExecInfo` 是 manager、slot 与 executor 之间的共享数据。
3. 定义资源模型：`NodeResource::GetStepResource` 根据任务运行时 slot 份额计算 `StepResource`。
4. 隔离持久化：`TaskTable` 抽象任务、子任务、节点元数据、检查点和新会话操作，使 `BaseTaskExecutor` 不依赖某个具体系统表实现。
5. 隔离生命周期和业务逻辑：`TaskExecutor` 描述单任务执行器；`StepExecutor` 描述一个步骤内单个 subtask 的实际执行；`Extension` 由任务类型提供幂等判断、步骤执行器工厂和可重试错误判断。
6. 提供最小基类：`BaseStepExecutor` 直接采用 `StepExecutor` 默认方法，适用于无需业务执行的占位或默认扩展，但不等于完整业务实现。

## 主要符号

- `ExecutorError(pub String)` 与 `Result<T>`：错误仅保存字符串，`Display` 原样输出，并实现标准 `Error`。它没有结构化错误码或 source 链，调用方目前会对部分消息（例如 `"task not found"`、`"context canceled"`）做分支判断。
- `ContextState { cancelled, cause, parent }`：私有共享状态。`cancelled` 是 `AtomicBool`，`cause` 是互斥保护的首个本地原因，`parent` 构成只向上查询的取消链。
- `Context`：`Background` 创建根上下文，`Child` 关联父状态，`Cancel`/`CancelWithCause` 取消当前节点，`Done` 和 `Cause` 同时观察本节点与父节点。取消不会反向传播给父节点，也不会主动唤醒阻塞操作。
- `TaskState`：`Pending`、`Running`、`Modifying`、`Pausing`、`Reverting`、`Succeed`、`Reverted`、`Failed`。本文件只枚举状态，不在类型层强制状态迁移。
- `SubtaskState`：`Pending`、`Running`、`Succeed`、`Failed`、`Canceled`、`Paused`；同样由上层执行流程维护转换。
- `TaskBase`：保存 `ID`、业务 `Key`、任务 `Type`、`State`、`Step`、`Priority`、`RequiredSlots`、`CreateTime`、`Keyspace`。`Compare` 按 `Priority → CreateTime → ID` 升序比较并返回 `-1/0/1`；数值更小的优先级更高。`GetRuntimeSlots` 当前直接返回 `RequiredSlots`。
- `Task` 与 `Subtask`：分别在基础字段外增加不透明 `Vec<u8>` 元数据；解析责任留给具体任务扩展。
- `NodeResource`/`StepResource`：节点容量包括 CPU、内存、磁盘，但步骤配额目前只包括 CPU 和内存。`GetStepResource` 令 CPU 等于任务 slot，内存按 `slots / TotalCPU * TotalMem` 比例换算；`TotalCPU == 0` 时内存为零，磁盘尚未参与计算。
- `SubtaskSummary { RowCount }`：供实时检查点使用的最小摘要；`StepExecutor::RealtimeSummaryJSON` 另可返回包含更多计数器的序列化摘要。
- `TaskTable: Send + Sync`：持久化边界。唯一无默认实现的必需方法是 `GetTaskByID`；多数方法提供空集合、`None`、空字符串或成功 no-op，`UpdateSubtaskSummaryJSON` 则默认返回“不支持完整摘要”错误，`GetTaskBaseByID` 默认从完整任务投影，`WithNewSession` 默认直接执行回调。
- `TaskExecutor: Send + Sync`：要求实现 `Init`、`Run`、`GetTaskBase`、两级取消、`Close` 和 `IsRetryableError`。所有方法均无默认实现。
- `StepExecutor: Send + Sync`：`Init`、`RunSubtask`、摘要、重置和 `Cleanup` 默认无行为；动态 `TaskMetaModified` 与 `ResourceModified` 默认返回 `"not implemented"`，防止静默接受运行时修改。
- `Extension: Send + Sync`：要求具体任务实现 `IsIdempotent`、`GetStepExecutor`、`IsRetryableError`，均无默认行为。
- `BaseStepExecutor`：零字段类型，只继承 `StepExecutor` 默认实现。

## 执行流程

这些定义由 `task_executor.rs::BaseTaskExecutor` 串成实际主链：

1. `manager.rs` 通过 `TaskTable::GetTaskExecInfoByExecID` 找到本节点可执行任务，结合 slot 管理和注册工厂创建 `Arc<dyn TaskExecutor>`，再运行其 `Init`/`Run` 生命周期。
2. `BaseTaskExecutor::Run` 循环调用 `TaskTable::GetTaskByID` 刷新任务；只有 `Running` 或 `Modifying` 继续执行，并用 `GetFirstSubtaskInStates` 拉取当前步骤的 `Pending`/`Running` 子任务。
3. `BaseTaskExecutor::createStepExecutor` 调用 `Extension::GetStepExecutor`，随后调用 `StepExecutor::Init`。工厂或不可重试初始化错误会触发子任务失败处理。
4. `BaseTaskExecutor::runSubtask` 对恢复出的 `Running` 子任务先询问 `Extension::IsIdempotent`；非幂等任务直接失败。`Pending` 子任务先经 `TaskTable::StartSubtask` 取得所有权，再把继承步骤上下文的子上下文传给 `StepExecutor::RunSubtask`。
5. 执行期间会并行监测负载均衡、任务 Meta/资源变更和实时摘要。成功后先取消子上下文、等待监测线程退出，再写完整摘要并调用 `FinishSubtask`；错误则由可重试判断、取消状态和持久化方法决定回到待执行、标记 `Canceled` 或 `Failed`。
6. 步骤变化或退出时，`cleanStepExecutor` 先取消步骤上下文，再调用 `StepExecutor::Cleanup` 并释放 trait object。`CancelRunningSubtask` 只取消步骤上下文且附带取消原因；`Cancel` 取消整个任务上下文；`Close` 在任务取消后再清理步骤执行器。
7. `RequiredSlots` 变化时，`NodeResource::GetStepResource` 生成新配额并传给 `ResourceModified`。缩容是先通知业务、再交换 slot；扩容是先取得 slot、再通知业务，通知失败则回滚 slot。

## 数据与状态

`TaskBase` 是调度与资源判断所需的轻量快照，`Task` 的 `Meta` 才承载业务参数；相应地，`SubtaskBase` 表示归属、步骤、状态和节点，`Subtask::Meta` 承载执行输入/输出。所有 Meta 都是不透明字节，本文件不规定编码格式。

默认值需要谨慎：任务和子任务 ID/Step 为零、字符串为空、状态为 `Pending`，任务创建时间是 `UNIX_EPOCH`；这些值方便测试和渐进构造，但不证明对象已持久化或可调度。`TaskExecInfo` 当前只包装 `TaskBase`。

排序不变量由 `TaskBase::Compare` 给出：先比较数值较小者优先的 `Priority`，同优先级时创建更早者优先，仍相同时 ID 更小者优先。`slot.rs` 依赖这一顺序完成排队和抢占；`slot_test.rs::test_task_base_compare_uses_priority_then_create_time_then_id` 明确覆盖三个键及相等情况。

资源换算使用浮点比例后截断为 `i64`，因此只保证近似按 CPU slot 比例分配内存；未校验负 slot、slot 超出 CPU、负内存或算术精度，调用方应提供合法容量。`TotalDisk` 是保留数据，当前 `StepResource` 没有磁盘字段。

## 依赖与调用关系

上游直接消费者包括：

- `manager.rs` 持有 `Arc<dyn TaskTable>` 与按任务 ID 保存的 `Arc<dyn TaskExecutor>`，创建、取消和回收执行器；其 `DefaultExtension` 实现 `Extension`。
- `register.rs` 将任务类型映射到返回 `Arc<dyn TaskExecutor>` 的工厂闭包。
- `slot.rs` 调用 `TaskBase::Compare` 和读取 `RequiredSlots` 实现容量与优先级管理。
- `task_executor.rs` 是最主要消费者：它实现 `TaskExecutor for BaseTaskExecutor`，使用全部三个扩展/存储 trait、父子 `Context`、状态枚举和资源结构驱动 subtask。
- `pkg/dxf/example/task_executor.rs` 展示业务接入：任务工厂构造具体 `TaskExecutor`，`exampleExtension` 返回具体 `StepExecutor`，后者实现 `RunSubtask`。

下游仅依赖 Rust 标准库的 `Any`、排序、格式化、`Arc`/`Mutex`/`AtomicBool` 与 `SystemTime`，没有直接数据库或异步运行时依赖。`TaskTable::AcquireTaskRuntime` 的返回类型引用 crate 根导出的 `TaskRuntime`（定义在 `task_executor.rs`），形成同 crate 内的接口互引；该方法默认 `Ok(None)`。

RustCodeGraph 将目标文件识别为 96 个符号，并报告被 `manager.rs`、`task_executor.rs`、`slot.rs`、示例和多份测试使用。由于对四个 trait 的精确 callers/callees 图查询没有返回静态边，本说明用上述实际 trait object 字段、impl 和方法调用点补齐证据，不把缺失图边解释为“无人调用”。

## 错误处理与边界

`ExecutorError` 只按消息相等比较，适合当前移植层，却会让错误分类依赖字符串稳定性。新增错误分支时应优先复用现有构造函数/常量语义，并检查 `task_executor.rs` 中按文本判断的路径，避免只改消息导致控制流变化。

`Context::CancelWithCauseInternal` 只在本地原因为空时写入，因此首个显式原因获胜；随后用 Release 写入取消标志，`Done` 用 Acquire 读取。`Cause` 先读本地原因，再递归读父原因；若 `Mutex` 中毒，本地原因会被忽略并继续查询父级。无原因 `Cancel` 会令 `Done == true` 但 `Cause == None`。它不是完整的 Go `context.Context`：没有 deadline、value、完成通道或阻塞唤醒，等待代码必须主动轮询。

`TaskTable` 的多数默认实现是兼容性兜底，不是成功持久化的保证。生产实现若遗漏 `StartSubtask`、`FinishSubtask`、失败/取消/暂停、检查点等覆盖，调用会返回 `Ok(())` 却不改变外部状态；新增生产表实现时必须逐项审计，而不能以“trait 已实现、可编译”为完成标准。相反，完整 JSON 摘要默认显式报错，成功子任务最终持久化摘要时会传播失败并阻止静默丢失。

`StepExecutor` 的 Meta/资源动态修改默认失败，这是有意的能力边界。业务不支持热修改时应让上层收到错误；支持时必须在具体实现中保证修改的原子性和回滚语义。`Init` 和 `GetStepExecutor` 的不可重试错误会被视为致命错误，具体可重试性由 `Extension::IsRetryableError` 决定。

## 并发与资源生命周期

四个公开 trait 都要求 `Send + Sync`，因为 manager 将执行器放入 `Arc` 并在独立线程运行，`BaseTaskExecutor` 还会为平衡检测、参数修改和摘要上报创建监测线程。实现方不得用非线程安全的内部可变状态绕过这一约束；共享业务状态应自行加锁或使用原子类型。

`Context` 克隆只克隆 `Arc`，所以同一上下文的克隆共享取消状态；`Child` 创建独立状态并保存父 `Arc`。父取消对子孙可见，子取消不影响父或兄弟。没有子列表意味着取消传播是读取时递归判断，不需要父节点广播，但深链查询成本随层级增长。

资源生命周期顺序是重要契约：步骤执行器通过 `Extension` 懒创建并 `Init`，subtask 的监测线程在 `RunSubtask` 前启动，结束时先取消其上下文并 join，步骤结束再 `Cleanup`。`TaskExecutor::Cancel` 与 `Close` 被刻意拆开：前者只发停止信号，后者负责清理；实现者应保持 `Close` 可重复且不能让长期工作绑定到传给 `Init` 的 manager 上下文。

slot 和内存配额只是逻辑预算，不在本文件中申请线程、内存或磁盘。`GetStepResource` 的结果由 `task_executor.rs::tryModifyTaskRequiredSlots` 传给业务执行器；真正释放/扩张资源仍是具体 `StepExecutor::ResourceModified` 的责任。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/taskexecutor/interface.go`。核心分层一致：Go 的 `TaskTable`、`TaskExecutor`、`Extension` 和 `BaseStepExecutor` 分别对应 Rust 同名 trait/类型；`TaskExecutor` 的 manager/task/step 上下文树、两级取消、执行器关闭语义，及 `Extension` 的幂等、步骤工厂和瞬态错误判断均被保留。

主要移植差异如下：

- Go 复用 `proto.Task`/`proto.Subtask`/`proto.StepResource`、`storage.TaskExecInfo` 和 `execute.StepExecutor`；Rust 当前把这些基础模型及一个较小的 `StepExecutor` 契约直接放在本文件。另有 `taskexecutor/execute/interface.rs` 的执行管线接口，不能与本 trait 因同名而混淆。
- Go `context.Context` 的取消通道、deadline/value 能力在 Rust 中缩减为轮询式父子取消和可选原因。
- Go `TaskTable` 每个方法都必须由实现者提供；Rust 为渐进移植给大量方法提供默认值，降低了测试 fake 成本，也增加了生产实现遗漏覆盖却静默成功的风险。Rust 还增加了 `GetTasksInStates`、`AcquireTaskRuntime`、`UpdateSubtaskSummaryJSON` 等当前执行链需要的能力。
- Go `WithNewSession` 把 `sessionctx.Context` 传给回调；Rust 当前回调无会话参数且默认原地执行，所以不能声称已经等价提供独立数据库会话。
- Go `BaseStepExecutor` 嵌入 `execute.StepExecFrameworkInfo`；Rust `BaseStepExecutor` 是零字段 no-op 类型，不携带该框架信息。
- Rust 额外暴露 `RealtimeSummaryJSON` 以持久化重试累计等完整计数；Go 同路径接口仅展示 `RealtimeSummary`。

Go 回归证据集中在 `task_executor_test.go`（步骤工厂、幂等恢复、取消、修改和错误分支）、`task_executor_testkit_test.go`（真实执行流程）以及 `slot_test.go`（任务排序/slot 行为）。Rust 对应独立测试是同目录的 `task_executor_test.rs`、`task_executor_testkit_test.rs`、`manager_test.rs`、`slot_test.rs` 和 `register_test.rs`，符合测试逻辑不内嵌生产源文件的仓库约束。

## 扩展指南

新增任务类型时，应实现 `Extension`：明确恢复中的 `Running` subtask 是否幂等；在 `GetStepExecutor` 中依据 `Task` 的步骤/Meta 创建业务执行器；用 `IsRetryableError` 严格区分瞬态与永久错误。随后实现或复用 `TaskExecutor`，并通过 `register.rs` 的工厂注册。最小测试应放在独立 `*_test.rs`，覆盖工厂命中、初始化失败、重试分类、成功完成和非幂等恢复，必要时与 Go 同名测试逐分支对齐。

新增 `StepExecutor` 能力时，优先修改具体实现，而不是改变默认行为。若支持任务 Meta 或资源热修改，分别实现 `TaskMetaModified`/`ResourceModified`，测试缩容与扩容顺序、通知失败的 slot 回滚和本地快照只在成功后更新。若提供实时摘要，应同时验证 `ResetSummary`、周期检查点、最终 `RealtimeSummaryJSON` 在 `FinishSubtask` 前持久化以及持久化失败传播。

新增 `TaskTable` 实现时，应把默认方法视为待办清单：尤其要覆盖所有会改变 durable state 的方法，并验证 exec ID 所有权条件、状态机条件和失败原子性。`WithNewSession` 若用于真实存储，必须提供隔离的新会话语义；不要沿用默认直接回调后声称具备 Go 等价性。

修改状态、排序或资源公式时，要同步检查 `slot.rs`、`manager.rs` 和 `task_executor.rs` 的分支，并更新同目录独立测试。兼容风险包括持久化状态值与 Go 不一致、错误字符串改变分支、默认 no-op 掩盖数据丢失；性能风险包括取消轮询延迟、父链递归、过多监测线程、频繁加锁和浮点资源换算。任何 Rust 行为调整都应以 Go 文件及 Go 测试为基准，不为通过局部测试删减语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点，目标目录已索引；`files --filter pkg/dxf/framework/taskexecutor` 定位 29 个 Go/Rust 文件；`node --file pkg/dxf/framework/taskexecutor/interface.rs --offset 1 --limit 500` 完整读取 447 行并确认 96 个符号；`query` 精确确认四个 trait 位于本文件。对 trait ID 执行 `callers`/`callees` 未产生静态边，因此再用引用搜索核实真实消费点。
- 生产源码：`pkg/dxf/framework/taskexecutor/interface.rs`（全部类型与默认实现）、`lib.rs`（模块装配与公开再导出）、`manager.rs`（任务发现与执行器生命周期）、`register.rs`（工厂类型）、`slot.rs`（`Compare` 消费点）、`task_executor.rs`（主循环、trait 调用、取消树、资源修改和清理顺序）、`pkg/dxf/example/task_executor.rs`（具体扩展示例）。
- crate 边界：`pkg/dxf/framework/taskexecutor/Cargo.toml`（crate 名、Go 包映射、正常依赖与 `cfg(any())` 下尚未启用的依赖）。
- Go 对照：`pkg/dxf/framework/taskexecutor/interface.go`（165 行完整接口及上下文树注释），以及 `task_executor_test.go`、`task_executor_testkit_test.go`、`slot_test.go` 的对应行为测试引用。
- Rust 独立测试：`slot_test.rs::test_task_base_compare_uses_priority_then_create_time_then_id`；`task_executor_test.rs::test_cancel_running_subtask_propagates_to_step_context`、`test_is_retryable_error_delegates_to_extension`、`test_task_base_get_runtime_slots` 和完整摘要持久化测试；`manager_test.rs`、`register_test.rs`、`task_executor_testkit_test.rs` 的 trait fake 与生命周期覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证文档存在且恰好包含十一个固定二级标题，并人工复核所有“已支持”陈述均有上述源码或测试依据。
