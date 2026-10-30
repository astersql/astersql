# `pkg/dxf/framework/taskexecutor/task_executor.rs`

## 文件定位

的 `BaseTaskExecutor` 位于管理器和任务类型专属的 `StepExecutor` 之间：它实现通用任务/子任务状态机，具体业务由 `Param.Extension` 创建的步骤执行器完成。

## 核心职责

`BaseTaskExecutor` 集中承担下列与任务类型无关的职责：

1. 维护从任务表刷新而来的本地 `Task` 快照，只在 `Running` 或 `Modifying` 状态下继续工作。
2. 在当前步骤内轮询 `Pending`/`Running` subtask，并在长时间无 subtask 时用有上限的指数退避退出，以归还管理器持有的资源。
3. 按步骤懒创建、初始化和清理 `StepExecutor`，把真正的 `RunSubtask` 业务委派出去。
4. 执行 subtask 的状态转换：`Pending → Running → Succeed`，以及取消、不可重试失败、可重试失败和遗留 `Running` subtask 的恢复决策。
5. 在 subtask 执行期间并行监控调度均衡、任务 `Meta`/`RequiredSlots` 修改和实时进度。
6. 通过 `Context` 传播任务级、步骤级和 subtask 级取消，并在 `Run` 的 panic 边界尽力失败一个 subtask，令调度端感知致命错误。

该文件不负责具体任务算法、任务类型注册、全节点执行器集合管理或 slot 的底层分配策略；这些职责分别由 `StepExecutor`/`Extension`、`register.rs`、`manager.rs` 和 `slot.rs` 承担。

## 主要符号

- `SubtaskCheckInterval()`、`MaxSubtaskCheckInterval()`：从两个 `AtomicU64` 读取轮询基础间隔（默认 300ms）和上限（默认 2s）。`SetSubtaskCheckIntervalForTest` 原子替换二者并返回旧值，供独立测试缩短等待；它不是 `cfg(test)`，因为下游 crate 的测试也需要调用。
- `DetectParamModifyInterval`：运行中参数检测间隔，固定为 5s。`maxChecksWhenNoSubtask` 为 7，配合 `runLoop` 的退避决定空闲退出。
- `ErrCancelSubtask()`：显式取消当前 subtask 的哨兵错误；`ErrNonIdempotentSubtask()`：拒绝重跑遗留的非幂等 `Running` subtask。
- `TaskRuntime`：最小 runtime 抽象。`Init` 使用 `CheckTaskKeyspace` 校验任务 keyspace；`AsAny`、`Release` 提供可选能力和生命周期钩子，但本文件不调用 `Release`，runtime 的所有权在 manager。
- `Param`：构造依赖，包含 `TaskTable`、`slotManager`、`NodeResource`、执行节点 ID、任务类型 `Extension` 和可选 `TaskRuntime`。`NewParamForTest` 只构造无 runtime 的测试参数。
- `BaseTaskExecutor`：核心对象。`task` 是可更新的任务快照；`stepExec`、`stepExecStep`、`stepCtx` 表示当前步骤环境；`currSubtaskID` 供均衡监控识别当前 subtask；`selfRef` 使监控线程可以安全持有 `Weak` 升级后的 `Arc`；`sampleLogger` 用于执行结果日志。
- `NewBaseTaskExecutor`：构造 `Arc<BaseTaskExecutor>`，初始化锁、原子状态和日志器，并把自己的 `Weak` 写入 `OnceLock`。
- `Run`/`runLoop`：公开 panic/清理边界与内部主循环。`Run` 捕获 Rust panic，调用 `failOneSubtask` 后统一 `cleanStepExecutor`。
- `createStepExecutor`/`cleanStepExecutor`：按当前 step 管理 `StepExecutor` 生命周期。创建失败或不可重试的 `Init` 失败会失败一个 subtask；清理先取消 step context，再调用 `Cleanup`，其错误不传播。
- `runSubtask`：单个 subtask 的状态机和监控线程编排入口。
- `detectAndHandleParamModify`/`tryModifyTaskRequiredSlots`：把持久化任务参数变化应用到正在运行的步骤，并只在成功后更新本地快照。
- `checkBalanceSubtask`：检查当前节点上的 `Running` subtask 集合；当前 subtask 被调度走时取消本地执行，多余的幂等 subtask 回退到 `Pending`，非幂等者标记 `Failed`。
- `updateSubtaskStateAndErrorImpl`、`startSubtask`、`finishSubtask`、`failOneSubtask`：所有持久化状态转换的集中封装，并通过 `retry` 最多尝试三次。
- `impl TaskExecutor for BaseTaskExecutor`：把固有方法适配为 manager 使用的 trait object API。

