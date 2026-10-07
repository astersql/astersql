# `pkg/dxf/framework/taskexecutor/register.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-taskexecutor` crate，是 DXF 节点侧任务执行器的“任务类型到执行器工厂”注册表。crate 入口 `pkg/dxf/framework/taskexecutor/lib.rs` 通过 `mod register` 装入本模块，并用 `pub use register::*` 对外再导出公开 API。仓库的 `pkg/dxf/framework/doc.go` 将 task executor manager、slot manager 和 task executor 定义为所有 DXF 节点都会运行的组件；本注册表位于 manager 创建具体 task executor 的接线点。

直接消费入口是 `pkg/dxf/framework/taskexecutor/manager.rs` 的 `Manager::startTaskExecutor`：它取得完整 `Task`、分配 slot 和 task runtime 后，以 `Task.TaskBase.Type` 调用 `GetTaskExecutorFactory`；查找成功才构造并初始化执行器，未注册则调用 `failSubtask` 并释放 slot。因而本文件只负责进程内类型分派，不负责任务持久化、资源分配、执行器初始化或线程启动。

## 核心职责

- `FactoryFn` 固定工厂调用约定：接收可取消的 `Context`、完整 `Task` 和依赖注入参数 `Param`，返回共享的 `Arc<dyn TaskExecutor>`。
- `factories` 懒初始化一个进程级注册表，并以 `RwLock<HashMap<...>>` 保护并发访问。
- `RegisterTaskType` 新增或覆盖任务类型对应的工厂。
- `GetTaskExecutorFactory` 查找并克隆工厂句柄；缺项以 `None` 表达。
- `ClearTaskExecutors` 清空全部映射，服务于测试隔离。
- 仅在 `cfg(test)` 下提供 `RegistryLockForTest`，使所有会直接或间接触碰全局注册表的 Rust 单元测试串行执行。

该文件不验证任务类型字符串、不拒绝重复注册，也不拥有已创建执行器的生命周期。`Manager::startTaskExecutor` 承担“查不到工厂即任务失败”的策略。

## 主要符号

- `pub type FactoryFn = Arc<dyn Fn(Context, Task, Param) -> Arc<dyn TaskExecutor> + Send + Sync>`：可跨线程共享的动态工厂。外层 `Arc` 让查找操作可以低成本克隆句柄，`Send + Sync` 允许 manager/worker 并发环境安全持有它；返回值的 `Arc<dyn TaskExecutor>` 同样允许 manager、工作线程和清理逻辑共享执行器。
- `fn factories() -> &'static RwLock<HashMap<TaskType, FactoryFn>>`：私有访问器。函数内 `OnceLock` 保证映射只初始化一次，`TaskType` 在 `interface.rs` 中是 `String` 的别名。
- `pub fn RegisterTaskType(task_type: TaskType, factory: FactoryFn)`：取得写锁后执行 `HashMap::insert`。相同 key 会用新工厂替换旧工厂，函数不返回旧值。
- `pub fn GetTaskExecutorFactory(task_type: &str) -> Option<FactoryFn>`：取得读锁，用借用的字符串查找 `String` key，并对命中的 `Arc` 执行 `cloned`。调用者得到独立强引用，不会在持锁期间调用工厂。
- `pub fn ClearTaskExecutors()`：取得写锁并调用 `HashMap::clear`；它清除条目但不重建 `OnceLock` 或锁对象。
- `pub fn RegistryLockForTest() -> MutexGuard<'static, ()>`：仅测试构建可见的全局互斥守卫。锁被 poison 时通过 `into_inner` 继续取得守卫，避免一次测试 panic 使后续测试无法做隔离清理。

本文件没有常量、结构体、枚举、trait、`impl` 或 feature 分支；唯一条件编译项是 `RegistryLockForTest` 的 `#[cfg(test)]`。

## 执行流程

1. 业务或测试接线方构造满足 `FactoryFn` 的 `Arc` 闭包，并用拥有所有权的 `TaskType` 调用 `RegisterTaskType`。
2. 首次访问时，`factories` 经 `OnceLock::get_or_init` 建立空 `HashMap` 和 `RwLock`；后续调用复用同一静态实例。
3. 注册取得写锁并插入映射。若类型已存在，`HashMap::insert` 原子地替换该锁临界区内的旧值。
4. `Manager::startTaskExecutor` 从任务表取得 `Task` 并完成 slot/runtime 前置步骤后，传入 `&task.TaskBase.Type` 查找工厂。
5. 查找取得读锁，克隆命中的 `Arc<dyn Fn...>` 后释放锁。缺项返回 `None`；manager 将其转换为 `task type ... not found` 的 `ExecutorError`，失败子任务并释放 slot。
6. 命中时，manager 组装 `Param`，在注册表锁之外调用工厂，随后调用执行器的 `Init`，登记执行器并在线程中运行 `Run`/`Close`。
7. 测试在操作注册表前持有 `RegistryLockForTest` 守卫，并在用例首尾调用 `ClearTaskExecutors`，防止并行测试相互观察到残留注册项。

