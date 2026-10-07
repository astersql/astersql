# `pkg/planner/cascades/task/task_scheduler.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-task` crate；crate 入口 `pkg/planner/cascades/task/lib.rs` 通过 `mod task_scheduler` 纳入模块，并以 `pub use task_scheduler::*` 导出本文件的公开项。`pkg/planner/cascades/task/Cargo.toml` 将该 crate 定位为 Go 包 `pkg/planner/cascades/task` 的 Rust 移植，并通过 `cascades-base` 取得 `Scheduler`、`Task` 等基础契约。

它提供串行 LIFO（后进先出）调度器 `SimpleTaskScheduler`。需要区分“实现存在”和“主链已接线”：仓库内 Rust 生产代码当前没有调用 `NewSimpleTaskScheduler`，直接调用者只有独立测试 `task_scheduler_test.rs`；Rust 优化器主链 `pkg/planner/cascades/cascades.rs` 当前使用另一份私有实现 `SharedTaskScheduler`。Go 主链则在 `pkg/planner/cascades/cascades.go:NewContext` 中装配 `task.NewSimpleTaskScheduler()`。

## 核心职责

- `SimpleTaskScheduler::PushTask` 把 `Box<dyn Task>` 的所有权移入栈顶。
- `SimpleTaskScheduler::ExecuteTasks` 反复弹出栈顶任务并调用 `Task::Execute`，直到栈空或首个错误出现。
- `SimpleTaskScheduler::Destroy` 清空尚未执行的任务，把内部 `Stack` 归还线程本地池，并使调度器进入不可再执行/入队的终止状态。
- `NewSimpleTaskScheduler` 从任务栈池取得一个 `Stack`，以 `Box<dyn Scheduler>` 隐藏具体实现。

这些职责对应 `cascades-base` 中的 `Scheduler` trait；任务本身的业务逻辑仍由各 `Task` 实现承担，本文件只规定串行执行次序、错误短路和栈资源回收。

## 主要符号

- `pub struct SimpleTaskScheduler { stack: Option<Stack> }`：唯一状态是可被取走的任务栈。字段私有，调用方只能经 `Scheduler` 接口操作。
- `impl Scheduler for SimpleTaskScheduler`：实现三个接口方法：
  - `fn ExecuteTasks(&mut self) -> Result<(), Box<dyn Error>>`
  - `fn Destroy(&mut self)`
  - `fn PushTask(&mut self, task: Box<dyn Task>)`
- `pub fn NewSimpleTaskScheduler() -> Box<dyn Scheduler>`：公开构造入口；调用 `crate::takeTaskStack()` 复用当前线程的空闲栈。
- 下游契约位于 `pkg/planner/cascades/base/task_scheduler_base.rs`（`Scheduler`）和 `pkg/planner/cascades/base/task_stack_base.rs`（`Task`/抽象 `Stack`）；本 crate 使用的具体 `Stack` 及对象池位于 `pkg/planner/cascades/task/task.rs`。

本文件没有模块级常量、条件编译项或固有方法；除构造函数和公开结构体类型外，行为通过 `Scheduler` trait 暴露。

## 执行流程

1. `NewSimpleTaskScheduler` 调用 `takeTaskStack`。`task.rs` 中该函数先从线程本地 `STACK_POOL` 弹出可复用栈，池空时创建默认容量为 4 的栈。
2. 调用方通过 `PushTask` 压入任务；具体 `Stack::Push` 把任务追加到内部 `Vec` 尾部，尾部即栈顶。
3. `ExecuteTasks` 取得内部栈的可变引用；若调度器已经销毁，立即以明确消息 panic。
4. 循环先用 `Stack::Empty` 判断是否为空，再以 `Stack::Pop` 取出栈顶任务。当前实现持有同一个 `&mut Stack`，所以非空检查后的 `Pop` 应返回 `Some`；否则触发内部不变量 panic。
5. 调用 `task.Execute()`。成功则继续弹出；错误通过 `?` 原样向上传播并立即停止，剩余任务继续留在栈中。
6. 正常耗尽时返回 `Ok(())`。生命周期结束时应调用 `Destroy`：它以 `Option::take` 取走栈，再调用 `Stack::Destroy` 清空任务并归还线程本地池。

