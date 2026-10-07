# `pkg/dxf/framework/mock/task_executor_mock.rs`

## 文件定位

本文件说明的源码是 [`task_executor_mock.rs`](./task_executor_mock.rs)。它属于 `astersql-dxf-framework-mock` crate，是 DXF（Distributed eXecution Framework）执行侧的手写 Rust mock 集合。crate 入口 `pkg/dxf/framework/mock/lib.rs` 将本模块私有装载后通过 `pub use task_executor_mock::*` 公开其类型与构造函数；`pkg/dxf/framework/mock/Cargo.toml` 则声明它依赖 `proto`、`storage` 和 `taskexecutor/execute` 三个直接参与本文件签名的本地 crate。

它对照同目录由 MockGen 生成的 `task_executor_mock.go`，覆盖三组边界：持久化任务表 `MockTaskTable`、单任务执行器 `MockTaskExecutor`、步骤执行扩展 `MockExtension`。需要特别注意：这些 Rust 类型目前只是 GoMock 风格的回调替身，源码中没有实现 `pkg/dxf/framework/taskexecutor/interface.rs` 的同名 `TaskTable`、`TaskExecutor`、`Extension` trait，且若干参数/返回类型也不同。因此它们不能仅凭同名方法就作为这些 trait 的对象直接注入生产执行链。

## 核心职责

- 用 `Handler<dyn FnMut(...) + Send>` 为每个可观察方法保存一段测试注入的闭包；方法被调用时，把参数原样交给闭包并返回闭包结果。实际的锁、期望安装和调用计数逻辑位于 `pkg/dxf/framework/mock/lib.rs` 的 `Handler::{set, invoke, call_count}`。
- 用 `mock_method!` 统一生成公开派发方法，避免为 28 个接口形状重复编写 `Handler::invoke` 包装代码。派发失败信息由宏传入 `"<方法名> called"`，最终由 `Handler::invoke` 补成未配置期望的 panic 文本。
- 提供 `EXPECT`、`ISGOMOCK`、`*MockRecorder` 别名以及 `NewMock*` 构造函数，保留 GoMock 使用方式的命名外观，方便移植测试逐步替换调用点。
- 用本地 `TaskExecInfo` 暂存 Go `storage.TaskExecInfo` 的形状。该类型包含 `TaskBase` 和 `SubtaskConcurrency`，但它既不是 storage crate 的同名类型，也不是 taskexecutor crate 中只含 `TaskBase` 的 `TaskExecInfo`。

## 主要符号

- `MockResult<T> = Result<T, storage::Error>`：本文件所有可失败回调的共同结果类型。
- `SessionCallback`：拥有所有权且只允许执行一次的 `FnOnce(storage::sessionctx::Context) -> MockResult<()> + Send` 回调，用于 `WithNewSession`。
- `mock_method!`：为字段同名方法生成 `&self` 接口，并调用相应 `Handler::invoke`。宏不会做参数转换、默认返回或错误包装。
- `TaskExecInfo`：本文件私有迁移边界所需的公开数据结构；字段沿用 Go 风格名称 `TaskBase`、`SubtaskConcurrency`。
- `MockTaskTable`：含 18 个 `Handler` 字段，覆盖任务/子任务读取，开始、完成、失败、取消、暂停、状态更新、检查点，以及执行器 meta 初始化和恢复。
- `MockTaskExecutor`：含 `Cancel`、`CancelRunningSubtask`、`Close`、`GetTaskBase`、`Init`、`IsRetryableError`、`Run` 七个生命周期处理器。
- `MockExtension`：含 `GetStepExecutor`、`IsIdempotent`、`IsRetryableError` 三个扩展处理器；`GetStepExecutor` 返回 `Box<dyn execute::StepExecutor>`。
- `MockTaskTableMockRecorder`、`MockTaskExecutorMockRecorder`、`MockExtensionMockRecorder`：都只是对应 mock 本体的类型别名；`EXPECT(&mut self)` 直接返回 `self`，并非 GoMock 中独立的 recorder 对象。
- `NewMockTaskTable`、`NewMockTaskExecutor`、`NewMockExtension`：忽略传入的泛型 controller，返回所有 `Handler` 均未安装闭包的默认实例。

## 执行流程

典型调用流程如下：

1. 测试以任意 controller 引用调用 `NewMock*`；controller 仅用于保留调用形状，不参与运行时行为。
2. 测试通过公开字段的 `Handler::set(Box::new(...))` 安装闭包。`EXPECT()` 也可取得同一个 mock 的可变引用，但本文件没有 GoMock 的链式匹配器或次数约束 API。
3. 调用同名公开方法，例如 `MockTaskExecutor::Cancel()`。`mock_method!` 展开后进入 `self.Cancel.invoke(...)`。
4. `Handler::invoke` 暂时从互斥量中取出闭包，递增原子调用计数，在不持锁的情况下执行用户闭包，再在没有新期望取代它时把旧闭包装回去。因此同一期望默认可重复调用，且回调可在执行期间重设自身期望。
5. 回调的返回值不经解释直接返回；例如存储方法的 `storage::Error`、布尔策略结果、可选任务及步骤执行器都由测试闭包完全决定。

