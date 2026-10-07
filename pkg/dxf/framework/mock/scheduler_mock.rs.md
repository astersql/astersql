# `pkg/dxf/framework/mock/scheduler_mock.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dxf-framework-mock`（见同目录 `Cargo.toml`），由 `lib.rs` 以私有模块 `scheduler_mock` 装入，再通过 `pub use scheduler_mock::*` 公开导出。它不是 DXF 调度器或存储层的生产实现，而是为测试提供 GoMock 风格、可注入闭包的替身，覆盖三类协作对象：任务调度生命周期 `MockScheduler<H>`、任务结束后的清理器 `MockCleaner`，以及读写任务状态的 `MockTaskManager`。

DXF 的系统位置由 `pkg/dxf/framework/doc.go` 给出：owner 节点运行 scheduler manager、task scheduler、node manager 和 balancer，所有节点运行 task executor。此文件模拟的正是 owner 调度路径中的扩展回调、清理回调和状态存储边界，但自身不做调度、持久化或资源分配。RustCodeGraph 显示当前 Rust 文件的直接使用文件只有 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`；生产行为应到 `pkg/dxf/framework/scheduler/` 和 `pkg/dxf/framework/storage/` 查阅。

## 核心职责

1. 用 `Handler<dyn FnMut(...) + Send>` 为每个 Go 风格方法保存一个可替换闭包，并将方法参数原样交给闭包、将结果原样返回。
2. 用 `mock_method!` 统一生成公开派发方法，使未设置期望、调用计数和并发串行化等行为由 crate 根的 `Handler::invoke` 集中控制。
3. 保留 `NewMock*`、`EXPECT`、`ISGOMOCK` 以及大写方法名，降低从 `scheduler_mock.go` 移植测试时的表面差异。
4. 用 Rust 所有权表达 Go 指针/接口语义：可变任务使用 `&mut proto::Task`，可空结果使用 `Option<Box<_>>`，Go 的 `storage.TaskHandle` 由泛型 `H` 替代，一次性 session 回调由 `FnOnce` 表达。

该文件只提供“派发到测试闭包”的机制，不验证参数匹配、调用顺序或期望调用次数，也不会自动构造成功返回值。调用者必须逐字段通过 `Handler::set` 安装行为，并可用 `Handler::call_count` 手工断言次数。

## 主要符号

- `MockResult<T> = Result<T, storage::Error>`：统一三个 mock 的可失败返回类型；错误不在本文件转换。
- `SessionCallback`：`Box<dyn FnOnce(storage::sessionctx::Context) -> MockResult<()> + Send>`，表达一次性消费的 session/transaction 回调。
- `mock_method!`：为同名 `Handler` 字段生成 `&self` 方法；传给 `Handler::invoke` 的诊断文本为 `"<Method> called"`。
- `MockScheduler<H>`：包含 12 个公开处理器。生命周期入口是 `Init`、`ScheduleTask`、`Close`；查询/决策入口是 `GetTask`、`GetEligibleInstances`、`GetNextStep`、`IsRetryableErr`；任务扩展入口是 `ModifyMeta`、`OnPrepare`、`OnNextSubtasksBatch`、`OnTick`、`OnDone`。泛型 `H` 是具体 task handle，因为源码说明 Rust 的 `TaskHandle` 不是对象安全的。
- `MockCleaner`：只包含 `Clean(storage::Context, &mut proto::Task)`；可变引用保证闭包对任务的修改返回调用者。
- `MockTaskManager`：包含任务/子任务查询，节点与 slot 查询，暂停、恢复、失败、回滚、成功等状态迁移，step 切换、历史迁移、执行器重绑，以及 `WithNewSession`、`WithNewTxn` 两个资源包装入口。其集合返回值使用 `Vec<Box<_>>`，状态计数使用 `HashMap`。
- `MockSchedulerMockRecorder<H>`、`MockCleanerMockRecorder`、`MockTaskManagerMockRecorder`：都只是对应 mock 自身的类型别名；`EXPECT` 返回 `&mut self`，并没有独立 recorder 对象。
- `NewMockScheduler`、`NewMockCleaner`、`NewMockTaskManager`：接受任意引用形式的 controller 以兼容调用形状，但参数名为 `_controller` 且不保存它；构造结果的全部 `Handler` 均为空、调用计数均为零。
- 三个 `ISGOMOCK`：无操作的兼容标记。与 Go 版返回空结构体不同，Rust 返回单元值 `()`。

## 执行流程

典型调用按以下步骤进行：

1. 测试调用 `NewMockScheduler::<H, _>(&controller)`、`NewMockCleaner(&controller)` 或 `NewMockTaskManager(&controller)`；构造函数忽略 controller 并返回 `Default`。
2. 测试在目标公开字段上调用 `set(Box::new(...))` 安装闭包；例如 `scheduler.Init.set(...)`。`Handler::set` 会替换旧闭包并把计数重置为零。
3. 测试或被测代码调用同名方法。`mock_method!` 将参数移交 `Handler::invoke`。
4. `Handler::invoke` 从互斥量中暂时取出闭包；若没有闭包就立即 panic。成功取出后以 `SeqCst` 原子操作增加调用计数，再在不持有 callback mutex 的情况下执行用户闭包。
5. 闭包返回后，`invoke` 再次加锁。如果闭包执行期间没有安装新期望，就放回旧闭包以允许重复调用；若闭包调用 `set` 替换了自身，则保留新闭包。
6. 派发方法原样返回闭包结果。对于 `Clean` 和 `OnPrepare`，闭包可通过 `&mut proto::Task` 留下可见修改；对于 `WithNewSession`/`WithNewTxn`，mock 闭包决定是否以及用什么 `sessionctx::Context` 执行传入的一次性回调。

`pkg/dxf/framework/mock/migration_aster_unit_test.rs` 的 `scheduler_mock_dispatches_lifecycle_handlers` 实际覆盖 `Init` 与 `ScheduleTask` 的独立派发和计数；`cleanup_mock_preserves_go_mutable_task_pointer_semantics` 覆盖 `Clean` 对 `Task.Meta` 的原位修改。

## 数据与状态

mock 自身不保存业务任务状态；所有业务结果均由测试闭包产生。每个方法独立持有一个 `Handler`，而每个 `Handler` 只保存两项状态：`Mutex<Option<Box<F>>>` 中的可选闭包，以及 `AtomicUsize` 调用计数。安装或调用某一方法不会影响其他方法。

`MockScheduler<H>` 中的 `H` 按值传给 `OnPrepare`、`OnNextSubtasksBatch` 和 `OnDone`，因此闭包是否还能重复使用同一个 handle 取决于 `H` 的构造/克隆策略。任务参数则按用途区分：只读流程使用 `&proto::Task` 或 `&proto::TaskBase`，准备和清理使用 `&mut proto::Task`。`MockTaskManager` 的任务写入入口多接收 `Box<proto::Task>`/`Vec<Box<_>>`，表示本次调用取得所有权；查询入口以 `Option<Box<_>>` 表示“未找到”而不强制报错。

`GetTasksInStates` 使用 `Vec<Box<dyn Any + Send>>` 模拟 Go 可变参数的动态值，这牺牲了编译期状态类型约束。`WithNewSession` 和 `WithNewTxn` 的 `SessionCallback` 是 `FnOnce`，确保一次闭包值最多被执行一次，但 mock 不保证它一定会被执行。

## 依赖与调用关系

- 上游装配：`pkg/dxf/framework/mock/lib.rs` 声明本模块并重导出全部公开符号；同文件的 `Handler` 是所有派发的直接下游。
- 数据模型：`astersql_dxf_framework_proto` 提供 `Task`、`TaskBase`、`Subtask`、`SubtaskBase`、`TaskState`、`SubtaskState`、`Step`、`ManagedNode` 和 `Modification`。
- 存储边界：`astersql_dxf_framework_storage` 提供 `Context`、`Error` 及 `sessionctx::Context`；本文件不打开真实 session 或事务。
- 执行摘要：`astersql_dxf_framework_taskexecutor_execute::SubtaskSummary` 只用于 `GetAllSubtaskSummaryByStep` 的返回类型。
- 标准库：`Any` 承载动态状态参数，`HashMap` 承载状态计数与节点 slot 统计。
- crate 边界：同目录 `Cargo.toml` 还声明 planner 与 sessionctx 依赖，供整个 mock crate 使用；本文件没有直接引用 planner，session context 则通过 storage 路径出现。
- Rust 直接调用证据：RustCodeGraph 的文件边仅指向 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`。`scheduler_mock_dispatches_lifecycle_handlers` 调用 `NewMockScheduler`；`cleanup_mock_preserves_go_mutable_task_pointer_semantics` 调用 `NewMockCleaner`。`rg` 另发现 `pkg/dxf/importinto/scheduler_test.rs` 文本中构造 `NewMockTaskManager`，但索引未把该文件识别为本文件的编译调用边，因此不能据此断言它已接入当前 Rust 测试目标。
- Go 应用链证据：RustCodeGraph 对 Go 版同名方法的 callers 显示，`TaskManager` 边界被 scheduler manager、scheduler 状态机、node maintenance 和 balancer 使用，例如 `failTask -> FailTask`、`onRunning -> GetSubtaskCntGroupByStates/PauseTaskOnError`、`maintainLiveNodes -> GetAllNodes/DeleteDeadNodes`、`doBalanceSubtasks -> GetActiveSubtasks/UpdateSubtasksExecIDs`。这些边说明 mock 所模拟接口的系统职责，但不是 Rust mock 已实现 Rust trait 的证据。