## 执行流程

管理器侧流程如下：`TaskExecutorManager` 取得任务及 runtime、分配 slot、构造 `Param`，经注册工厂得到 `Arc<dyn TaskExecutor>`；`Init` 成功后在线程中运行 `Run`，最终执行 `Close`、移除登记并释放 slot。`BaseTaskExecutor::Init` 对非空 keyspace 要求存在 runtime 且校验成功，空 keyspace 在没有 runtime 时允许继续。

`Run` 的主体 `runLoop` 每轮按以下顺序执行：

1. 从 `TaskTable::GetTaskByID` 刷新任务。`task not found` 立即退出，其他读取错误本轮忽略并重试。
2. 若仍是同一步骤但 `Meta` 改变，通知现存 `StepExecutor::TaskMetaModified`；失败时清理步骤执行器并进入下一轮，之后可重建。
3. 若 `RequiredSlots` 改变，先调用 `slotMgr.exchange`；交换失败则退出主循环。成功后替换本地任务快照。
4. 仅 `Running`/`Modifying` 继续；其他任务状态表示当前节点不应再取 subtask。
5. 查询当前 exec、task、step 下第一个 `Pending` 或 `Running` subtask。读取失败重试；无 subtask 时按 `300ms、600ms、1.2s、2s……` 退避，并在计数达到 7 后退出。
6. step 改变时清理旧执行器；随后通过 `Extension::GetStepExecutor` 懒创建并 `Init` 新执行器。已取消的 step context 不再执行 subtask。
7. 调用 `runSubtask`。失败不会直接终止整个循环，scheduler 可通过持久化状态决定后续取消或回滚；取消类错误按 info 记录，其他错误按 error 记录。

`runSubtask` 的详细流程是：

1. 遗留 `Running` subtask 只有在 `Extension::IsIdempotent` 为真时才允许重跑；否则持久化为 `Failed` 并返回 `ErrNonIdempotentSubtask`。`Pending` subtask 必须先通过 `StartSubtask` 抢占为运行态。
2. 记录 `currSubtaskID`，从 step context 派生 subtask context。
3. 启动最多三个监控线程：每 2s 检查均衡；每 5s检测 `Meta`/slot 修改；若 `RealtimeSummary` 非空，则重置摘要并每约 100ms写检查点。
4. 调用 `StepExecutor::RunSubtask`。成功或失败后都先取消 subtask context并 `join` 全部监控线程，保证监控不会越过 subtask 生命周期。
5. 成功路径先持久化可选的 `RealtimeSummaryJSON`，再用 `FinishSubtask` 写回业务修改后的 `subtask.Meta`；任一步失败都会阻止成功完成。
6. 失败路径由 `markSubTaskCanceledOrFailed` 分类：显式 `ErrCancelSubtask` 写 `Canceled`；任务/manager 的普通 context 取消保留 `Running`；可重试错误也保留原状态；其他错误写 `Failed`。

## 数据与状态

`task: RwLock<Task>` 是本文件的权威本地快照，但持久化真相仍在 `TaskTable`。`runLoop` 每轮整体替换快照；运行中参数检测只有在回调/slot 交换成功后，才分别通过 `metaModifyApplied` 和 `requiredSlotsModifyApplied` 修改对应字段。这一顺序避免把“尚未应用”的配置误记为已应用，并允许下一轮重试失败的修改。

`stepExec`、`stepExecStep` 和 `stepCtx` 是一个逻辑整体：步骤执行器成功 `Init` 后才写入三者；步骤变化、通知失败或 `Run` 退出时统一清理。`stepCtx` 是取消正在运行步骤的控制点，`CancelRunningSubtask` 在其上写入 `ErrCancelSubtask` cause；`Cancel` 则取消根 `ctx`。

slot 修改有方向相关的不变量。缩容时先通知 `StepExecutor::ResourceModified`，成功后才交换 slot；扩容时必须先抢到 slot，再通知步骤执行器，通知失败会交换回旧 `TaskBase`。只有整个路径成功才更新本地 `RequiredSlots`。

`currSubtaskID: AtomicI64` 使用 Acquire/Release 供执行线程和均衡线程共享。轮询间隔使用进程级原子值，测试修改会影响同一进程中的所有执行器，因此测试文件用互斥夹具串行保护并在结束时恢复旧值。

