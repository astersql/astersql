# `pkg/planner/cascades/base/task_stack_base.rs`

## 文件定位

本文件位于 `astersql-planner-cascades-base` crate，是 Cascades 优化器任务系统的契约层。crate 入口 `pkg/planner/cascades/base/lib.rs` 在 `base` 模块中通过 `include!("task_stack_base.rs")` 纳入本文件，并用 `pub use base::*` 对外重新导出其符号。因此，下游 crate 以 `cascades_base::{Stack, Task}` 使用这里的两个公开 trait，而不是直接把本文件编译成独立模块。

`pkg/planner/cascades/base/Cargo.toml` 将 `lib.rs` 指定为库入口，并把对应 Go 包记录为 `pkg/planner/cascades/base`。本文件自身只定义接口，不保存任务、不实现调度循环；生产栈、调度器和具体优化任务位于相邻的 `pkg/planner/cascades/task/` crate。

## 核心职责

- `Stack` 规定优化任务容器必须提供压栈、弹栈、判空和销毁四项操作。接口语义以 LIFO 为前提，但本文件不规定底层容器、容量策略或复用机制。
- `Task` 统一不同优化工作单元的执行与诊断描述能力。规则应用、组优化和组表达式优化等生产任务都可以装箱成 `Box<dyn Task>`，交给调度器动态分发。
- 两个 trait 共同切断调度机制与具体优化逻辑的静态耦合：调度器只需要操作任务对象，任务可以在执行期间继续派生新的任务。

当前迁移状态需要特别区分：`Task` 已有生产实现；`Stack` trait 在本仓库 Rust 代码中只找到测试实现。生产 `pkg/planner/cascades/task/task.rs::Stack` 提供同名固有方法，但没有显式 `impl cascades_base::Stack for Stack`。因此不能把 Go 侧“生产栈实现接口”的事实直接当成 Rust 已完成的接线。

## 主要符号

### `pub trait Stack`

- `fn Push(&mut self, one: Box<dyn Task>)`：取得任务所有权并把它放到容器顶端。`Box<dyn Task>` 同时承载堆对象所有权和动态分发表。
- `fn Pop(&mut self) -> Option<Box<dyn Task>>`：移除并交还栈顶任务。`None` 明确表达空栈，对应 Go `Pop() Task` 返回 `nil` 的分支。
- `fn Empty(&self) -> bool`：只读判空，供调度循环决定是否继续弹栈。
- `fn Destroy(&mut self)`：约定实现清理或重置其持有的任务资源。是否释放容量、归还对象池或仅清空元素由实现决定。

### `pub trait Task`

- `fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>>`：以独占可变借用执行任务；成功返回 `Ok(())`，失败以动态错误向调度器传播。
- `fn Desc(&self, w: &mut dyn util::StrBufferWriter)`：以共享借用把任务描述写入调用方提供的缓冲写入器。本方法没有返回值，trait 本身也没有规定刷新行为。

本文件没有常量、结构体、枚举、自由函数、默认方法或条件编译项。方法名沿用 Go 风格；crate 根 `lib.rs` 通过 `#![allow(non_snake_case)]` 允许这种命名。

## 执行流程

典型生产流程由下游代码补全，而非在本文件中实现：

1. 构造函数（如 `NewOptGroupTask`、`NewOptGroupExpressionTask`、`NewApplyRuleTask`）把具体任务装箱为 `Box<dyn Task>`。
2. `Scheduler::PushTask` 接收任务。生产 `SimpleTaskScheduler::PushTask` 将任务压入其具体 `task::Stack`。
3. `SimpleTaskScheduler::ExecuteTasks` 在栈非空时反复 `Pop`，随后调用动态分发的 `Task::Execute`。
4. 任务执行时可以通过其 `BaseTask` 和上下文继续压入派生任务。例如 `OptGroupExpressionTask::Execute` 先压规则任务，再逆序压子组任务；LIFO 顺序使第 0 个子组优先执行。
5. 任一 `Execute` 返回错误时，生产调度循环用 `?` 立即短路；全部成功且栈空时返回 `Ok(())`。
6. 生命周期结束时，调度器调用具体栈的 `Destroy`。生产栈会清空元素并把可复用容器放回线程本地池。

这里的第 2 至第 6 步证明 `Task` 契约已进入生产调度链，但生产调度器直接使用具体栈的固有方法，并未通过本文件的 `Stack` trait 进行动态分发。

## 数据与状态

本文件不拥有字段或全局状态。状态变化全部由 trait 实现承担：

- `Push` 把 `Box<dyn Task>` 的所有权转入实现，`Pop` 再把所有权移出，避免同一任务被容器和调用方同时拥有。
- `Execute(&mut self)` 允许具体任务更新探索标记、Memo 或自身进度；具体状态含义不属于该基础契约。
- `Desc(&self, ...)` 不允许通过普通字段修改任务，但写入器本身通过可变引用累积输出。
- `Destroy` 的后置状态没有在类型层编码。测试桩将容器清空；生产固有实现还涉及对象池复用。

