# `pkg/planner/cascades/task/base.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-task` crate，是 Cascades 搜索任务层的共享契约与轻量基类。crate 入口 [`lib.rs`](./lib.rs) 将本模块私有挂载后再公开导出全部符号；上层 `pkg/planner/cascades` 通过 Cargo 依赖名 `task` 使用它。它位于优化器入口与具体任务之间：上层 [`../cascades.rs`](../cascades.rs) 创建实现 `task::Context` 的 `TaskContext`，具体的 `OptGroupTask`、`OptGroupExpressionTask` 和 `ApplyRuleTask` 则持有这里定义的 `BaseTask`。

该文件本身不执行规则匹配、Memo 搜索或调度循环。它只规定具体任务可以向优化器请求哪些服务，并把“派生任务入栈”这一共同操作集中到 `BaseTask::Push`。真正的 LIFO 执行循环位于 [`task_scheduler.rs`](./task_scheduler.rs) 及上层 `SharedTaskScheduler`。

## 核心职责

- `Context` trait 划定任务与优化器状态之间的边界：任务可以入队、向指定 Group 写入或移除逻辑表达式、按 `Operand` 查询规则，以及检查规则开关。
- `ContextRef` 为这些服务提供可克隆的、单线程共享且内部可变的动态分发句柄。
- `BaseTask` 保存该句柄，让三种具体任务复用同一上下文；`Push` 只调用 `Context::PushTask`，不会在当前调用栈中直接执行新任务。
- 默认的 `Context::CopyInWithChildren` 保持向后兼容：未覆写时忽略显式 `child_groups`，退化为普通 `CopyIn`。生产 `TaskContext` 会覆写它，把绑定得到的孩子 Group 一并交给 Memo。

## 主要符号

- `pub trait Context`：任务所需服务的最小接口。
  - `PushTask(&mut self, Box<dyn Task>)` 把类型擦除后的任务交给调度器。
  - `CopyIn(&mut self, &GroupRef, LogicalPlanRef) -> Result<GroupExpressionRef, TaskError>` 把普通逻辑计划写入目标 Group，并把 Memo 错误显式返回。
  - `CopyInWithChildren(...)` 接受规则绑定产生的孩子 Group；默认实现调用 `CopyIn`，生产实现位于 [`../cascades.rs`](../cascades.rs) 的 `impl task::Context for TaskContext`。
  - `RemoveOut` 删除目标 Group 中的旧表达式；接口没有返回值。
  - `RulesFor(Operand) -> Vec<RuleRef>` 返回该算子类型可用规则的拥有型列表。
  - `RuleEnabled(usize) -> bool` 以规则 ID 查询掩码。
- `pub type ContextRef = Rc<RefCell<dyn Context>>`：动态 trait object 的共享句柄；`Rc` 提供同线程引用计数，`RefCell` 把借用检查推迟到运行时。
- `pub struct BaseTask { pub ctx: ContextRef }`：可 `Clone` 的公共包装。克隆只增加 `Rc` 计数，不复制 Memo、规则表或任务栈。
- `BaseTask::New(ContextRef) -> Self`：保存已有上下文，不做初始化或校验。
- `BaseTask::Push(&self, Box<dyn Task>)`：取得上下文的独占可变借用并转发到 `PushTask`。

文件没有模块级常量、枚举、条件编译项或私有辅助函数。

## 执行流程

1. `Optimizer::NewOptimizer` 在 [`../cascades.rs`](../cascades.rs) 初始化 Memo 根 Group，并通过 `Context::TaskContext` 组装共享 Memo、调度器、规则掩码和规则表。
2. 上层把 `NewOptGroupTask(context, root_group)` 压入调度器；构造函数以 `BaseTask::New` 保存同一个 `ContextRef`。
3. `OptGroupTask::Execute` 枚举 Group 内尚未探索的 GroupExpression，并调用 `BaseTask::Push` 派生 `OptGroupExpressionTask`。
4. `OptGroupExpressionTask::Execute` 通过 `RulesFor` 和 `RuleEnabled` 过滤规则，再用 `BaseTask::Push` 派生 `ApplyRuleTask`；它也会为尚未探索的孩子 Group 派生 `OptGroupTask`。
5. `ApplyRuleTask::Execute` 对绑定运行规则，调用 `CopyInWithChildren` 写入新表达式，随后再次压入表达式优化任务；当规则要求擦除原表达式时调用 `RemoveOut`。
6. 调度器稍后按 LIFO 顺序弹出这些任务。`BaseTask::Push` 在步骤 3 至 5 中均只入栈，所以任务执行不会发生递归内联。

