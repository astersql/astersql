# `pkg/dxf/example/task_executor.rs`

## 文件定位

本文件属于 `astersql-dxf-example` crate 的执行端。crate 根模块 `pkg/dxf/example/lib.rs` 以 `mod task_executor` 装载它，再用 `pub use task_executor::*` 导出 `taskExecutor`、`newTaskExecutor` 和 `stepExecutor`。包级说明 `pkg/dxf/example/doc.go` 将该 crate 定位为 DXF（Distributed eXecution Framework）的教学示例：调度器为 StepOne、StepTwo 生成子任务，而 follower 节点上的执行器消费这些子任务并打印消息。

它不是完整执行循环的实现。`taskExecutor` 是 `astersql-dxf-framework-taskexecutor::BaseTaskExecutor` 的薄适配层，负责安装示例专用 `Extension` 并转发生命周期方法；真正的轮询、状态迁移、取消和步骤资源管理在框架基类中。业务叶子 `stepExecutor::RunSubtask` 只把 `Subtask.Meta` 解码为 `subtaskMeta` 并写一条 info 日志（依据：`task_executor.rs::newTaskExecutor`、`impl TaskExecutor for taskExecutor`、`impl StepExecutor for stepExecutor`）。

`pkg/dxf/example/Cargo.toml` 声明该 crate 对应 Go 包 `pkg/dxf/example`，当前正常构建依赖为 `astersql-dxf-framework-taskexecutor` 与 `astersql-util-logutil`。scheduler、storage、handle 等依赖仍位于永不启用的 `cfg(any())` 占位区，因此生产 crate 当前只直接接入 taskexecutor 框架；更完整的两步流程由本 crate 的内存端到端测试模拟。

## 核心职责

- `newTaskExecutor` 覆盖调用方传入的 `Param.Extension`，确保框架执行循环使用示例的幂等、步骤执行器和重试策略，然后构造 `BaseTaskExecutor`。
- `taskExecutor` 实现框架 `TaskExecutor` trait，把初始化、运行、任务快照、取消和关闭全部委托给 `BaseTaskExecutor`。
- `exampleExtension` 告诉框架所有示例子任务均幂等、所有执行错误均可重试，并为当前任务创建一个新的默认 `stepExecutor`。
- `stepExecutor::RunSubtask` 从子任务字节元数据中解出 `Message`，成功时记录 `subtaskID` 与 `message`，失败时返回 `ExecutorError`。
- `taskExecutor` 还暴露与 `Extension` 同形的三个便捷方法；其中 trait 的 `IsRetryableError` 显式转发到固有方法，以避免同名递归。

## 主要符号

- `pub struct taskExecutor { pub BaseTaskExecutor: Arc<framework::BaseTaskExecutor> }`：示例任务级执行器。公开字段持有共享框架基类；类型与命名保留 Go 移植风格，crate 根模块已允许非 Rust 惯用命名。
- `pub fn newTaskExecutor(ctx: Context, task: Task, mut param: Param) -> Arc<taskExecutor>`：公开工厂入口。它先把 `param.Extension` 替换成 `Arc<exampleExtension>`，再调用 `framework::NewBaseTaskExecutor`，最终返回共享的具体执行器。
- `struct exampleExtension`：文件内私有、无字段的策略对象，实现 `Extension`。`IsIdempotent` 恒为 `true`；`GetStepExecutor` 恒返回新的 `stepExecutor::default()`；`IsRetryableError` 恒为 `true`。
- `impl taskExecutor::{IsIdempotent, GetStepExecutor, IsRetryableError}`：与 Go 接收者方法对齐的公开固有方法，返回值与 `exampleExtension` 相同。当前构造函数把框架 Extension 设置为独立的 `exampleExtension`，框架策略调用不会经过这组三个固有方法；其中错误判断仍被 `TaskExecutor` trait 实现调用。
- `impl TaskExecutor for taskExecutor`：`Init`、`Run`、`GetTaskBase`、`CancelRunningSubtask`、`Cancel`、`Close` 均直接转发；trait 的 `IsRetryableError` 调用 `taskExecutor::IsRetryableError(self, e)`。
- `pub struct stepExecutor { pub BaseStepExecutor: framework::BaseStepExecutor }`：每一步的业务执行器。嵌入的零状态基类用于表达框架默认行为，本文件只覆盖 `RunSubtask`，其余 `StepExecutor` 钩子沿用 trait 默认实现。
- `impl StepExecutor for stepExecutor::RunSubtask(&self, _: &Context, subtask: &mut Subtask) -> Result<()>`：本文件唯一的具体业务动作；上下文当前未使用，子任务只被读取，没有改写 meta。