相关布局证据在 `pkg/planner/cascades/task/task_test.rs::TestTaskStack`：64 位目标上 `Box<dyn Task>` 是包含数据指针与 vtable 指针的胖指针，测试期望大小为 16 字节；这是当前实现测试，不是 trait 对所有平台作出的 ABI 保证。

## 依赖与调用关系

向下依赖很小：`Task::Execute` 依赖标准库 `std::error::Error`，`Task::Desc` 依赖同一 crate 入口中定义的 `util::StrBufferWriter`。`pkg/planner/cascades/base/Cargo.toml` 没有声明外部依赖；writer 由 `lib.rs` 的 `util` 模块通过 `include!("../util/string_writer.rs")` 提供。

主要下游关系如下：

- `pkg/planner/cascades/base/task_scheduler_base.rs::Scheduler::PushTask` 直接接收 `Box<dyn Task>`，是任务进入调度器的基础接口。
- `pkg/planner/cascades/task/task_apply_rule.rs::ApplyRuleTask`、`task_opt_group.rs::OptGroupTask`、`task_opt_group_expression.rs::OptGroupExpressionTask` 实现 `Task`，分别承担规则应用、Memo Group 优化和 GroupExpression 优化。
- `pkg/planner/cascades/task/task_scheduler.rs::SimpleTaskScheduler::ExecuteTasks` 弹出任务并调用 `Execute`，形成直接运行链。
- `pkg/planner/cascades/task/task.rs::Stack::Desc` 遍历具体栈内的任务并调用 `Task::Desc`，形成诊断输出链。
- `pkg/planner/cascades/base/task_stack_base_test.rs` 和 `migration_aster_unit_test.rs` 提供 `Stack` 的测试实现，验证契约可实现及可被调度器式循环使用。

RustCodeGraph 能定位 `Stack`、`Task` 及上述源码，但对两个 trait 执行 `callers`/`callees` 没有产出调用边；因此实现列表和方法调用关系由限定在 `pkg/planner/cascades` 的精确引用检索及对应源码交叉核对。索引把 `Task` 这种通用名字映射到多个 crate，查询时必须用文件路径或符号 ID 消歧。

## 错误处理与边界

- 空栈是正常边界：`Stack::Pop` 返回 `Option`。独立测试 `pop_returns_none_for_an_empty_stack` 覆盖空栈、一次压入/弹出以及再次空栈；迁移测试也在 `Empty` 为假后才 `Pop`。
- 任务失败由 `Task::Execute` 返回的 `Box<dyn Error>` 表达。契约不要求具体错误类型，也没有恢复、重试或聚合策略；生产串行调度器在首个错误处短路。
- `Desc` 不返回 `Result`，所以该层无法表达写入失败；`StrBufferWriter` 的实现与刷新错误策略属于 writer 层。
- trait 没有规定 `Push` 是否接受逻辑上的“空任务”；Rust 的 `Box<dyn Task>` 本身不能是 Go 风格的 `nil` 接口值。
- trait 没有把“`Empty() == false` 后 `Pop()` 必为 `Some`”编码成单个原子操作。单线程、同一独占借用下生产调度器可依赖该不变量；并发实现若分别同步两个调用，仍需自行防止检查与弹栈之间的竞态。
- `Destroy` 没有规定幂等性或销毁后能否复用对象。调用方必须以具体实现的契约为准，不能只凭本 trait 假设。

## 并发与资源生命周期

`Stack` 与 `Task` 都没有 `Send`、`Sync` 或 `'static` 上界，本文件也不包含锁、通道、线程或异步接口。因此它只足以描述当前线程内的可变调度；不能据此断言 trait object 可跨线程转移或共享。

所有权路径是清晰的：构造者创建 `Box<dyn Task>`，`Push` 接管，`Pop` 交给执行者，任务在执行完成后随 box 离开作用域而释放。未弹出的任务应由具体容器在清空或析构时释放。

