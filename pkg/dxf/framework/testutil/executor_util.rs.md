# `pkg/dxf/framework/testutil/executor_util.rs`

## 文件定位

该文件属于 `astersql-dxf-framework-testutil` crate，是 DXF（Distributed eXecution Framework，分布式任务执行框架）的执行器测试适配层。crate 入口 `pkg/dxf/framework/testutil/lib.rs` 通过 `pub mod executor_util` 声明模块，并用 `pub use executor_util::*` 再导出其公开 API；`pkg/dxf/framework/testutil/Cargo.toml` 则把 crate 映射回 Go 包 `pkg/dxf/framework/testutil`。

它不实现生产环境的 task executor manager，也不调度任务。它把 `disttest_util.rs` 中可注入的 `TaskExecutorExtension` 和 `StepExecutor` 组装成名为 `Example` 的测试任务执行器，并提供一次完整执行单个子任务生命周期的辅助函数。DXF 的生产语义是“任务按顺序经过多个 step，每个 step 包含可并行的 subtask”；该定位由 `pkg/dxf/framework/doc.go` 的 Task abstraction 和 task executor 描述确认。

## 核心职责

1. 用 `TaskExecutorRegistry` 隔离具体注册表实现，使测试能够观察注册和清理，而不依赖全局生产注册表。
2. `InitTaskExecutor` 为 `TASK_TYPE_EXAMPLE`（字符串值 `"Example"`）组装并注册通用 `TaskExecutorExtension`；扩展根据任务当前的 `task.base.step` 创建 `StepExecutor`，并向其中注入调用者提供的 `run_subtask` 回调。
3. `ExecutorRegistrationGuard` 用 RAII（资源获取即初始化）把注册项生命周期绑定到 Rust 作用域：守卫析构时调用 `clear_executors`，避免测试之间共享注册状态。
4. `run_registered_subtask` 显式驱动 `get_step_executor → init → run_subtask → cleanup`，供独立测试覆盖扩展和执行器的连接方式。

该文件是测试工具而非完整的 Go API 等价实现：Rust 版本只处理执行器扩展注册和单次子任务执行，没有在这里构造生产 `BaseTaskExecutor` 或运行完整 executor loop。

## 主要符号

- `pub trait TaskExecutorRegistry: Send + Sync`：测试注册表边界。`register_executor(&self, task_type, extension) -> Result<(), DxfError>` 登记一种任务类型的扩展；`clear_executors(&self)` 清空注册项。`Send + Sync` 允许实现被装入 `Arc<dyn TaskExecutorRegistry>` 并跨线程安全共享，但具体同步策略由实现负责。
- `pub struct ExecutorRegistrationGuard`：仅持有私有字段 `registry: Arc<dyn TaskExecutorRegistry>`。外部不能直接操作该字段，只能通过守卫的作用域控制清理时机。
- `impl Drop for ExecutorRegistrationGuard`：无条件调用 `registry.clear_executors()`；显式 `drop(guard)` 和正常离开作用域都会触发相同清理。
- `pub fn InitTaskExecutor(...) -> Result<ExecutorRegistrationGuard, DxfError>`：公开注册入口。函数名保留 Go 风格，文件和 crate 也允许 `non_snake_case`，便于迁移代码按原名称调用。
- `pub fn run_registered_subtask(...) -> Result<(), DxfError>`：Rust 侧额外提供的生命周期驱动器，参数为既有扩展、任务和子任务的借用，不取得它们的所有权。

文件没有模块级可变全局、常量、枚举、条件编译项或异步函数。它使用的回调类型 `RunSubtaskFn`、任务类型常量 `TASK_TYPE_EXAMPLE`、扩展和步骤执行器都定义在相邻的 `disttest_util.rs`。

## 执行流程

`InitTaskExecutor` 的注册流程如下：