## 数据与状态

`BaseTask` 唯一字段是 `ctx`。生产 `TaskContext` 将它间接连接到四类共享状态：`Rc<RefCell<Memo>>`、`SharedTaskScheduler`、`Rc<RefCell<RuleMask>>` 和按 `Operand` 分组的规则表。任务因此看到的是同一次优化阶段的统一状态，而不是快照。

`Context` 方法有意使用不同所有权形式：Group 和 GroupExpression 以引用计数句柄传递；新逻辑计划使用 `LogicalPlanRef` 转移所有权；任务以 `Box<dyn Task>` 转移到调度器；`RulesFor` 返回克隆后的 `Vec<RuleRef>`，避免调用者在持有规则表借用时继续调度。`CopyInWithChildren` 的 `child_groups` 也按值传递，生产实现把这些 Group 作为新表达式的明确孩子关系写入 Memo。

`BaseTask::Clone` 共享上下文。只要任一克隆或由它创建的具体任务仍持有 `ContextRef`，上下文及其引用的优化状态就不会仅因另一个任务结束而释放。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](./Cargo.toml) 声明。当前文件直接依赖：

- `cascades-base` 的 `Task`，用于调度参数的动态任务接口；
- `cascades-pattern` 的 `Operand`，用于规则索引；
- `logicalop` 的 `LogicalPlanRef`，用于 Memo 写入；
- 本 crate 从 `cascades-memo`、`cascades-rule` 再导出的 `GroupRef`、`GroupExpressionRef`、`RuleRef`，以及本 crate 的 `TaskError`；
- 标准库 `Rc` 与 `RefCell`。

直接上游调用者可由三个构造点确认：[`task_opt_group.rs`](./task_opt_group.rs)、[`task_opt_group_expression.rs`](./task_opt_group_expression.rs)、[`task_apply_rule.rs`](./task_apply_rule.rs) 都调用 `BaseTask::New`；这三个文件也分别调用 `BaseTask::Push` 或经 `BaseTask.ctx` 调用 `Context` 服务。生产实现和应用入口在 [`../cascades.rs`](../cascades.rs)：`TaskContext` 实现全部六个服务，`Optimizer::NewOptimizer` 压入根任务，`Optimizer::Execute` 驱动共享调度器。

## 错误处理与边界

只有可能失败的 Memo 写入以 `Result<_, TaskError>` 暴露；具体任务用 `?` 或转换后的 `TaskError` 向调度循环传播。`PushTask`、`RemoveOut`、规则查询和 `BaseTask::Push` 没有可恢复错误通道，因此实现方必须维护目标 Group、表达式归属和调度器生命周期等前置条件。

`RefCell` 违反动态借用规则时会 panic。特别是调用 `BaseTask::Push` 时，调用方不能仍持有同一 `ContextRef` 的活动借用。上层 `SharedTaskScheduler::ExecuteTasks` 特意先结束任务栈借用再调用 `Task::Execute`，正是为了允许执行中的任务继续入栈。

默认 `CopyInWithChildren` 忽略 `child_groups`，只适合能从逻辑计划本身正确恢复孩子关系的实现。需要保留 Binder 已匹配 Group 的上下文必须像生产 `TaskContext` 一样覆写该方法，否则会丢失显式孩子绑定语义。`BaseTask::New` 不拒绝处于已销毁状态的上下文；是否可继续调度由具体 `Context` 实现保证。

## 并发与资源生命周期

`Rc<RefCell<...>>` 明确限定为单线程模型：`ContextRef` 不提供 `Send`/`Sync` 保证，不能直接跨线程传递。这里的“共享”指同一线程的多个任务和包装对象共享优化阶段状态；互斥由 Rust 动态借用规则而非锁完成。

任务的典型生命周期是“构造并持有 `ContextRef` → 装箱入栈 → 调度器取得所有权并执行 → 执行期间派生更多共享同一上下文的任务 → 执行结束后释放该任务的引用”。调度顺序由外部栈决定，当前生产调度器为 LIFO。资源清理由上层 `Context::Destroy`/调度器 `Destroy` 完成，本文件没有显式清理或后台任务。

由于 `ContextRef` 可能间接连接调度栈，而栈中任务又持有 `ContextRef`，扩展实现时应避免让 Context 自身强引用含有自己的任务形成无法清理的长期环；现有优化流程通过排空或清空任务栈终止持有关系。

