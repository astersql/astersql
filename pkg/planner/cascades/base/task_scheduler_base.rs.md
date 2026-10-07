# `pkg/planner/cascades/base/task_scheduler_base.rs`

## 文件定位

该文件定义 Cascades 优化任务调度器的公共 Rust 契约 `Scheduler`，位于 crate `astersql-planner-cascades-base` 的 `base` 模块中。crate 入口 `pkg/planner/cascades/base/lib.rs` 通过 `include!("task_scheduler_base.rs")` 将它与 `Task`、`Stack` 等基础抽象放在同一模块，再以 `pub use base::*` 从 crate 根导出。因此下游可以写 `cascades_base::Scheduler`；文件中的 `Task` 也直接解析为同一模块里 `task_stack_base.rs` 定义的 trait，而不需要额外 `use`。

`pkg/planner/cascades/base/Cargo.toml` 只声明库入口和 Go 包迁移元数据，没有运行时依赖或 feature。真正的串行栈实现位于下游 crate 的 `pkg/planner/cascades/task/task_scheduler.rs`，完整优化器中另有 `pkg/planner/cascades/cascades.rs::SharedTaskScheduler`。本文件本身不是调度算法实现，也不拥有队列、线程或 Memo。

## 核心职责

本文件只规定调度器实现必须提供三项能力：

1. `ExecuteTasks` 驱动实现内部的待执行任务集合，并把执行失败返回给调用方。
2. `PushTask` 接收一个拥有所有权的动态 `Task`，使外部入口或正在执行的任务能够派生后续工作。
3. `Destroy` 为实现持有的栈、队列、工作线程或其他资源提供显式清理入口。

这三个方法共同建立“入队—驱动—清理”的最小边界，但不规定 FIFO/LIFO、单线程/多线程、错误后是否保留剩余任务、是否可以重复执行或销毁。源码注释允许串行或并发实现；具体语义必须由实现者补充并由其测试证明。

## 主要符号

- `pub trait Scheduler`：唯一的模块级类型，也是公共 API。它没有关联类型、默认方法、常量、泛型参数或条件编译项。
- `fn ExecuteTasks(&mut self) -> Result<(), Box<dyn Error>>`：要求对调度器进行独占可变访问；成功返回 `Ok(())`，失败以动态标准错误返回。方法名用 `#[allow(non_snake_case)]` 保留 Go API 拼写。
- `fn Destroy(&mut self)`：显式资源清理钩子，不返回错误，也没有默认实现；它不是 Rust `Drop`，离开作用域不会因为该 trait 自动调用它。
- `fn PushTask(&mut self, task: Box<dyn Task>)`：把任务对象所有权移入调度器。`Box<dyn Task>` 保留 Go 接口值的动态分派能力；调用后调用方不能继续直接持有该任务值。

`Task` 的相邻定义位于 `pkg/planner/cascades/base/task_stack_base.rs`：`Execute(&mut self)` 执行任务并返回同形状的动态错误，`Desc` 写出描述。因此调度器只负责组织任务生命周期，不解释任务类型或业务内容。

## 执行流程

从完整 Cascades 优化链观察，典型流程如下：

1. `pkg/planner/cascades/cascades.rs::Optimizer::NewOptimizer` 初始化 Memo，并通过 `Context::PushTask` 压入根 Group 的 `NewOptGroupTask`。
2. `Context::PushTask` 或 `pkg/planner/cascades/base/cascadesctx/cascades_ctx.rs::Context::PushTask` 把任务转发给调度器的 `PushTask`。
3. `Optimizer::Execute` 调用调度器的 `ExecuteTasks`。具体实现反复取得待执行任务并调用 `Task::Execute`；任务执行期间可以再次经任务上下文入队。
4. 执行循环在实现认定任务集合为空时成功结束，或把首个任务错误向上返回。首错短路是当前 `SimpleTaskScheduler` 和 `SharedTaskScheduler` 的实现事实，不是 trait 文法强制的算法。
5. 所有者结束使用后调用 `Optimizer::Destroy` / `Context::Destroy`，最终转发到调度器 `Destroy` 清理其内部资源。

当前 `pkg/planner/cascades/task/task_scheduler.rs::SimpleTaskScheduler` 使用 `Stack` 做 LIFO 循环；`pkg/planner/cascades/cascades.rs::SharedTaskScheduler` 使用 `Rc<RefCell<Vec<Box<dyn Task>>>>`，在每次执行任务前释放对向量的可变借用，使任务可以在执行中继续压入工作。

## 数据与状态

`Scheduler` trait 自身没有字段或持久状态。唯一跨边界的数据是 `Box<dyn Task>`：

