# `pkg/planner/cascades/task/task.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-cascades-task`，crate 根为同目录的 `lib.rs`。`lib.rs` 将本文件的公开项连同任务基类、规则任务、Group/GroupExpression 优化任务和调度器一起再导出，因此 `Stack`、`TaskError`、`newTaskStack`、`newTaskStackWithCap` 是该任务 crate 的公开 API；`takeTaskStack` 只在 crate 内可见，`putTaskStack` 还仅在测试编译中存在。

它提供两类基础设施：一是装载 `Box<dyn cascades_base::Task>` 的 LIFO 栈及线程本地复用池，二是把字符串消息包装为标准 Rust 错误的 `TaskError`。直接消费栈的生产代码是 `task_scheduler.rs` 中的 `SimpleTaskScheduler`。需要注意，`pkg/planner/cascades/cascades.rs` 当前优化器入口使用自己定义的 `SharedTaskScheduler`（内部为 `Rc<RefCell<Vec<Box<dyn Task>>>>`），并不直接持有本文件的 `Stack`；本文件的栈仍由公开的 `NewSimpleTaskScheduler` 路径实际使用，而不是整个 Cascades 主链唯一的栈实现。

## 核心职责

- `Stack` 用 `Vec<Box<dyn Task>>` 保存异构优化任务，约定向量尾部为栈顶，使 `Push`/`Pop` 都是尾部操作。
- `STACK_POOL` 在每个线程内保存可复用的 `Stack`，`takeTaskStack` 取出最近归还的栈，池空时创建默认容量为 4 的新栈。
- `Destroy` 清除仍被栈持有的任务，再把保留原容量的空栈归还当前线程的池，减少下一次调度器创建时的向量分配。
- `Desc` 按底到顶的向量迭代顺序调用每个任务的 `Task::Desc`，且每个任务后都追加换行；它描述当前栈状态，不改变栈内容。
- `TaskError` 为 Memo/规则任务之间提供轻量、可显示且实现 `std::error::Error` 的错误类型；其内容只有一条字符串消息，不保留结构化来源或错误链。

## 主要符号

- `pub struct TaskError(pub String)`：公开元组结构体；派生 `Clone`、`Debug`、`Eq`、`PartialEq`。`TaskError::New(message)` 接受任意 `Into<String>`，`Display` 原样输出内部字符串，空的 `impl Error` 使用标准默认行为。
- `thread_local! { static STACK_POOL: RefCell<Vec<Stack>> }`：线程局部、后进先出的栈对象池。它不是跨线程共享池，也没有锁。
- `pub struct Stack { tasks: Vec<Box<dyn Task>> }`：任务容器；字段私有，外部只能通过方法维护不变量。
- `newTaskStack() -> Stack`：创建容量 4、长度 0 的栈；也是池为空时的回退构造器。
- `newTaskStackWithCap(capacity: usize) -> Stack`：按调用者给定容量创建空栈，主要与 Go benchmark 构造器对齐；不会预先创建任务。
- `takeTaskStack() -> Stack`：从当前线程的池尾弹出一个栈，否则调用 `newTaskStack`。函数为 `pub(crate)`，生产调用点在 `NewSimpleTaskScheduler`。
- `putTaskStack(stack: Stack)`：仅在 `cfg(test)` 下编译，不清理栈而直接入池，用于验证 Go `sync.Pool.Put` 可归还“脏”对象的行为。
- `Stack::Destroy(&mut self)`：先 `clear` 释放所有任务，再以新默认栈替换 `self`，把原来已清空、且保留既有容量的栈放入池。调用后原变量本身仍是一个新的容量 4 空栈。
- `Stack::{Desc, Len, Cap, Pop, Push, Empty}`：分别负责描述输出、长度/容量观测、LIFO 弹压和空状态判断。`Pop` 在空栈返回 `None`。

文件没有模块级普通常量、枚举、trait、泛型类型、异步函数或 feature 分支；唯一条件编译项是测试辅助函数 `putTaskStack`。

## 执行流程

典型的简单调度流程如下：