## 执行流程

注册与构造路径以 `pkg/dxf/example/app_test.rs::test_example_application` 为直接证据：测试调用 `RegisterTaskType` 注册闭包，闭包再调用 `newTaskExecutor`；框架通过 `GetTaskExecutorFactory` 取得工厂，用任务、上下文和 `Param` 构造 `Arc<dyn TaskExecutor>`。`newTaskExecutor` 无条件覆盖测试传入的占位 Extension，因此后续策略来自本文件，而不是调用方。

运行时，调用方执行 `taskExecutor::Run`，本文件立即转发给 `BaseTaskExecutor::Run`。框架取得当前 step 的待处理子任务，调用 Extension 的 `GetStepExecutor` 懒创建并初始化 `stepExecutor`；对于 Pending 子任务，框架先标记 Running，再建立子任务上下文和监控线程，然后调用 `stepExecutor::RunSubtask`。本文件执行如下两步：

1. `subtaskMeta::Unmarshal(&subtask.Meta)` 解析调度器生成的 JSON 字节；字符串错误经 `map_err(ExecutorError)` 转为框架错误并立即返回。
2. 解析成功后，通过 `BgLogger().log(LogLevel::Info, "RunSubtask", ...)` 写入 `subtaskID` 和解码后的 `message`，然后返回 `Ok(())`。

框架收到成功结果后取消子任务上下文、等待监控线程退出，并把子任务标记 Succeed；收到错误则走取消或失败状态处理，再把错误向执行循环返回。`app_test.rs` 用两个 step、每步三个子任务验证调用 `executor.Run()` 后 Pending 子任务全部转为 Succeed；状态迁移本身不在本文件实现。

## 数据与状态

`taskExecutor` 的唯一字段是 `Arc<BaseTaskExecutor>`，因此克隆或 trait object 持有者共享同一个任务执行状态。任务、参数、任务表、当前 step、取消上下文和执行中的子任务 ID 都由框架基类拥有；本文件不另建缓存、队列或状态机。

`exampleExtension` 是零大小、无状态对象。它不检查传入的 `Subtask`、`Task` 或 `ExecutorError`，所以幂等性和重试判定不随 step、元数据或错误种类变化。每次 `GetStepExecutor` 都创建新的 `stepExecutor`；框架按 step 缓存该实例，并在 step 变化时执行 Cleanup 后替换。

`stepExecutor` 派生 `Default`，只含无状态的 `BaseStepExecutor`。`RunSubtask` 借用可变 `Subtask`，但当前实现不修改 ID、状态或 Meta；解码结果 `meta` 仅存活到日志调用结束。日志字段拥有 ID 值和消息字符串，方法没有业务返回数据或进度摘要。

## 依赖与调用关系

直接上游与接线路径为：

- `pkg/dxf/example/lib.rs` 装载并再导出本文件的公开符号，同时把 `task_executor_test.rs` 挂为独立测试模块。
- `pkg/dxf/example/app_test.rs::test_example_application` 注册 `newTaskExecutor` 工厂，取得 trait object，随后调用 `TaskExecutor::Run` 驱动两个步骤。
- 框架注册表 `RegisterTaskType`/`GetTaskExecutorFactory` 保存和取得该工厂；生产调用方需采用相同注册方式才能让 follower 节点为示例任务创建执行器。

核心下游关系为：

- `newTaskExecutor` → `framework::NewBaseTaskExecutor`，并通过 `Param.Extension` 把 `exampleExtension` 注入框架。
- `TaskExecutor` 各生命周期方法 → 同名 `BaseTaskExecutor` 方法。
- 框架 `BaseTaskExecutor::createStepExecutor` → `Extension::GetStepExecutor` → `stepExecutor::default`。
- 框架 `BaseTaskExecutor::runSubtask` → `Extension::IsIdempotent`、`StepExecutor::RunSubtask`，成功或失败后再更新任务表状态。
- `stepExecutor::RunSubtask` → `crate::subtaskMeta::Unmarshal` → `BgLogger().log`。