1. 接收一个共享注册表 `Arc<dyn TaskExecutorRegistry>` 和一个共享子任务回调 `Arc<RunSubtaskFn>`。
2. 调用 `GetCommonTaskExecutorExt` 构造扩展。传入的步骤执行器工厂读取 `task.base.step`，再调用 `GetCommonStepExecutor`，使创建出的 `StepExecutor` 同时绑定当前 step 和克隆后的 `run_subtask` 回调。
3. 调用 `registry.register_executor(TASK_TYPE_EXAMPLE, extension)`。
4. 注册失败时先调用 `clear_executors` 回滚，再原样返回注册错误；注册成功时返回持有同一注册表 `Arc` 的 `ExecutorRegistrationGuard`。
5. 守卫被丢弃时，`Drop::drop` 清空注册表。

`run_registered_subtask` 的执行流程如下：

1. `extension.get_step_executor(task)?` 根据任务创建步骤执行器；工厂错误立即返回。
2. `executor.init()?` 初始化执行器；初始化错误立即返回。
3. 调用 `executor.run_subtask(subtask)`，保存运行结果，但不立即用 `?` 返回。
4. 无论运行结果成功或失败，都调用一次 `executor.cleanup()`。
5. 返回 `result.and(cleanup)`：运行失败时保留运行错误；运行成功而清理失败时返回清理错误；两者成功才返回 `Ok(())`。

## 数据与状态

本文件自身不保存 task/subtask 的业务状态，也不读写持久化数据。注册阶段的持久状态完全位于调用者提供的 `TaskExecutorRegistry` 中；本文件只通过 trait 方法修改它。`ExecutorRegistrationGuard` 保存一个注册表的引用计数指针，因此即使调用方释放原始 `Arc`，注册表也会至少存活到守卫析构。

任务当前 step 来自 `Task.base.step`，被按值复制进 `StepExecutor`。子任务执行状态不在这里维护；`RunSubtaskFn` 接收 `&Subtask` 并返回 `Result<(), DxfError>`，副作用和错误由注入回调决定。相邻 `disttest_util.rs` 中的通用 `StepExecutor::init` 与 `cleanup` 当前恒成功，`run_subtask` 只是委托回调；因此该文件保留了完整生命周期接线，但当前通用桩不会自行产生初始化或清理错误。

## 依赖与调用关系

上游关系：

- `pkg/dxf/framework/testutil/lib.rs` 公开模块并再导出符号。
- `pkg/dxf/framework/testutil/migration_aster_unit_test.rs::executor_helpers_run_and_cleanup_with_raii_registration` 调用 `InitTaskExecutor`，并显式丢弃守卫以验证清理；同一测试调用 `run_registered_subtask` 验证回调到达。
- `pkg/dxf/framework/integrationtests/framework_test.rs::framework_executor_failure_is_returned_without_hiding_cleanup` 调用 `run_registered_subtask`，验证运行错误仍为 `"run failed"`。

下游关系：

- `crate::context::{DxfError, Subtask, Task}` 提供错误和任务数据模型。
- `crate::disttest_util::GetCommonTaskExecutorExt` 创建默认“错误不可重试”的 `TaskExecutorExtension`。
- `crate::disttest_util::GetCommonStepExecutor` 创建绑定 step 与回调的 `StepExecutor`。
- `crate::disttest_util::{RunSubtaskFn, TASK_TYPE_EXAMPLE, TaskExecutorExtension}` 分别定义回调边界、Go 兼容任务类型值和扩展对象。
- 标准库 `Arc` 管理注册表和回调的共享所有权。

RustCodeGraph 对目标文件给出的直接调用边包括 `InitTaskExecutor → register_executor`，并识别 `clear_executors` 被 `InitTaskExecutor` 的失败分支与守卫 `drop` 调用；目标文件被上述迁移契约测试和集成测试两个文件使用。Cargo 清单虽声明多个 DXF 子 crate 依赖，但本文件通过本 crate 的 `context` 和 `disttest_util` 间接使用它们，没有直接引用外部 crate 路径。