1. `task_scheduler.rs::NewSimpleTaskScheduler` 调用 `takeTaskStack`。若线程池有对象，直接取得池尾栈；否则创建容量 4 的空栈。
2. `Scheduler::PushTask` 把 `Box<dyn Task>` 传给 `Stack::Push`，任务所有权进入向量尾部。任务执行时还可以继续压入派生任务，因此新任务会先于更早的待办执行。
3. `SimpleTaskScheduler::ExecuteTasks` 在 `!stack.Empty()` 时调用 `Pop`，取得栈顶任务并执行 `Task::Execute`。任何任务返回错误都会由 `?` 立即向上传播，尚未弹出的任务继续留在栈内。
4. `SimpleTaskScheduler::Destroy` 从自身的 `Option<Stack>` 取走栈并调用 `Stack::Destroy`。剩余任务被清除，原向量缓冲被归还当前线程池；调度器中的栈变为 `None`，后续执行或压栈会触发显式 `expect` panic。
5. 下一次同线程创建简单调度器时可复用刚归还的空栈及其容量。

`Desc` 是旁路诊断流程：它按 `tasks` 的存储顺序逐项调用动态分发的 `Task::Desc`，每项后写一个 `"\n"`。它不会按照实际弹栈顺序反向输出，也不会自动 `Flush` 写入器。

`TaskError` 的流程与栈相互独立。`base.rs::Context::{CopyIn, CopyInWithChildren}`、`task_apply_rule.rs::ApplyRuleTask::Execute` 以及 `cascades.rs` 的 Memo/优化器入口将底层错误文本转换为 `TaskError::New(error.to_string())`，之后可以作为标准错误继续传播。

## 数据与状态

`Stack` 的核心状态只有 `tasks` 的长度、容量和元素所有权。长度表示待执行任务数；容量仅表示当前向量无需重新分配即可容纳的元素数，不限制最大任务数。`Push` 可能扩容，`Pop` 降低长度但通常保留容量，`clear` 释放所有 `Box<dyn Task>` 指向的任务对象但保留向量分配。

池的状态是每线程一个 `Vec<Stack>`。归还和取出都操作该向量尾部，所以同一线程内是最近归还优先；不同线程看不到彼此归还的栈。池没有大小上限或主动收缩策略。线程结束时，线程局部值连同其中所有栈缓冲一起释放。

`Destroy` 有一个容易忽略的双重结果：池中收到的是调用前那个已清空的栈（保持它可能增长过的容量），调用者手中的 `Stack` 被替换成一个新的容量 4 空栈。正常生产路径随后丢弃调用者变量；若外部直接对一个仍会继续使用的 `Stack` 调用 `Destroy`，该变量在类型层面仍可再次压栈。

`TaskError` 只保存拥有所有权的 `String`。克隆会复制消息；相等性只比较消息文本。它没有 source、错误码或回溯字段。

## 依赖与调用关系

直接 Rust 依赖只有：

- `cascades_base::Task`：定义 `Execute` 与 `Desc` 的任务动态接口，`Stack` 存放其 trait object。
- `cascades_base::util::StrBufferWriter`：`Stack::Desc` 的输出抽象。
- 标准库 `RefCell`、`Error`、`fmt`、`Vec`、`Box` 和 `thread_local!`：实现线程局部可变池、错误展示和任务所有权。

`pkg/planner/cascades/task/Cargo.toml` 声明本 crate 还依赖 memo、pattern、rule、util、logicalop 等相邻 crate，但这些依赖由同 crate 的其他任务文件使用；`task.rs` 自身的直接源码依赖只有 `cascades-base` 和标准库。上层 `pkg/planner/cascades/Cargo.toml` 以本地 path 依赖名 `task` 引入整个任务 crate，根 workspace 另提供 `facade_planner_cascades_task` 别名。

可核实的调用边包括：

- `task_scheduler.rs::NewSimpleTaskScheduler -> takeTaskStack -> newTaskStack`（池空分支）。
- `SimpleTaskScheduler::{PushTask, ExecuteTasks, Destroy} -> Stack::{Push, Empty/Pop, Destroy}`。
- `Stack::Desc -> dyn Task::Desc`；这里是 trait 动态调用，具体实现取决于栈内任务类型。
- `Stack::Destroy -> newTaskStack`，并将清空后的旧栈写入 `STACK_POOL`。
- `base.rs`、`task_apply_rule.rs`、`cascades.rs` 和 `task_test.rs` 使用 `TaskError`；其中多处将 Memo/规则错误的 `to_string()` 结果包装进来。