RustCodeGraph 已索引目标文件并报告其 23 个符号。对精确名称 `newTaskExecutor`、`stepExecutor`、`RunSubtask` 的查询找到了本文件、Go 对照和测试，但常见/跨语言重名使通用 `callers`/`callees` 没有产出可归因的精确边；上述函数级关系因此以已索引的实际调用现场为准，不把空图结果解释为“没有调用”。

## 错误处理与边界

本文件唯一直接产生的失败来自 `subtaskMeta::Unmarshal`。任何无效 JSON、非对象或 `message` 类型错误都被包装为 `ExecutorError` 返回；失败发生在日志调用之前，因此不会产生代表成功执行的 `RunSubtask` 日志。`pkg/dxf/example/task_executor_test.rs::run_subtask_propagates_json_errors_without_logging_success` 明确覆盖这一边界。

成功解码时，即使 `message` 缺失，其 Go 对齐语义是得到空字符串，本文件仍会记录日志并返回成功；该默认规则由 `proto.rs`/其测试负责，而不是此处额外验证。`RunSubtask` 不检查 task ID、step、状态、消息长度或上下文取消状态，相关前置检查和取消协调属于 `BaseTaskExecutor`。

重试策略非常宽：`exampleExtension::IsRetryableError` 和 `taskExecutor::IsRetryableError` 对所有错误都返回 `true`，包括确定性的 meta 解析错误。幂等判定也对所有子任务返回 `true`，因此框架允许恢复 Running 子任务。修改这两个恒真策略会改变故障恢复和失败收敛行为，不能只按错误表面类型局部调整。

日志后端若只按当前接口观察没有错误返回路径；`RunSubtask` 在调用日志后直接成功。`GetStepExecutor` 当前也没有失败分支，但保留 `Result` 以满足框架 trait，未来构造资源时可传播初始化错误。

## 并发与资源生命周期

本文件不直接创建线程、异步任务、锁、通道、事务、文件或网络连接。`taskExecutor` 和返回的 `StepExecutor` 必须满足框架 trait 的 `Send + Sync`；这里的共享能力来自 `Arc<BaseTaskExecutor>` 和无状态的 step/extension 类型。

并发生命周期由 `BaseTaskExecutor` 管理：它为子任务建立派生 `Context`，按需启动负载均衡、参数变更和进度监控线程，在 `RunSubtask` 完成后取消上下文并 join 线程；step 切换时取消 step 上下文并调用 `Cleanup`。本文件的 `Init`、`CancelRunningSubtask`、`Cancel` 和 `Close` 只是把请求转发到该基类，不应被描述为本地另有一套清理逻辑。

默认 `stepExecutor` 不保存跨调用状态，也不提供实时进度摘要，因此多个执行器实例之间没有本文件级共享数据。是否会并行调用同一实例由框架调度决定；当前叶子逻辑只读 subtask、进行局部分配并调用线程安全的后台日志器，没有显式临界区。

## 与 Go 版本的对应关系

Go 对照为 `pkg/dxf/example/task_executor.go`。两端都以一个任务执行器包装框架 `BaseTaskExecutor`，都声明子任务幂等、错误可重试，并返回无状态 `stepExecutor`；`RunSubtask` 都解码 `subtaskMeta`，成功后以 info 级别记录 subtask ID 和 message，解码失败则原样结束业务处理。

构造接线有语言实现差异。Go 先创建 `BaseTaskExecutor`，再令 `e.BaseTaskExecutor.Extension = e`，所以同一个 `taskExecutor` 同时承担框架 TaskExecutor 与 Extension。Rust 为避免自引用对象，在调用 `NewBaseTaskExecutor` 前把 `Param.Extension` 设置为独立的 `Arc<exampleExtension>`；`taskExecutor` 自身仍保留对应的三个固有方法，并通过 trait 的错误判断方法使用其中之一。行为返回值对齐，但对象身份和策略调用路径不同。