## 错误处理与边界

- 注册错误不包装、不改写，清理注册表后原样返回。`clear_executors` 没有返回值，因此无法向调用者报告回滚失败；实现必须把清理设计为可靠且可重复调用。
- 获取步骤执行器失败或 `init` 失败会通过 `?` 立即返回，此时没有已成功初始化的执行器生命周期需要由本辅助函数继续运行；尤其是 `init` 返回错误时，本函数不会调用 `cleanup`，这是当前明确边界。
- `run_subtask` 返回后始终求值 `cleanup`，所以运行失败不会跳过清理。
- `Result::and` 规定了双重失败时的优先级：若运行和清理都失败，返回运行错误而丢弃清理错误。当前通用 `StepExecutor::cleanup` 恒成功，因此测试只直接证明运行错误传播；若以后引入真实可失败清理，应新增独立测试锁定优先级。
- `Drop` 不捕获 panic。由于 trait 把 `clear_executors` 设计为无返回值，正常实现应避免在析构清理中 panic，尤其应避免在栈展开期间造成二次 panic。
- `InitTaskExecutor` 固定注册 `Example`，不接受任意 task type；需要其他类型时不应悄悄复用该函数。

## 并发与资源生命周期

该文件不启动线程、异步任务或通道，也不持有锁。并发保证来自接口约束：注册表必须同时满足 `Send` 和 `Sync`，回调 `RunSubtaskFn` 也要求 `Send + Sync`；`Arc` 只保证引用计数和共享所有权安全，不替注册表内部状态提供原子性。测试实现 `Registry` 因而用 `Mutex` 保护 `registered` 与 `cleared`。

资源生命周期的关键不变量是“成功注册返回一个守卫，守卫析构清空注册表；注册失败在返回前立即清空”。这避免注册项泄漏到后续测试，但 `clear_executors` 是全量清理而非仅撤销本次 `Example` 注册，因此同一注册表上并行运行相互独立的注册测试会互相影响。调用方应让守卫覆盖所有依赖该注册项的执行，并避免在同一全局注册表上并行使用多个此类守卫。