`MockTaskTable` 的方法可按生命周期理解：`InitMeta`/`RecoverMeta` 建立或恢复执行节点记录；查询方法选择任务和子任务；`StartSubtask` 进入运行态；`FinishSubtask`、`FailSubtask`、`CancelSubtask` 或 `UpdateSubtaskStateAndError` 收束状态；检查点方法保存或读取进度；`RunningSubtasksBack2Pending` 支持恢复语义。这里只模拟调用边界，不实现任何状态机或持久化。

## 数据与状态

mock 本体没有任务数据库、执行队列或状态机。每个字段的状态完全封装在独立 `Handler` 中：一个可替换的闭包和一个调用次数计数器。构造后的闭包为 `None`、计数为零；`set` 会安装闭包并将对应字段的计数重置为零；成功进入 `invoke` 后计数以 `SeqCst` 原子顺序增加。

参数通常按值传递：`String`、`Vec<u8>`、状态向量和 `Box<proto::...>` 会转移所有权。`MockExtension::{GetStepExecutor, IsIdempotent}` 使用共享引用读取任务或子任务。`UpdateSubtaskCheckpoint` 和 `GetTasksInStates` 以 `Box<dyn Any + Send>` 保留 Go `any` 的开放形状，调用方和闭包必须自行约定真实动态类型。

`TaskExecInfo` 中的 `SubtaskConcurrency` 仅作为数据字段保存，本文件没有计算或校验它；同理，各 proto 状态、step、任务 ID 和执行器 ID 都不会在 mock 层验证。

## 依赖与调用关系

- 上游导出：`pkg/dxf/framework/mock/lib.rs` 重导出本文件全部公开符号，并提供核心 `Handler`。RustCodeGraph 将目标文件识别为 21 个符号，并显示该模块由 crate 入口装配。
- 已确认的直接 Rust 使用者：`pkg/dxf/framework/mock/migration_aster_unit_test.rs::task_executor_mock_dispatches_cancel_handler` 调用 `NewMockTaskExecutor`、为 `Cancel` 安装闭包、执行 `Cancel()` 并检查调用次数。仓库级 `rg` 未发现其他 Rust 文件直接使用本文件的 `MockTaskTable` 或 `MockExtension`。
- 下游类型依赖：`astersql-dxf-framework-proto` 提供 `Task`、`TaskBase`、`Subtask`、`SubtaskBase`、`Step` 与状态类型；`astersql-dxf-framework-storage` 提供上下文、session context 和错误；`astersql-dxf-framework-taskexecutor-execute` 提供初始化上下文与 `StepExecutor` trait。
- 语义参照：`pkg/dxf/framework/taskexecutor/interface.rs` 定义真正执行链使用的三个 trait；生产适配示例 `pkg/session/runtime/modify_column_dist_backfill.rs::TaskTable` 实现的是该 taskexecutor trait，而不是本文件的 mock 类型。
- Go 上游：`pkg/dxf/framework/mock/task_executor_mock.go` 声明由 `taskexecutor` 的 `TaskTable, TaskExecutor, Extension` 接口生成，并被多个 Go 测试及执行路径使用。Rust 文件沿用其方法集合与命名，但没有 GoMock controller 调用边。

## 错误处理与边界

可失败方法只传播闭包给出的 `storage::Error`，本文件不转换、不记录也不重试错误。策略方法 `IsRetryableError` 只是把错误交给测试闭包决定布尔值；它本身不包含可重试规则。

调用未设置期望的字段会在 `Handler::invoke` 中 panic，而不是返回 `storage::Error`。若保存闭包的互斥量中毒，`set` 或 `invoke` 同样 panic。`NewMock*` 不会安装默认 no-op，所以构造后直接调用任一派发方法都属于测试配置错误。

与生产 trait 的差异是安全扩展时最重要的边界：本文件的 context、错误、所有权和返回容器使用 storage/proto/execute crate 类型；taskexecutor trait 则使用自身的 `Context`、`ExecutorError`、值类型或 `Arc<dyn StepExecutor>`。此外，Rust `TaskTable` trait 已包含 `UpdateSubtaskSummaryJSON`、`AcquireTaskRuntime` 等接口，而该 mock 没有对应处理器。不能把“方法名称相近”当作 trait 已实现的证据。

## 并发与资源生命周期

所有闭包类型都要求 `Send`，`Handler` 使用 `Mutex<Option<Box<F>>>` 和 `AtomicUsize` 管理共享状态。`invoke` 在执行用户闭包前释放互斥锁，避免闭包内调用同一 `Handler::set` 时死锁；`pkg/dxf/framework/mock/migration_aster_unit_test.rs::handler_does_not_hold_callback_lock_during_callback` 用线程和超时验证了这一契约。执行结束后仅当槽位仍为空才恢复原闭包，从而保留回调期间安装的新期望。