Go 依靠嵌入自动提升 `BaseTaskExecutor` 和 `BaseStepExecutor` 方法；Rust 显式实现 `TaskExecutor` 并逐项转发，`StepExecutor` 的未覆盖钩子则使用 trait 默认实现。Go 使用 `encoding/json.Unmarshal` 和 zap 字段；Rust 使用 `subtaskMeta::Unmarshal` 与 `astersql-util-logutil` 的 `LogField`，其 JSON 兼容细节由独立 `proto.rs` 实现。

Rust `app_test.rs` 已用内存 `FakeTaskTable` 跑通本 crate 的真实 scheduler/executor 两步流程，但测试注释和 Cargo 的 `cfg(any())` 依赖表明 scheduler/handle/storage/testkit 的完整 Go 式集成尚未接通。因此可以确认本文件与已移植框架基类协作，不应据此声称整个 Go DXF 示例环境已经等价移植。

## 扩展指南

若不同 step 需要不同业务执行器，最直接的接入点是 `exampleExtension::GetStepExecutor`：读取 `Task.TaskBase.Step` 并返回相应实现，同时保持 `taskExecutor::GetStepExecutor` 的公开兼容行为一致。新增执行器应放在生产源文件中，测试继续放在独立的 `task_executor_test.rs` 或相邻独立测试文件，不要把测试模块内嵌进本文件。

若 `RunSubtask` 需要写回结果，应在 `stepExecutor::RunSubtask` 中明确修改 `subtask.Meta`；框架成功路径会把更新后的 meta 交给 `FinishSubtask`。同时扩展 `subtaskMeta` 协议、scheduler 生成逻辑和 `task_executor_test.rs` 的往返/错误用例，并在 `app_test.rs` 验证两步状态与结果。协议修改须继续核对 Go `encoding/json` 行为。

若引入外部资源或每步状态，应分别利用 `StepExecutor::Init`、`Cleanup`、`TaskMetaModified`、`ResourceModified` 和进度摘要钩子，不要绕开 `BaseTaskExecutor` 的 step 切换与取消上下文。资源构造失败应通过 `Result` 返回；Cleanup 需可重复且能应对取消后的部分初始化状态。

修改 `IsIdempotent` 或 `IsRetryableError` 时，需要新增 Running 子任务恢复、确定性解析错误和瞬态错误的独立测试，验证框架最终状态，而不只验证布尔返回值。兼容风险集中在重试次数和恢复语义；性能风险主要来自为每个 step 重建 executor、每个子任务解析 JSON及同步记录日志。若消息量上升，应先用实际负载确认日志成本，再决定采样或降级策略。

## 验证依据

- RustCodeGraph `status`：索引包含 11467 个文件、7032 个 Rust 文件；`files --filter pkg/dxf/example` 确认目标、Go 对照、模块入口和独立测试均已索引。
- RustCodeGraph `node --file` 已阅读：`pkg/dxf/example/task_executor.rs`、`lib.rs`、`task_executor.go`、`task_executor_test.rs`、`app_test.rs`、`proto.rs`、`pkg/dxf/framework/taskexecutor/interface.rs` 与 `task_executor.rs` 的直接相关区段。
- RustCodeGraph 已查询 `newTaskExecutor`、`stepExecutor`、`RunSubtask`，并执行 callers/callees/explore；由于跨语言重名导致精确图边不足，调用关系又以 `app_test.rs` 的工厂注册现场和框架 `createStepExecutor`/`runSubtask` 源码核验。
- crate 与包契约证据：`pkg/dxf/example/Cargo.toml`、`pkg/dxf/example/doc.go`；Go 语义对照：`pkg/dxf/example/task_executor.go`。
- 独立测试证据：`task_executor_test.rs` 验证合法 meta 的 ID/message 日志以及非法 JSON 不写成功日志；`app_test.rs::test_example_application` 验证工厂覆盖占位 Extension、两步各三个子任务并由 `executor.Run()` 推进为 Succeed。
- 本任务只新增说明文档，按计划未运行 Cargo。交付时使用任务指定的结构命令确认文件存在并恰有 11 个固定二级标题，并人工复核文档区分了本文件职责、框架职责和尚未接通的完整集成边界。