步骤执行器只在 `run_registered_subtask` 的栈帧中存活；`cleanup` 执行后函数返回并释放它。任务与子任务均为共享借用，不由该函数释放或修改所有权。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dxf/framework/testutil/executor_util.go`。两边都保留 `InitTaskExecutor` 名称，都用调用者注入的子任务回调构造通用 step executor，并固定面向 `proto.TaskTypeExample` / Rust 的 `TASK_TYPE_EXAMPLE`。

当前 Rust 移植有以下显式差异：

- Go 函数接收 `gomock.Controller`，通过 `GetCommonTaskExecutorExt` 和 `GetCommonStepExecutor` 建立 mock；Rust 用 trait、`Arc` 和闭包表达同一可注入边界。
- Go 直接调用全局 `taskexecutor.RegisterTaskType`，注册的工厂还创建 `NewBaseTaskExecutor` 并设置 `s.Extension`；Rust 把 `TaskExecutorExtension` 交给外部 `TaskExecutorRegistry`，此文件不创建生产 task executor，也不接收 Go 工厂里的 `context` 和 `Param`。
- Go 函数没有返回值，清理由 Go 测试控制器/测试进程的全局环境承担；Rust 返回 `Result` 和 RAII 守卫，并在失败或析构时显式全量清理，使隔离行为可测试。
- `run_registered_subtask` 是 Rust 为验证移植后的 extension/step executor 生命周期新增的辅助函数，Go 对照文件没有同名函数。

Go 的真实用例 `pkg/dxf/framework/taskexecutor/task_executor_testkit_test.go::TestTaskExecutorBasic` 用该注册函数运行两个 step、每步创建多个 subtask 并检查成功状态。Rust 当前直接相关测试覆盖了注册值、守卫清理、回调调用和运行错误传播，但没有在本文件层面复刻该完整 manager loop；不能据此宣称 Rust 辅助函数已经独立覆盖 Go 集成用例的全部行为。

## 扩展指南

- 新增其他 task type：优先增加接收 `task_type` 的通用注册入口，再让 `InitTaskExecutor` 保持 `Example` 兼容包装；同步检查注册失败回滚和多守卫并存语义。
- 改变执行器构造：修改 `InitTaskExecutor` 内传给 `GetCommonTaskExecutorExt` 的工厂，并确认仍从 `task.base.step` 选择正确 step、仍克隆而非搬走共享回调。
- 引入可失败的初始化或清理：修改真实定义所在的 `disttest_util.rs`，不要把执行逻辑复制进本文件；同时在独立测试文件中覆盖 factory 失败、init 失败、run/cleanup 各自失败及双重失败的错误优先级。
- 改变注册生命周期：应评估 `clear_executors` 的全量清理是否会破坏并行测试。若需要精确撤销，应让注册 API 返回注册句柄或提供按 task type 注销能力，而不是在 `Drop` 中猜测注册表内部结构。
- 测试应继续放在独立文件。针对本文件的契约测试应扩展 `pkg/dxf/framework/testutil/migration_aster_unit_test.rs`；跨 crate 的用户可见执行路径应扩展 `pkg/dxf/framework/integrationtests/framework_test.rs`。不要把测试模块内嵌回 `executor_util.rs`。
- 兼容风险主要是固定任务类型字符串、Go 风格公开函数名和错误优先级；性能风险很低，主要成本是每次构造执行器时克隆 `Arc`。若改用锁或全局表，应额外评估注册/清理竞争。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标目录查询识别 `executor_util.rs` 的 8 个符号，并识别两个直接使用文件。
- 源文件：`pkg/dxf/framework/testutil/executor_util.rs`，核对 `TaskExecutorRegistry`、`ExecutorRegistrationGuard`、`Drop::drop`、`InitTaskExecutor`、`run_registered_subtask` 的完整实现。
- crate 边界：`pkg/dxf/framework/testutil/Cargo.toml` 与 `pkg/dxf/framework/testutil/lib.rs`，核对 crate 名、Go 包映射、模块声明和公开再导出。
- 下游实现：`pkg/dxf/framework/testutil/disttest_util.rs`，核对 `RunSubtaskFn`、`TASK_TYPE_EXAMPLE`、`StepExecutor`、`TaskExecutorExtension`、`GetCommonTaskExecutorExt` 和 `GetCommonStepExecutor` 的真实语义。
- 框架定位：`pkg/dxf/framework/doc.go`，核对 DXF 的 task/step/subtask 抽象以及 task executor 在各节点执行任务的职责。
- Go 对照：`pkg/dxf/framework/testutil/executor_util.go` 与 `pkg/dxf/framework/taskexecutor/task_executor_testkit_test.go::TestTaskExecutorBasic`。
- Rust 独立测试：`pkg/dxf/framework/testutil/migration_aster_unit_test.rs::executor_helpers_run_and_cleanup_with_raii_registration` 与 `pkg/dxf/framework/integrationtests/framework_test.rs::framework_executor_failure_is_returned_without_hiding_cleanup`。
- RustCodeGraph 查询包括 `status`、目录 `files`、目标文件 `explore` / `node`，以及对 `InitTaskExecutor`、`run_registered_subtask`、`TaskExecutorExtension`、`GetCommonTaskExecutorExt`、`GetCommonStepExecutor` 和两个测试函数的精确查询。图中确认的关键边为 `InitTaskExecutor → register_executor`、失败分支与 `Drop::drop → clear_executors`，以及两个测试对公开辅助函数的调用。
- 本任务只新增说明文档，没有修改运行时代码；依照任务约束未运行 Cargo。最终结构以任务指定命令验证恰好包含 11 个固定二级章节。