- `Box` 表示任务的堆分配与所有权转移；
- `dyn Task` 隐藏具体探索、规则应用或实现任务类型；
- trait 没有为任务增加 ID、优先级、取消标记或结果通道。

状态布局由实现决定。`SimpleTaskScheduler` 保存 `Option<Stack>`，`Destroy` 后取走并清理栈，随后再次 `ExecuteTasks` 或 `PushTask` 会 panic；`SharedTaskScheduler` 保存可克隆的 `Rc<RefCell<Vec<_>>>`，`Destroy` 只清空共享向量。调用方不能从 `Scheduler` 接口查询队列长度、销毁状态或当前任务。

## 依赖与调用关系

直接依赖只有标准库 `std::error::Error` 和同模块的 `Task`。base crate 的 Cargo 清单没有外部依赖；下游 `pkg/planner/cascades/task/Cargo.toml`、`pkg/planner/cascades/Cargo.toml` 与 `pkg/planner/cascades/base/cascadesctx/Cargo.toml` 分别通过本地 path 依赖接入 `astersql-planner-cascades-base`。

已核验的主要上游和实现边如下：

- `cascadesctx::Context::PushTask -> GetSchedulerMut -> Scheduler::PushTask`；
- `cascades.rs::Context::PushTask -> SharedTaskScheduler::PushTask`；
- `cascades.rs::Optimizer::Execute -> SharedTaskScheduler::ExecuteTasks`；
- `cascades.rs::Optimizer::Destroy -> Context::Destroy -> SharedTaskScheduler::Destroy`；
- `task_scheduler.rs::SimpleTaskScheduler implements Scheduler`，由 `NewSimpleTaskScheduler` 以 `Box<dyn Scheduler>` 返回。

RustCodeGraph 能定位 trait、三个方法、上述两个生产实现及测试实现；其精确 `callers` 查询没有返回可用的绑定边，因此方法调用点又用限定目录的 `rg` 交叉核验。这里不把同名的 DXF 或资源管理调度器算作本接口调用者。

## 错误处理与边界

`ExecuteTasks` 是唯一可报告失败的方法，错误类型为 `Box<dyn Error>`。它保留 Go `error` 的开放错误集合，但签名未附加 `Send + Sync`，不能据此推断错误可安全跨线程传递。`PushTask` 和 `Destroy` 没有返回值，接口也没有队满、拒绝入队或清理失败的表达通道。

当前生产实现都用 `?` 原样传播 `Task::Execute` 的第一个错误。独立测试 `pkg/planner/cascades/task/task_scheduler_test.rs::TestSimpleTaskScheduler` 和 `pkg/planner/cascades/base/cascadesctx/cascades_ctx_aster_unit_test.rs::scheduler_executes_lifo_and_stops_at_first_error` 证明：按 1、2、3 入队时先执行 3，任务 2 失败后任务 1 保留且不执行。该结论描述现有实现；新的实现若改变错误聚合或剩余任务策略，应显式记录兼容性差异。

接口没有规定空队列、重复 `Destroy`、销毁后调用、任务 panic、任务在执行中递归入队等边界。现有 `SimpleTaskScheduler` 对空栈返回成功、重复销毁无动作，但销毁后的执行/入队会 panic；这些约束位于实现文件，不能仅凭本 trait 假定所有实现相同。

## 并发与资源生命周期

所有方法都接收 `&mut self`，单个调度器值在普通调用路径上需要独占可变借用。trait 没有 `Send`、`Sync` 或异步约束，`Box<dyn Task>` 也没有这些边界，所以本文件不承诺跨线程调度。若实现多线程调度器，必须自行增加线程安全容器、工作线程停止协议以及适当的任务/错误 trait bound，并在具体构造 API 上表达这些约束。

当前 `SharedTaskScheduler` 的 `Rc<RefCell<_>>` 明确是单线程共享内部可变性：克隆句柄允许任务执行时继续入队，但不能跨线程；运行循环在调用 `Task::Execute` 前释放 `RefCell` 借用，避免重入入队触发借用冲突。`SimpleTaskScheduler` 则以单一 `&mut` 栈串行执行。

资源生命周期是显式的：`PushTask` 将任务交给实现，弹出后所有权交给执行局部变量，执行结束即释放；未执行任务由队列/栈持有，`Destroy` 应负责释放或回收。由于 `Scheduler` 未继承 `Drop`，所有者必须按具体实现契约调用 `Destroy`；接口也不保证销毁会等待后台线程，因为当前 trait 没有定义等待或 join 的结果。

## 与 Go 版本的对应关系