## 数据与状态

注册表的 key 是区分大小写的任意 `String`（`TaskType`）；本模块没有格式约束或命名空间。value 是类型擦除的共享闭包，闭包可捕获自己的状态，但本模块既不检查也不管理该状态。

`OnceLock` 使注册表的容器地址和锁在进程生命周期内稳定。`ClearTaskExecutors` 只删除 map 中的强引用：若调用者此前通过 `GetTaskExecutorFactory` 克隆过某个 `FactoryFn`，该工厂仍然存活且仍可调用。重复注册同一 key 会替换 map 持有的强引用，也不会使已经取出的旧句柄失效。

`FactoryFn` 按值接收 `Context`、`Task`、`Param`。相关定义分别位于 `interface.rs` 和 `task_executor.rs`：`Context` 内部以 `Arc` 共享取消状态，`Task` 包含 `TaskBase` 与不透明 `Meta`，`Param` 聚合任务表、slot manager、节点资源、执行节点 ID、扩展点与可选 task runtime。

## 依赖与调用关系

本文件的直接标准库依赖只有 `HashMap`、`Arc`、`OnceLock`、`RwLock`，以及测试函数签名中的 `MutexGuard`/`Mutex`。`Context`、`Task`、`TaskExecutor`、`TaskType` 来自 crate 的 `interface` 再导出，`Param` 来自 `task_executor` 再导出。

`pkg/dxf/framework/taskexecutor/Cargo.toml` 指定库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/dxf/framework/taskexecutor"` 记录 Go 对照包。当前无普通 Cargo feature；唯一始终启用的 crate 依赖是 `astersql-lightning-log`，大量尚未启用的迁移依赖被放在 `target.'cfg(any())'.dependencies` 下。本文件自身不调用外部 crate API。

RustCodeGraph 的关键边为：

- `RegisterTaskType -> factories`；索引中的直接 Rust 调用者均为测试侧接线，包括 `register_test.rs`、`manager_test.rs`、`task_executor_testkit_test.rs` 和 `example/app_test.rs`。
- `GetTaskExecutorFactory -> factories`；生产调用者是 `manager.rs::startTaskExecutor`，另有 `register_test.rs`、`task_executor_testkit_test.rs` 覆盖测试调用。
- `ClearTaskExecutors -> factories`；索引调用者为需要隔离全局状态的测试。
- `RegistryLockForTest` 由 `register_test.rs`、`manager_test.rs` 和 `task_executor_testkit_test.rs` 的相关测试使用。

基于本次查询，Rust 生产路径“消费工厂”已经接入 manager，但没有检索到生产代码直接调用 `RegisterTaskType` 的边；具体任务类型在完整 Rust 应用启动阶段的生产注册点未由本文件及其直接证据验证，不能据此宣称所有任务类型已经接线。

## 错误处理与边界

注册、查找和清理没有可恢复的 `Result`。注册表 `RwLock` 一旦因持锁线程 panic 而 poison，三个生产 API 都通过 `expect("factory lock poisoned")` 再次 panic；这是显式的进程内一致性失败策略。测试串行锁采用不同策略：poison 时恢复内部守卫，以便后续测试仍能执行清理。

缺少任务类型不是本文件中的错误，而是 `GetTaskExecutorFactory` 的 `None`。生产调用者 `Manager::startTaskExecutor` 将其升级为任务失败。重复注册不是错误，会覆盖旧工厂；空字符串也未被拒绝。工厂闭包本身不返回 `Result`，因此“构造失败”没有此层协议；可恢复的启动失败发生在随后 `TaskExecutor::Init` 返回的 `Result`。

本模块不捕获工厂调用期间的 panic。它也不保证注册与任务启动的业务时序：锁只保证内存安全和单次 map 操作的一致性，不能防止某线程在另一线程清理之后才查找，或取到清理/覆盖前已经克隆出的旧工厂。

## 并发与资源生命周期

`RwLock` 允许多个查找并行，注册和清理互斥于所有读写。`GetTaskExecutorFactory` 在读锁内只完成查找与 `Arc` 克隆，随后释放锁，所以用户工厂、执行器 `Init` 和 `Run` 都不会扩大注册表临界区，也不会因回调重入注册 API 而直接自锁。

全局 map 在首次访问时创建并存活到进程退出。条目由 `Arc` 计数管理；覆盖或清理会释放 map 的强引用，但外部克隆决定工厂真实销毁时点。注册表不跟踪工厂创建出的 executor，后者由 manager 登记并交给 worker 线程；worker 结束时执行 `Close`、注销和 slot 释放，task runtime 由 `RuntimeLease` 生命周期持有。