LIFO 是可观察语义而非实现偶然。例如 `task_opt_group_expression.rs:OptGroupExpressionTask::Execute` 逆序压入子 Group，正是为了让编号较小的子 Group 先从栈顶执行；若替换成 FIFO，必须同时审查这些反向压栈点。

## 数据与状态

`SimpleTaskScheduler` 的状态机可概括为：

- 活跃：`stack == Some(Stack)`，可入队、执行或销毁。
- 已销毁：`stack == None`，重复 `Destroy` 是空操作，但 `PushTask` 和 `ExecuteTasks` 会 panic。

具体 `Stack` 在 `task.rs` 中以 `Vec<Box<dyn Task>>` 保存任务，`Vec` 尾部为栈顶。`Box<dyn Task>` 让不同任务类型可共存，并把任务所有权从生产者转移给栈，再转移给执行循环。执行失败时，失败任务已被弹出并将在返回时释放；更早压入、尚未弹出的任务仍由栈持有，直到再次执行或 `Destroy`。

`Destroy` 不只是清空：`Stack::Destroy` 先清除任务，再以一个新空栈替换自身，并把原栈放回线程本地 `STACK_POOL`，从而保留其容量供同一线程后续调度器复用。

## 依赖与调用关系

直接依赖如下：

- `crate::{Stack, takeTaskStack}`：具体 LIFO 容器及线程本地池入口，定义于 `task.rs`。
- `cascades_base::{Scheduler, Task}`：调度器和任务的跨 crate 接口，分别定义于 `task_scheduler_base.rs`、`task_stack_base.rs`。
- `std::error::Error`：仅作为 `ExecuteTasks` 返回的动态错误边界。

已验证的调用边包括：`NewSimpleTaskScheduler -> takeTaskStack`；`PushTask -> Stack::Push`；`ExecuteTasks -> Stack::{Empty, Pop}` 以及 `Task::Execute`；`Destroy -> Stack::Destroy`。RustCodeGraph 将本文件标为由 `pkg/planner/cascades/task/task_scheduler_test.rs` 使用；精确符号查询也只找到该测试中的 Rust 构造调用。

任务派生的概念上游是 `BaseTask::Push -> Context::PushTask -> Scheduler::PushTask`，但当前 Rust 主链的 `Context` 把这条边接到 `cascades.rs:SharedTaskScheduler`，不是本文件。若未来把主链切换到 `SimpleTaskScheduler`，还需解决任务执行期间如何取得可入队的共享调度器句柄；当前本文件持有普通独占 `&mut self`，而 `SharedTaskScheduler` 使用 `Rc<RefCell<Vec<_>>>` 支持任务经克隆上下文继续压栈。

## 错误处理与边界

- 业务错误：任意 `Task::Execute` 返回的 `Box<dyn Error>` 被原样短路传播；调度器不包装、不记录、也不自动清除剩余任务。
- 空栈：`ExecuteTasks` 直接返回 `Ok(())`。
- 销毁后误用：`ExecuteTasks` 与 `PushTask` 分别通过 `expect` 产生带方法名的 panic；这是使用协议违例，不是可恢复错误。
- 重复销毁：`Option::take` 使第二次及后续 `Destroy` 安全无操作。
- 内部不变量：在同一独占借用内 `Empty == false` 后若 `Pop == None`，以 `expect("non-empty stack must pop a task")` panic。当前 `Vec` 栈实现不会触发该分支。
- 无自动回池：本类型没有 `Drop` 实现；若调用方直接丢弃调度器而不调用 `Destroy`，Rust 仍会释放任务和内存，但不会走 `STACK_POOL` 复用路径。主链接入时应保留显式清理协议或补充经过设计的自动回收策略。

## 并发与资源生命周期

调度循环是单线程串行的，没有锁、通道、后台任务或工作线程。底层池由 `thread_local!` + `RefCell<Vec<Stack>>` 实现，栈只在创建/销毁调度器的当前线程内取还。`Task` trait 未要求 `Send`/`Sync`，因此不能假定 `SimpleTaskScheduler` 可安全跨线程传递或共享。

生命周期为“取池中栈 → 多次压栈/执行 → 显式销毁 → 清空并归还池”。错误返回不会自动销毁栈，调用方仍负责最终调用 `Destroy`。任务执行期间当前函数一直保有内部 `Stack` 的可变引用；与 `SharedTaskScheduler` 在执行任务前释放 `RefCell` 借用的设计不同，本实现本身不提供可克隆句柄。安全扩展时尤其要避免让任务以别名方式重入同一个 `SimpleTaskScheduler`。