## 与 Go 版本的对应关系

[`base.go`](./base.go) 的 `BaseTask` 只保存 `cascadesctx.Context`，`Push` 直接调用 `ctx.PushTask`；Rust 的 `BaseTask` 与这两点保持一致，并增加 `New` 构造函数和 `Clone`。Go 的接口值天然支持共享动态分发，Rust 用 `Rc<RefCell<dyn Context>>` 显式表达同线程共享和内部可变性。

Rust 的 `Context` trait 比 Go [`../base/cascadesctx/cascades_ctx.go`](../base/cascadesctx/cascades_ctx.go) 中的基础接口更贴近任务实际需求：Go 任务通过 `GetMemo()`、`GetRuleMask()` 等入口取得对象后操作，Rust 则把 `CopyIn`、`CopyInWithChildren`、`RemoveOut`、`RulesFor` 和 `RuleEnabled` 收窄为直接能力。这避免具体任务依赖完整 Memo/位集合类型，但也意味着新增任务能力时必须同步修改 trait 及其生产、测试实现。

两边的 `Push` 都只调度。Go `SimpleTaskScheduler` 与 Rust 调度器都使用栈，因此派生任务采用后进先出顺序；Rust 独立测试还验证了失败任务会使执行循环短路。

## 扩展指南

- 新增只需现有能力的任务时，嵌入或持有 `BaseTask`，由构造函数接收 `ContextRef`，并通过 `Push` 派生后续任务；不要在任务内复制 Memo 或自行递归执行子任务。
- 新增上下文能力时，先判断它是否确属所有任务的最小公共边界。若必须加入 `Context`，需同步更新 [`../cascades.rs`](../cascades.rs) 的生产 `TaskContext` 和 [`task_test.rs`](./task_test.rs) 的 `TaskContextHarness`，并为成功、失败或开关分支补充独立测试，不能把测试写进 `base.rs`。
- 修改 `CopyInWithChildren` 时必须维护“目标 Group、逻辑表达式、绑定孩子 Group”三者一致性；应覆盖无孩子、多个孩子、重复表达式和 Memo 拒绝写入等边界。默认退化行为的兼容性也需单独评估。
- 改变共享指针或调度模型属于高风险修改：从 `Rc<RefCell<_>>` 迁移到多线程结构会影响所有任务构造、动态借用语义和调度顺序。性能上还需关注 `RulesFor` 克隆规则向量及频繁 `RefCell` 借用的成本。
- 若改变 `Push` 的“只入栈”不变量，必须同步审查三个具体任务的压栈顺序以及 [`task_scheduler_test.rs`](./task_scheduler_test.rs) 的 LIFO/错误短路断言；内联执行会改变搜索顺序并可能引入深递归。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/planner/cascades/task/base.rs --offset 1 --limit 260` 读取到完整 78 行与 `Context`、`ContextRef`、`BaseTask`；`query BaseTask --kind struct` 同时定位 Rust 和 Go 定义。方法级 callers 查询未及时返回，因此调用边用下述精确源码搜索补证。
- 目标与模块：[`base.rs`](./base.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)。Cargo 文件确认 crate 名、入口和六个本地依赖；本文件没有 feature 或条件编译分支。
- 直接调用边：[`task_opt_group.rs`](./task_opt_group.rs)、[`task_opt_group_expression.rs`](./task_opt_group_expression.rs)、[`task_apply_rule.rs`](./task_apply_rule.rs) 中的 `BaseTask::New`、`BaseTask::Push` 与各 `Context` 方法调用。
- 生产接线：[`../cascades.rs`](../cascades.rs) 中的 `SharedTaskScheduler`、`TaskContext`、`impl task::Context for TaskContext`、`Context::TaskContext`、`Optimizer::NewOptimizer` 和 `Optimizer::Execute`。
- Go 对照：[`base.go`](./base.go) 与 [`../base/cascadesctx/cascades_ctx.go`](../base/cascadesctx/cascades_ctx.go)，并复核三个具体任务的同路径 `.go` 文件。
- 独立测试：[`task_test.rs`](./task_test.rs) 的 `TaskContextHarness` 和 `task_chain_uses_real_memo_and_rule_contracts` 覆盖真实 Memo、规则、派生任务与探索标记；[`task_scheduler_test.rs`](./task_scheduler_test.rs) 及 Go 对应测试覆盖 LIFO 和错误短路。未运行 Cargo，符合本任务的纯文档限制。