## 错误处理与边界

所有 `MockResult` 错误均由安装的闭包创建，并由派发方法原样传播；文件内没有重试、包装、日志或降级逻辑。`IsRetryableErr` 也只是测试可配置的布尔判断，不实施重试。

未调用 `set` 就调用任一生成方法会 panic，消息包含方法名。callback mutex 中毒也会以 `mock handler lock poisoned` panic。调用计数只在成功取出闭包后增加，因此“未配置期望”的 panic 不计数。

若用户闭包 panic，`Handler::invoke` 无法走到恢复闭包的步骤；由于执行闭包时不持有 callback mutex，锁不会因此中毒，但该 handler 的旧闭包已被取走，后续调用会表现为未配置，除非测试重新 `set`。这是一项测试替身的失败边界，不应当被当成生产错误恢复机制。

当前文件是 Go 形状的兼容 mock，并未为 `MockScheduler<H>`、`MockCleaner` 或 `MockTaskManager` 实现 `pkg/dxf/framework/scheduler/interface.rs` 中的 Rust `Scheduler`、`Cleaner`、`TaskManager` trait。两套接口的命名、参数所有权和部分方法集合也不同，因此不能直接把这些类型作为 `Arc<dyn TaskManager>` 等生产依赖注入；扩展前必须先判断目标测试使用的是 Go 兼容面还是 Rust trait 面。