直接对照文件 `pkg/planner/cascades/base/task_scheduler_base.go` 同样只定义 `Scheduler` 接口，方法顺序和职责一一对应：`ExecuteTasks() error`、`Destroy()`、`PushTask(task Task)`。Rust 保留了 Go 方法名和顺序，并用 `Result<(), Box<dyn Error>>` 映射 `error`，用 `Box<dyn Task>` 映射动态接口值及任务所有权转移。

主要语言差异是：Go 通过方法集隐式满足接口，Rust 必须显式 `impl Scheduler for T`；Go 接口值没有在签名中表达独占借用，Rust 三个方法均要求 `&mut self`；Rust 的任务所有权在 `Box` 中显式，而 Go 由接口值和垃圾回收管理。两边的基础接口都只宣称可由串行或并发实现，并未规定具体队列顺序。

生产对照 `pkg/planner/cascades/task/task_scheduler.go::SimpleTaskScheduler` 与 Rust 同名实现均使用栈、遇到首错即返回、显式销毁栈。Rust 将栈放入 `Option` 以表示已销毁状态；Go 将字段置为 `nil`。Go 测试 `task_scheduler_test.go::TestSimpleTaskScheduler` 验证固定错误，Rust 对应测试额外记录执行序列，明确验证 LIFO 与短路后的 `[3, 2]`。

## 扩展指南

新增调度器实现时，应首先判断是否能复用本 trait；需要修改本文件的典型情况只有新增所有实现都必须支持的能力，例如取消、结果收集或可观测性。给 trait 增加必需方法会破坏所有生产和测试实现，至少要同步检查 `SimpleTaskScheduler`、`SharedTaskScheduler`、`cascadesctx` 的 `TestScheduler` 与 base 迁移测试的 `SerialScheduler`。

若只改变排队策略、优先级或并发模型，应优先在具体实现和构造函数中扩展，避免把实现细节塞入基础接口。必须继续保证任务执行期间可安全 `PushTask`；使用锁或 `RefCell` 时，不应在持有队列可变借用/锁的情况下调用外部任务代码，否则会造成重入失败或死锁。多线程实现还需要决定 `Task`、错误和调度器是否增加 `Send + Sync`，并明确 `Destroy` 是取消、排空还是等待完成。

测试逻辑必须放在独立文件中，不要内嵌进本生产文件。接口契约可扩展 `pkg/planner/cascades/base/migration_aster_unit_test.rs`；串行生产实现行为应扩展 `pkg/planner/cascades/task/task_scheduler_test.rs` 并同步 Go 测试意图；上下文转发和任务内再入队应扩展 `pkg/planner/cascades/base/cascadesctx/cascades_ctx_aster_unit_test.rs`。重点覆盖空队列、LIFO/优先级、首错后的剩余任务、重复/销毁后调用、任务内派生任务以及资源释放。

## 验证依据

- 目标与相邻契约：`pkg/planner/cascades/base/task_scheduler_base.rs::Scheduler`、`pkg/planner/cascades/base/task_stack_base.rs::{Task, Stack}`。
- crate 边界：`pkg/planner/cascades/base/lib.rs` 和 `pkg/planner/cascades/base/Cargo.toml`；下游依赖由 `pkg/planner/cascades/{Cargo.toml,task/Cargo.toml,base/cascadesctx/Cargo.toml}` 核验。
- 生产实现与调用链：`pkg/planner/cascades/task/task_scheduler.rs::{SimpleTaskScheduler, NewSimpleTaskScheduler}`、`pkg/planner/cascades/cascades.rs::{SharedTaskScheduler, Context, Optimizer}`、`pkg/planner/cascades/base/cascadesctx/cascades_ctx.rs::Context`。
- Go 对照：`pkg/planner/cascades/base/task_scheduler_base.go`、`pkg/planner/cascades/task/task_scheduler.go`、`pkg/planner/cascades/task/task_scheduler_test.go`。
- Rust 独立测试：`pkg/planner/cascades/task/task_scheduler_test.rs`、`pkg/planner/cascades/base/migration_aster_unit_test.rs`、`pkg/planner/cascades/base/cascadesctx/cascades_ctx_aster_unit_test.rs`。
- RustCodeGraph：索引状态为 11,467 个文件；目标文件识别出 5 个节点；`node` 核验 trait、相邻 `Task`、两个生产实现和测试实现；`query` 找到 `ExecuteTasks`/`PushTask` 的生产及测试候选。由于精确 `callers` 未产出绑定结果，调用点用 `rg` 在 `pkg/planner/cascades/**/*.rs` 中复核。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前以任务规定的命令检查目标文件存在且恰有 11 个固定二级章节，并人工复核没有把实现特性提升为接口保证。