RustCodeGraph 已索引目标目录的 17 个 Go/Rust 文件，并识别 `task.rs` 的 16 个符号。精确 `query` 找到了 `Stack`、`TaskError`、`newTaskStack`、`newTaskStackWithCap`、`takeTaskStack` 及各方法；调用图命令在本次环境中未在 30 秒内返回结果，因此调用边又用上述生产源码引用逐项核验，没有用无输出的图查询推断关系。

## 错误处理与边界

- `Stack::Pop` 把空栈视为正常边界，返回 `None`，不 panic。调度器先检查 `Empty` 再 `Pop().expect(...)`；该检查与弹出发生在同一可变借用内，当前单线程实现中不可能被并发修改。
- `Push` 接受所有实现 `Task` 且可装箱的值；Rust 类型系统不允许像 Go 测试那样压入无类型 `nil`。空状态只能由无元素表示。
- `Desc` 忽略写入器内部可能存在的 I/O 状态，因为 `StrBufferWriter::WriteString` 没有返回 `Result`；刷新由调用方负责。即使最后一个任务也会追加换行。
- `STACK_POOL.with` 和 `RefCell::borrow_mut` 在正常、非重入使用下不会失败；若在池已被可变借用期间重入同一池操作，`RefCell` 会 panic。本文件当前闭包不会在保持池借用时调用任务代码。
- `TaskError` 不改变消息，也不自动附加上下文。用 `error.to_string()` 包装会丢失原错误的类型与 `source` 链，扩展错误模型时必须考虑兼容现有文本断言。
- `newTaskStackWithCap` 的超大容量请求可能触发标准 `Vec` 分配失败行为；本文件不做容量上限校验。
- `Destroy` 是显式生命周期操作而不是 `Drop` 实现；忘记调用不会破坏正确性，但不会把缓冲归还池。重复对同一变量调用会不断把空栈放入池，因此调用方应遵守“一次调度生命周期一次销毁”的约定。

## 并发与资源生命周期

`STACK_POOL` 是 `thread_local`，所以栈复用完全局限于创建/销毁它的线程，不需要 `Mutex` 或原子操作，也不承诺跨线程负载均衡。`Stack` 内含 `Box<dyn Task>`，而 `Task` trait 没有 `Send`/`Sync` 约束；这一设计与 Cascades 当前 `Rc<RefCell<...>>` 单线程任务上下文一致，不能直接把栈或调度器移交线程池。

资源生命周期为“构造或取池 → 压入/弹出任务 → 显式 Destroy → 清空任务并还池”。弹出会把任务所有权移交调度器，任务执行结束后由正常作用域释放；错误短路时，当前失败任务在栈外释放，未执行任务仍在栈内，随后应通过调度器 `Destroy` 清理并归还缓冲。

线程退出会销毁其池，因此复用只优化线程存活期间的分配。由于池无上限，多个调度器归还的栈都会被保留到再次取用或线程退出；由于 `Destroy` 保留旧容量，曾经扩张很大的栈也可能长期占用该线程内存。这是性能与峰值内存之间的明确权衡。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/task/task.go`，测试对照是 `task_test.go`，调度消费方对照是 `task_scheduler.go` / `task_scheduler_test.go`。

- Rust `Stack { Vec<Box<dyn Task>> }` 对应 Go `Stack { []base.Task }`；两者都从尾部压弹并保持 LIFO。64 位测试分别确认 Rust `Vec`/Go slice 为 24 字节、Rust trait object box/Go interface 为 16 字节。
- Rust 默认容量 4、指定容量构造器、`Len`、`Pop`、`Push`、`Empty` 和逐项 `Desc` 加换行均保持 Go 行为。Rust 空栈返回 `Option::None`，对应 Go `nil`。
- Go 用全局并发安全 `sync.Pool`，可跨 goroutine/线程使用且运行时可丢弃缓存；Rust 用每线程 `RefCell<Vec<Stack>>`，缓存确定地留在本线程直到取走或线程结束。二者目标相同但缓存可见范围与回收策略不同。
- Go `Destroy` 清空切片后把同一个 `*Stack` 放入池；Rust 不能在仍借用 `&mut self` 时移动该值，因此先清空，再用新默认栈替换 `self`，把旧值入池。正常调度器销毁路径的外部效果相同，但直接调用后 Go 指针同时已进入池、不应再用，Rust 原变量则成为新的空栈。
- Go `sync.Pool.New` 负责池空构造；Rust `takeTaskStack().unwrap_or_else(newTaskStack)` 承担该分支。
- Go 测试能把 `nil` 接口压入栈做布局测量；Rust 的 `Box<dyn Task>` 不可为空，因此 Rust 测试直接通过类型尺寸验证胖指针布局。
- Go benchmark 使用真实 benchmark 次数；Rust 稳定测试环境没有 `#[bench]`，`task_test.rs` 只执行一次同形的 1000 次压入/弹出循环，证明路径可运行而不提供性能对等数据。
- `TaskError` 是 Rust 移植为跨 crate 明确引入的错误包装；Go 任务 API 直接使用内建 `error`，`task.go` 没有同名类型。