## 依赖与调用关系

上游调用关系：

- `pkg/dxf/framework/taskexecutor/manager.rs` 通过 `TaskExecutor` trait 调用 `Init`、`Run`、`Close` 和 `CancelRunningSubtask`，并负责 runtime lease 与 slot 的外层生命周期。
- `pkg/dxf/importinto/task_executor.rs` 的多个注册入口调用 `NewBaseTaskExecutor`；统一 IMPORT INTO 实现用 `ImportNodeTaskExecutor` 包装它，以补充任务级指标注销。
- `pkg/dxf/framework/taskexecutor/task_executor_test.rs`、`task_executor_testkit_test.rs` 和 `tests/realtikvtest/importintotest4/recorded_summary_harness.rs` 直接驱动真实 `Run`。

下游依赖主要通过 `crate::*` 暴露的 trait 解耦：

- `TaskTable` 提供任务/subtask 查询、状态转换、均衡回退、摘要/检查点持久化。
- `Extension` 提供 `GetStepExecutor`、`IsIdempotent` 和 `IsRetryableError` 的任务类型策略。
- `StepExecutor` 提供步骤初始化、执行、清理、实时摘要、Meta 修改和资源修改回调。
- `slotManager::exchange` 调整节点资源占用，`NodeResource::GetStepResource` 把 task slot 数换算成步骤资源。
- `Context` 形成根任务、step、subtask、监控线程的层级取消树。

Cargo 中目前唯一正常启用的外部依赖是 `astersql-lightning-log`；大量历史 Go 对应依赖位于 `target.'cfg(any())'`，该条件恒假，不应视为当前 Rust 构建的运行依赖。其余任务表、slot、proto 和执行接口由同一 crate 的模块实现或定义。

## 错误处理与边界

`Run` 是 panic 隔离边界：`catch_unwind(AssertUnwindSafe(...))` 将 panic payload 转成 `ExecutorError`，随后尽力 `FailSubtask`，并无论是否 panic 都清理 step。`failOneSubtask` 和清理错误被刻意吞掉，因为此处已经处于致命/收尾路径，没有更高层可安全恢复；这也意味着失败持久化并非强保证。

普通持久化操作由 `retry` 最多执行三次，等待约 10ms、20ms、40ms；根 context 已取消时提前停止。它不按错误类型区分重试，因此 `startSubtask` 在 Rust 中也会固定重试，最终把错误返回给调用者。

错误分类的关键边界是取消原因而非错误字符串：只有 subtask context 的 cause 等于 `ErrCancelSubtask()` 才写 `Canceled`。根 context 取消表示优雅停机，subtask 保持 `Running`，留给调度器判断；可重试业务错误同样不写失败，以便幂等 subtask 后续重跑。遗留 `Running` 且非幂等则绝不能执行第二次。

锁中毒目前通过 `expect("... lock poisoned")` 转为 panic，再由最外层 `Run` 捕获；在 `Run` 外直接调用相关方法时，这个 panic 边界并不存在。任务表读取错误在若干监控路径中是 best effort：均衡、摘要和周期参数检测会忽略单轮错误并等待后续检查。

## 并发与资源生命周期

`BaseTaskExecutor` 由 `Arc` 共享，manager 的 worker 线程拥有 trait object。构造时存入的 `Weak<BaseTaskExecutor>` 避免对象通过监控闭包自引用；只有成功升级时才启动对应线程。

每个 subtask 的监控线程都由其 context 控制。`runSubtask` 在业务执行返回后先取消 context，再逐一 `join`，所以 `finishSubtask` 或错误状态持久化不会与仍在运行的监控线程无界并发。`wait_or_cancel` 将最长睡眠切成至多 10ms 的片段，使取消不必等待完整的 2s/5s 周期。

生命周期层次是：manager runtime lease/slot → `BaseTaskExecutor` 根 context → step executor/step context → subtask context → 监控 context。`cleanStepExecutor` 先取消 step context，再清空 step 标识并取得执行器调用 `Cleanup`。`Close` 取消根 context并清理 step，但 runtime 的 `Release` 不在这里执行；`manager.rs` 的 `RuntimeLease` 承担外层释放。