测试锁并不参与生产注册表加锁，只是更外层的用例级协议。任何触碰该注册表或运行真实 manager/executor 的新单元测试都应先取得同一个 `RegistryLockForTest` 守卫，不能在不同测试文件各建私有锁，否则 `cargo test` 的进程内并行仍会产生相互清理或覆盖。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dxf/framework/taskexecutor/register.go`。两版保留相同的三项核心行为：类型到工厂的进程级映射、重复 key 覆盖、测试用全量清理；工厂参数也对应 `context`、task 和 `Param`，返回 `TaskExecutor`。

主要差异如下：

- Go 使用包级 `map[proto.TaskType]FactoryFn`，没有内部锁；Rust 使用 `OnceLock<RwLock<HashMap<...>>>` 支持跨线程安全访问。
- Go 工厂接收 `context.Context` 和 `*proto.Task`，返回接口值；Rust 接收拥有所有权但内部可共享的 `Context`、`Task`、`Param`，返回 `Arc<dyn TaskExecutor>`，工厂本身也包装在 `Arc` 且要求 `Send + Sync`。
- Go 查找直接返回 map 值，缺项表现为 `nil`；Rust 返回 `Option<FactoryFn>`，迫使 manager 显式处理 `None`。
- Go 的 `ClearTaskExecutors` 用新 map 替换旧 map；Rust 在持写锁时清空原 map，并保持静态容器不变。
- Rust 增加了 `RegistryLockForTest`，补偿 Rust 测试默认在同一进程并发运行的隔离需求；Go 对照测试依赖包测试的顺序化状态使用方式，没有对应 API。

`register_test.go::TestRegisterTaskType` 验证两种类型注册和重复注册后 map 长度不增加。Rust 的 `register_test.rs::test_register_task_type` 保留这些意图，并额外从公开 API 验证存在/缺失和清理结果；`test_get_task_executor_factory_invokes_registered_factory` 进一步证明取出的工厂可创建、初始化并运行执行器。

## 扩展指南

- 新增任务类型时，应在任务所属模块构造 `FactoryFn` 并在应用启动、manager 可能观察到任务之前调用 `RegisterTaskType`。同时确认 `Task.TaskBase.Type` 与注册 key 完全一致；本模块不会做规范化。
- 若需要阻止重复注册，应修改 `RegisterTaskType` 的返回协议并同步所有调用者，而不能仅靠预查；“检查后插入”若分开加锁会产生竞态。相应测试应放在独立的 `register_test.rs`，不要嵌入生产文件。
- 若工厂创建可能失败，需要把 `FactoryFn` 返回类型改为 `Result<Arc<dyn TaskExecutor>>`，并同步 `Manager::startTaskExecutor` 的失败、runtime 与 slot 清理路径；这属于公开 API 兼容性变更。
- 若增加按类型注销或快照枚举，应在同一 `RwLock` 临界区完成，并明确已克隆 `Arc` 仍可继续使用的语义。不要尝试重置 `OnceLock`。
- 新增会使用 manager 或注册表的 crate 内测试必须取得 `RegistryLockForTest`，并在开始/结束清理注册表。相关测试继续放在 `register_test.rs`；涉及 manager 的失败与资源释放应同步 `manager_test.rs`。
- 性能风险主要是高频动态注册/清理导致写锁阻塞查找。正常模型应是启动期注册、运行期读多写少；如改变这一模型，需要基准或并发测试证明新的锁策略不会放大任务启动延迟。
- 兼容性风险集中在 `FactoryFn` 签名、缺项行为和重复注册语义；修改时应同时核对 Go 文件，避免 Rust 版本无意偏离 Go 的任务分派行为。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/dxf/framework/taskexecutor/register.rs` 的 `FactoryFn`、`factories`、`RegisterTaskType`、`GetTaskExecutorFactory`、`ClearTaskExecutors`、`RegistryLockForTest`。
- crate 与模块边界：`pkg/dxf/framework/taskexecutor/Cargo.toml`、`pkg/dxf/framework/taskexecutor/lib.rs`。
- DXF 包契约：`pkg/dxf/framework/doc.go`。
- 生产消费链：`pkg/dxf/framework/taskexecutor/manager.rs::Manager::startTaskExecutor`。
- 关联类型：`pkg/dxf/framework/taskexecutor/interface.rs` 的 `Context`、`TaskType`、`Task`，以及 `pkg/dxf/framework/taskexecutor/task_executor.rs` 的 `Param`。
- Go 对照：`pkg/dxf/framework/taskexecutor/register.go`、`pkg/dxf/framework/taskexecutor/register_test.go`。
- 独立 Rust 测试：`pkg/dxf/framework/taskexecutor/register_test.rs`；跨文件测试锁调用还由 `manager_test.rs`、`task_executor_testkit_test.rs` 的 RustCodeGraph 调用边确认。
- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；精确 `node` 查询确认 `RegisterTaskType -> factories`、`GetTaskExecutorFactory -> factories`、`ClearTaskExecutors -> factories` 以及上述调用者关系。

任务规定的结构检查用于确认文档存在且恰含十一个固定二级标题。由于这是纯文档分析，没有修改 Rust/Go/Cargo 行为，也没有运行 Cargo 或代码测试。人工复核重点是：所有“已支持”陈述均落到上述源码、调用边或测试证据；Rust 生产注册入口未被直接证据确认的部分已明确标为未验证。