## 扩展指南

- 若要改变任务排序策略，修改点不应只局限于 `Push`/`Pop`：必须同步检查 `task_scheduler.rs`、`task_opt_group.rs`、`task_opt_group_expression.rs` 中依赖逆序压栈实现执行顺序的逻辑，并扩展独立测试 `task_test.rs` 与 `task_scheduler_test.rs`。FIFO 或优先队列会改变规则探索顺序和错误首先暴露的位置。
- 若要让栈跨线程使用，需要同时为 `Task` 增加合适的 `Send`（必要时 `Sync`）约束、替换 `Rc<RefCell>` 上下文、重新设计池同步方式，并审计所有具体任务；只把 `thread_local` 换成全局锁不足以保证安全。
- 若要限制缓存内存，可在 `Destroy` 中按 `Cap` 阈值决定是否保留原缓冲，或限制每线程池长度。应新增独立测试覆盖“大容量栈销毁后再获取”的容量策略，并评估频繁分配与常驻内存的权衡。
- 若要扩充 `TaskError` 为结构化错误，应优先修改 `TaskError` 及其 `Display/Error::source`，再同步 `base.rs` 的 Context 契约、`task_apply_rule.rs` 和 `cascades.rs` 的字符串映射点，以及相应错误断言。避免只增加字段却继续在所有入口调用 `to_string()`。
- 若要增加栈观测/调试输出，保持 `Desc` 不消费任务和不隐式刷新写入器的现有契约；新增格式必须考虑每项尾随换行的兼容性。
- Rust 单元测试继续放在独立的 `task_test.rs` / `task_scheduler_test.rs`，通过 `lib.rs` 的 `#[path] mod` 接入，不应把测试嵌入生产 `task.rs`。

## 验证依据

- 目标实现：`pkg/planner/cascades/task/task.rs`，逐项核对 `TaskError`、`STACK_POOL`、`Stack`、两个构造器、池函数及全部 `Stack` 方法。
- crate 边界：`pkg/planner/cascades/task/Cargo.toml` 与 `pkg/planner/cascades/task/lib.rs`，确认 crate 名、path 依赖、公开再导出和独立测试模块接线。
- 生产调用：`pkg/planner/cascades/task/task_scheduler.rs`，确认取池、LIFO 循环、错误短路和销毁归还；`pkg/planner/cascades/cascades.rs`，确认当前优化器主入口另用 `SharedTaskScheduler`，并确认 `TaskError` 的上游传播点。
- 契约与任务链：`pkg/planner/cascades/base/task_stack_base.rs`、`pkg/planner/cascades/task/base.rs`、`task_apply_rule.rs`、`task_opt_group.rs`、`task_opt_group_expression.rs`，确认动态 Task/调度契约与派生任务入栈关系。
- Rust 独立测试：`pkg/planner/cascades/task/task_test.rs` 验证布局、默认容量、LIFO、空弹栈、脏栈复用、Destroy 清空、指定容量循环及真实 Memo/规则任务链；`task_scheduler_test.rs` 验证 3→2 的 LIFO 执行顺序和任务 2 出错后短路。
- Go 对照：`pkg/planner/cascades/task/task.go`、`task_test.go`、`task_scheduler.go`、`task_scheduler_test.go`，确认 API、池化目的、边界行为和测试意图，并明确 `sync.Pool` 与线程本地池的实现差异。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、目标目录 17 个文件，`task.rs` 含 16 个符号；`query` 确认目标符号定义及 `TaskError` 在 `cascades.rs` 等处的引用。带 `--file` 的 callers/callees 查询本次未在 30 秒内返回，故调用边均由精确源码引用搜索和文件阅读补证。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构验证要求目标文件存在，并且固定的十一个二级标题各出现一次。