生产 `task::Stack` 的资源策略是线程本地 `STACK_POOL`：`Destroy` 清空任务，将当前容器替换为新栈，再把清空后的旧容器放回当前线程的池中。这是具体实现的生命周期，不是 `Stack` trait 的强制语义。生产 `SimpleTaskScheduler` 以 `Option<Stack>` 防止销毁后继续使用；这些约束同样没有进入本文件的类型签名。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cascades/base/task_stack_base.go`。Rust 保留了 Go 两个接口的名称、方法顺序和核心职责：

- Go `Stack.Push(one Task)` 对应 Rust `Push(Box<dyn Task>)`；Rust 显式转移堆对象所有权。
- Go `Stack.Pop() Task` 以 `nil` 表示空栈，Rust 用 `Option<Box<dyn Task>>` 把空值分支写入类型。
- `Empty` 与 `Destroy` 在两侧形状一致，但具体清理策略仍由实现决定。
- Go `Task.Execute() error` 对应 Rust `Result<(), Box<dyn Error>>`；两者都把失败交给调用方。
- Go `Task.Desc(w util.StrBufferWriter)` 对应 Rust 对 `dyn util::StrBufferWriter` 的可变借用。

Go 生产 `pkg/planner/cascades/task/task.go::Stack` 通过方法集隐式满足 `base.Stack`。Rust 没有结构化隐式接口：生产 `task.rs::Stack` 虽有相同方法，却未显式实现 `cascades_base::Stack`。另一方面，Rust 的 `Task` 已由生产任务显式实现。Go 的 `sync.Pool` 是跨线程可访问的并发池；Rust 当前具体实现使用 `thread_local!` 与 `RefCell`，复用范围限于线程内。这些都是迁移语义差异，而不是本基础 trait 自身的行为。

Go 测试 `pkg/planner/cascades/task/task_test.go` 验证接口值布局、LIFO、空栈返回 `nil`、未清理复用及 `Destroy` 清空。Rust 对照测试 `pkg/planner/cascades/task/task_test.rs` 保留这些意图，并以 `Box<dyn Task>`、`Option` 和线程本地池适配 Rust 所有权模型；本文件同目录的较小测试只验证基础 trait 可实现及空栈 `None`。

## 扩展指南

- 新增任务类型时，实现 `Task::Execute` 与 `Task::Desc`，并从构造入口返回 `Box<dyn Task>`。应在独立测试文件中覆盖成功路径、错误传播、描述输出，以及任务派生顺序；不要把测试写进生产 `.rs` 文件。
- 修改 `Task::Execute` 的错误边界会影响所有生产实现及 `Scheduler::ExecuteTasks`。优先保持可用 `?` 传播的统一返回形状，并同步 Go 对照语义。
- 修改 `Desc` 时需同时检查 `util::StrBufferWriter`、生产栈的 `Desc` 聚合格式及任务测试中的输出断言。刷新仍应由拥有 writer 的外层负责，除非契约明确改变。
- 若要让生产 `task::Stack` 真正通过此 trait 使用，应显式增加 `impl cascades_base::Stack for task::Stack`，再让调度器字段或测试以 trait 边界验证；不能仅因固有方法同名就认为已实现。此类改动应同步 `pkg/planner/cascades/base/task_stack_base_test.rs`、`migration_aster_unit_test.rs`、`pkg/planner/cascades/task/task_test.rs` 与调度器测试。
- 若引入并行调度，必须先决定是否给 `Task`/`Stack` 增加 `Send`/`Sync` 上界，并重新设计 `Empty` 加 `Pop` 的非原子组合、错误取消、剩余任务清理和对象池归属。直接给当前 trait object 跨线程使用并不安全可证。
- 性能敏感点主要在动态分发、每任务装箱和栈复用。任何消除装箱或更换容器的方案都应保留 LIFO 顺序、空栈语义和 `Destroy` 资源后置条件，并用独立 benchmark/测试与 Go 意图对照。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/cascades/base/task_stack_base.rs`，RustCodeGraph 文件节点显示完整 47 行及 `Stack`、`Task` 两个 trait。
- crate 装配与边界：`pkg/planner/cascades/base/lib.rs`、`pkg/planner/cascades/base/Cargo.toml`，确认 `include!`、公开再导出、库入口和 Go 包映射。
- Rust 生产调用链：`pkg/planner/cascades/base/task_scheduler_base.rs`、`pkg/planner/cascades/task/task.rs`、`task_scheduler.rs`、`task_apply_rule.rs`、`task_opt_group.rs`、`task_opt_group_expression.rs`。
- Rust 独立测试：`pkg/planner/cascades/base/task_stack_base_test.rs` 验证空栈 `None`；`migration_aster_unit_test.rs::migration_task_stack_and_scheduler_contracts_are_usable` 验证描述、压栈、执行和销毁契约；`pkg/planner/cascades/task/task_test.rs` 验证生产具体栈的布局、LIFO、对象池和清理行为。
- Go 对照：`pkg/planner/cascades/base/task_stack_base.go`、`pkg/planner/cascades/task/task.go`、`task_scheduler.go`、`task_test.go`。
- RustCodeGraph 查询：`status` 确认索引包含 11,467 个文件；`query Stack --kind trait` 与 `query Task --kind trait` 定位目标符号；`node --file ...` 读取目标及上下游；对目标 trait 的 `callers`/`callees` 查询无返回，随后用限定目录的精确引用检索补足图未覆盖的实现和调用证据。

未运行 Cargo 或测试二进制：本任务是纯文档分析，任务计划明确禁止运行 Cargo。结构完整性由任务指定的 11 章节校验命令验证；运行结果记录在最终交付说明中。