并发访问分别由 `RwLock`、`Mutex` 和原子类型保护。多个锁没有以嵌套方式长期持有；调用外部 `RunSubtask` 前已克隆 `Arc<dyn StepExecutor>` 并释放锁。不过 `Cleanup`、`TaskMetaModified`、`ResourceModified` 等部分外部回调是在持有相应 mutex guard 时调用，扩展实现应避免回调进入会再次取得同一锁的基类路径，以免死锁。

## 与 Go 版本的对应关系

当前 Rust 不是逐项等价的完整 Go 实现，已核实的差异包括：

- Go 集成 metrics、flight recorder、failpoint、分步骤日志任务、内存调优信息和框架 checkpoint 注入；Rust 目前仅保留基础 sample logger 与业务状态机。
- Go 的 SQL 状态更新使用 scheduler 级长退避（可达分钟级），Rust `retry` 只有三次短退避。
- Go 的实时摘要只对具体 `storage.TaskManager` 启用，并在监控退出前做一次长重试的最终更新；Rust 只以 `StepExecutor::RealtimeSummary().is_some()` 判断，约 100ms best effort 更新，没有退出前最终重试。
- Go `Close` 主要取消执行并清理任务指标；Rust `Close` 还直接调用 `cleanStepExecutor`。Rust runtime 的释放仍由 manager lease 而非 `TaskRuntime::Release` 直接触发。
- Go `createStepExecutor` 注入 framework info/resource 并建立带 cause 的 runtime cancel；Rust 通过 `Context::Child` 和 `stepCtx` 表达取消，未实现同等的 metrics/checkpoint 注入层。
- Go 成功执行一个 subtask 后会跳过一次轮询等待；Rust 下一轮直接从任务表刷新，但没有单独的 `skipBackoff` 状态，因为其等待只发生在“没有 subtask”的分支。

## 扩展指南

新增任务类型通常不应修改本文件：实现 `Extension` 与 `StepExecutor`，通过 `RegisterTaskType` 的工厂把扩展写入 `Param.Extension`，再复用 `NewBaseTaskExecutor`。业务 subtask 的输出应写入传入的 `Subtask.Meta`，成功后基类会交给 `FinishSubtask` 持久化。

若要扩展通用生命周期，最可能的接入点是：

- 新的步骤初始化能力：`createStepExecutor`，并同步考虑失败是否可重试、是否需 `failOneSubtask`。
- 新的 per-subtask 监控：`runSubtask` 的 monitor 集合；必须绑定 subtask context并在返回前 join。
- 新的动态参数：`detectAndHandleParamModify`；只有回调成功后才能更新 `task` 快照。
- 新的错误类别：`markSubTaskCanceledOrFailed` 与 `Extension::IsRetryableError`；必须明确持久化状态及能否安全重跑。
- 新的持久化字段：`startSubtask`/`finishSubtask` 或 `TaskTable` 接口；不可用“执行函数成功”代替状态写入成功。

修改后应优先扩展独立文件 `pkg/dxf/framework/taskexecutor/task_executor_test.rs`，不要把单元测试写回生产源文件。涉及 manager 生命周期时同步 `manager_test.rs`；涉及真实摘要持久化时参考 `tests/realtikvtest/importintotest4/recorded_summary_harness.rs`。需要对齐 Go 行为时还应检查 `task_executor_test.go` 的对应分支，尤其是重试、取消、slot 回滚、Meta 通知和均衡测试。

兼容性风险集中在状态机和取消语义：错误分类改变可能造成重复执行或错误地失败任务；调整锁/线程顺序可能造成死锁或监控泄漏；缩短/加长退避会影响数据库压力和资源释放时延；改动摘要完成顺序可能出现任务已成功但最终进度丢失。资源修改必须保持缩容与扩容的不同操作顺序和扩容失败回滚。


### 0766a90d52 限定前置接线

可重试失败保留Running，使用实际TaskID增加ExecuteEventCounter/retry；取消分支不增加重试。

依据：来源Go ingest错误分类/NextGen重试与原自动恢复测试，以及用户明确授权的限定前置执行链。限定生产链已通过持久化 SQL/DXF/MVCC 恢复回归；真实集群部署仍需独立验证。

## 验证依据

`cargo test -p astersql-session --features nextgen --lib port_0766a90d52_ -- --test-threads=1`：2通过、0忽略，覆盖运行中真实重试/无暂停、并发DML、自动恢复与索引结果，以及启动前回滚的索引归属保护。Ready另含fmt检查、make lint及NextGen二进制构建；PG六组与无本任务基线逐失败名称比较无新增失败。