这不等于同一字段支持并行重入：一次 `invoke` 执行期间闭包已从槽位取走，另一线程同时调用该方法会看到没有期望并 panic。不同字段各有独立锁，可以独立派发。调用次数使用 `SeqCst`，但仅表示成功取出闭包后的派发次数，不提供跨多个 Handler 的全局调用顺序。

`MockTaskExecutor::{Cancel, Close, Run}`、`MockTaskTable` 的 session 方法和 `MockExtension` 的步骤执行器工厂都不自行创建线程、关闭资源或管理任务；生命周期效果完全由注入闭包承担。`WithNewSession` 接收 `FnOnce`，只保证类型层面回调至多执行一次，是否执行仍由外层 Handler 闭包决定。

## 与 Go 版本的对应关系

Go 文件是 MockGen 产物，controller 负责记录期望、参数匹配、返回值和校验；Rust 版本改用一个 `Handler` 字段对应一个方法。Rust 的 `EXPECT()` 返回 mock 自身，recorder 也是本体别名，controller 参数被忽略，`ISGOMOCK()` 为空操作，因此不具备 GoMock 的 matcher、调用顺序、期望次数和测试结束自动校验。

三组方法的总体意图与 Go 一致，但存在可见签名差异：Go 的状态过滤方法使用可变参数，Rust mock 接收 `Vec`；Go 指针切片在 Rust 中多表示为 `Vec<Box<_>>`；Go `error` 被具体化为 `storage::Error`；Go `TaskExecutor::GetTaskBase` 返回指针，Rust mock 返回 `Option<Box<_>>`；Go `Extension::GetStepExecutor` 返回接口，Rust mock返回 `Box<dyn execute::StepExecutor>`。Rust 还暴露 `GetTasksInStates` 处理器，而当前同目录 Go 生成文件没有该方法，说明两边生成/迁移时间点并非完全同步。

Go 的 `TaskExecInfo` 来自 storage 包；Rust mock 在文件内另建了含 `SubtaskConcurrency` 的版本。仓库中 storage crate 和 taskexecutor crate 已分别存在不同的 `TaskExecInfo`，后续对齐时必须先确定权威接口，不能直接以名称互换。

## 扩展指南

新增或修改一个 mock 方法时，应先以 `pkg/dxf/framework/taskexecutor/interface.rs` 的当前 trait 和同目录 Go 接口/生成文件为依据，明确这是补齐生产 trait 还是仅保留迁移兼容外观。然后在相应 struct 增加 `Handler` 字段，并用 `mock_method!` 增加同签名派发；涉及新 crate 类型时同步检查 `pkg/dxf/framework/mock/Cargo.toml`。

若目标是让 mock 可注入 Rust 生产 trait，应单独实现 trait，并显式处理 context、错误、借用、`Box`/`Arc` 以及可选值转换；不要仅改现有方法签名来“碰巧匹配”，否则会破坏迁移测试的 Go 形状。尤其要处理当前缺失的 trait 方法和 `TaskExecInfo` 分叉。

测试应放在独立的 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`，不要内嵌到生产源文件。至少覆盖参数与返回值透传、错误透传、未设期望的 panic、重复调用、回调内重设期望以及新增生命周期分支；若增加 trait 实现，还应加入编译期 trait 约束和通过 trait object 调用的回归测试。并发测试要区分已验证的“回调期间不持锁”和尚不支持的“同一 Handler 并行调用”。

兼容风险主要来自 Go/Rust 接口漂移及动态 `Any` 类型约定；正确性风险来自错误的所有权转换或把未实现 trait 的替身用于生产链；性能通常不是该测试辅助模块的目标，但长时间回调会占用闭包本身，导致同字段并发调用 panic。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录及目标文件均已索引。
- RustCodeGraph `files --filter pkg/dxf/framework/mock`：确认目标、Go 对照、crate 入口与独立迁移测试位于同一模块边界。
- RustCodeGraph `node --file pkg/dxf/framework/mock/task_executor_mock.rs --offset 1 --limit 260`：核对完整 255 行源码、21 个符号及目标文件的索引使用关系。
- RustCodeGraph `query MockTaskExecutor --kind struct`、`query NewMockTaskExecutor --kind function`：区分 Rust/Go 同名符号；查询结果确认 Rust 构造函数及测试调用点。
- 已读源码/配置：`pkg/dxf/framework/mock/lib.rs`、`pkg/dxf/framework/mock/Cargo.toml`、`pkg/dxf/framework/taskexecutor/interface.rs`、`pkg/session/runtime/modify_column_dist_backfill.rs`。
- 已读 Go 对照：`pkg/dxf/framework/mock/task_executor_mock.go`，包括生成来源、三组 mock 及其方法签名。
- 已读独立 Rust 测试：`pkg/dxf/framework/mock/migration_aster_unit_test.rs`；其中 `task_executor_mock_dispatches_cancel_handler` 是本文件的直接行为回归，`handler_does_not_hold_callback_lock_during_callback` 验证共享 `Handler` 的锁生命周期。
- 仓库 `rg` 检索三组 mock 的类型与构造函数：确认 Rust 直接使用范围，并确认生产执行 trait 与 `TaskExecInfo` 的其他定义位置。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级章节。