## 并发与资源生命周期

三个 mock 的闭包都要求 `Send`。`Handler` 用 mutex 保护 `FnMut`，因此同一方法的并发调用会串行取得并执行同一个闭包；不同字段拥有不同 mutex，可以彼此并行。调用计数使用 `AtomicUsize` 的 `SeqCst` 顺序，便于跨线程观察一致的累计次数。

关键不变量是用户闭包执行期间不持有 callback mutex。`handler_does_not_hold_callback_lock_during_callback` 用线程、同步通道和一秒超时验证闭包可在内部重设自身而不死锁。闭包被临时移出 mutex 也意味着同一 handler 正在执行时，另一个并发调用可能看到 `None` 并以“without an expectation” panic；本实现没有等待正在执行闭包完成的机制。

本文件不拥有真实数据库连接、事务、后台任务或清理句柄。`WithNewSession`/`WithNewTxn` 仅转交 `FnOnce`，其 session 创建、提交、回滚和释放行为完全由测试闭包定义。`Close`、`Clean`、`TransferTasks2History` 等名字也不会触发任何真实资源动作。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/mock/scheduler_mock.go`，其头部注明由 MockGen 从 Go `Scheduler`、`Cleaner`、`ExpiredFileCleaner`、`TaskManager` 接口生成。两版共享大部分公开方法名和参数语义，但机制不同：Go 版把调用交给 `gomock.Controller`，独立 recorder 负责参数匹配与调用期望；Rust 版把 recorder 别名设为 mock 自身，由每个 `Handler` 直接持有闭包，controller 参数仅保留构造签名兼容性。

重要类型映射包括：Go `error` 对应 `storage::Error`，`*Task` 的可空查询对应 `Option<Box<Task>>`，可修改指针对应 `&mut Task`，slice/map 对应 `Vec`/`HashMap`，`func(sessionctx.Context) error` 对应 `FnOnce`。Go 的 `storage.TaskHandle` 在 Rust 版由 `MockScheduler<H>` 的泛型参数表示。

当前方法集合并非完整一一对应。Go 生成文件包含 `MockExpiredFileCleaner`/`CleanExpiredFiles`，Rust 本文件没有对应类型；Go `MockTaskManager` 有 `GetCleanupTasks` 和 `GetTaskCleanupInfoByIDs`，Rust mock 没有；Rust mock 另有动态参数形式的 `GetTasksInStates`。Rust scheduler trait 的当前接口又采用 `init/schedule_once/task/extension` 等 snake_case 方法，Rust `TaskManager` trait 采用无显式 `storage::Context` 的借用参数。故维护时不能只按名称判断已完成迁移，应同时核对 Go 生成源接口与 `pkg/dxf/framework/scheduler/interface.rs`。

## 扩展指南

- 新增 Go 兼容方法时，应同时添加同名 `Handler` 字段、在相应 `Default` 初始化中置空（手写 `Default` 的 `MockScheduler` 尤其不能漏）、并用 `mock_method!` 生成派发方法；返回错误应继续使用 `MockResult`。
- 选择参数所有权时保持 Go 语义：需要让调用方观察修改时用 `&mut`，纯查询用共享引用，允许缺失用 `Option`，不必要地改为按值会破坏移植测试的观察点。
- 若目标是接入 Rust scheduler 生产 trait，应单独设计 trait adapter 或实现，而不是假定大写兼容方法会自动满足 trait；需处理 `H` 与 `dyn TaskHandle`、显式 context 差异、借用与 `Box` 差异，以及缺失/额外方法。
- 扩充 `WithNewSession`/`WithNewTxn` 时必须保留 `FnOnce` 和资源结果传播，并用独立测试证明 callback 的执行次数、错误路径以及事务语义；mock 本身不应伪造生产提交/回滚保证。
- 所有测试逻辑应继续放在独立的 `migration_aster_unit_test.rs` 或调用方的独立 `*_test.rs`，不要内嵌进本生产文件。至少补充：参数和错误原样转发、可变任务修改、未配置 panic、重复/并发调用，以及回调自替换行为。
- 同步 Go 生成文件时重点检查 `ExpiredFileCleaner`、cleanup 查询方法和 task manager trait 的差异；这些是当前已知兼容缺口。大量任务/子任务向量和 map 会被闭包取得所有权，扩展高频测试路径时应评估不必要克隆和装箱的成本。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 391 行、22 个符号，直接使用者为 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`。
- RustCodeGraph `node --file`：完整读取 `pkg/dxf/framework/mock/scheduler_mock.rs`、`pkg/dxf/framework/mock/lib.rs`、`pkg/dxf/framework/mock/migration_aster_unit_test.rs`、`pkg/dxf/framework/scheduler/interface.rs` 的相关 trait 段，以及 `pkg/dxf/framework/doc.go`。
- RustCodeGraph `query/node/callers/callees/explore`：核对 `MockScheduler`、`MockCleaner`、`MockTaskManager`、三个 `NewMock*` 入口和 Go 侧 scheduler/task-manager 调用面。图对宏展开的 Rust 方法调用覆盖有限，因此文档没有把 Go callers 冒充 Rust 编译调用边。
- Cargo 证据：`pkg/dxf/framework/mock/Cargo.toml` 的 crate 名、`lib.rs` 入口、Go package 元数据，以及 proto、storage、taskexecutor execute、planner、sessionctx 依赖。
- Go 对照：`pkg/dxf/framework/mock/scheduler_mock.go` 的 MockGen 来源、构造器、recorder 和全部公开方法签名。
- Rust 测试：`pkg/dxf/framework/mock/migration_aster_unit_test.rs` 中 `cleanup_mock_preserves_go_mutable_task_pointer_semantics`、`handler_does_not_hold_callback_lock_during_callback`、`scheduler_mock_dispatches_lifecycle_handlers`。
- 辅助搜索：`rg` 核对 Rust 引用、Cargo 依赖者、Go 方法集合和最近的 `doc.go`。本任务是纯文档分析，按计划未运行 Cargo，也未把未索引或未接线文本当作已验证运行行为。