## 与 Go 版本的对应关系

Rust 文件逐项保留了 `pkg/planner/cascades/task/task_scheduler.go` 的行为形状：相同的 `SimpleTaskScheduler`/`NewSimpleTaskScheduler` 名称，相同的 LIFO 循环、首错短路、显式 `Destroy` 和栈池复用。Rust 用 `Option<Stack>` 表达 Go 在销毁时把 `s.stack` 设为 `nil`，并让重复销毁变为安全空操作；用 `Box<dyn Task>`/`Box<dyn Scheduler>` 对应 Go 接口值。

独立测试也对齐 `task_scheduler_test.go`：按 1、2、3 压栈，任务 2 返回 `mock error at task id = 2`。Rust 测试额外记录执行序列并断言为 `[3, 2]`，同时证明 LIFO 顺序和错误后任务 1 未执行。

主要迁移差异在装配层：Go `cascades.go:NewContext` 使用本调度器；Rust `cascades.rs:Context::NewContext` 使用私有 `SharedTaskScheduler`。后者通过 `Rc<RefCell<...>>` 让任务持有的上下文可以在执行期间继续入队。故当前不能仅把构造调用机械替换为本文件的工厂函数；应先统一上下文所有权/重入模型并用主链测试证明语义一致。

## 扩展指南

- 改变调度次序时，修改点主要是 `ExecuteTasks` 与 `Stack`，并必须同步审查 `task_opt_group_expression.rs` 等依赖 LIFO、使用逆序压栈的生产代码。
- 增加取消、预算或追踪时，可在 `ExecuteTasks` 的每次弹栈/执行边界接入；需明确错误时剩余任务是保留、清空还是回池，并为每种策略写独立测试。
- 改变销毁协议时，应同时修改 `stack: Option<Stack>` 的状态约束和 `task.rs:Stack::Destroy`，覆盖重复销毁、错误后销毁、销毁后误用及池复用。
- 若要把本实现接入 Rust `Optimizer`，应从 `cascades.rs:{Context, TaskContext, SharedTaskScheduler}` 入手，先设计可克隆任务上下文如何安全回到同一个调度器，不能只替换字段类型。兼容风险是派生任务顺序和错误短路；性能风险是 `Rc<RefCell>` 借用、对象池复用及动态分发开销变化。
- 测试逻辑应继续放在独立文件 `pkg/planner/cascades/task/task_scheduler_test.rs`，不要内嵌进生产源文件。至少保留现有 LIFO/错误短路用例，并建议补充空栈成功、成功耗尽、错误后 `Destroy`、重复 `Destroy`、销毁后误用以及栈池清空复用。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/planner/cascades/task` 找到本模块 17 个 Go/Rust 文件。
- RustCodeGraph 源码/关系查询：读取 `task_scheduler.rs` 全部 69 行，并报告其由 `task_scheduler_test.rs` 使用；查询 `SimpleTaskScheduler`/`NewSimpleTaskScheduler` 候选；读取 `task.rs`、`base/task_stack_base.rs`、`base/task_scheduler_base.rs`、`task/base.rs`、`task_opt_group_expression.rs` 和 `cascades.rs` 的相关符号与调用上下文。
- crate 与模块证据：`pkg/planner/cascades/task/Cargo.toml`、`pkg/planner/cascades/task/lib.rs`。
- Go 对照证据：`pkg/planner/cascades/task/task_scheduler.go`、`pkg/planner/cascades/task/task_scheduler_test.go`、`pkg/planner/cascades/cascades.go`。
- Rust 测试证据：`pkg/planner/cascades/task/task_scheduler_test.rs`；该测试直接覆盖构造、三次入栈、LIFO 顺序及错误短路。
- Rust 主链证据：`pkg/planner/cascades/cascades.rs` 中 `Context::NewContext`、`TaskContext::PushTask`、`Optimizer::{NewOptimizer, Execute, Destroy}`，证明当前生产优化路径使用 `SharedTaskScheduler`。
- 本任务是只读代码分析与文档新增，按计划不运行 Cargo；交付前仅执行固定 11 章节结构检查并人工复核上述事实。
