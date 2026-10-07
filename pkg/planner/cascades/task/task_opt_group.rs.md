# `pkg/planner/cascades/task/task_opt_group.rs`

[源文件：`task_opt_group.rs`](./task_opt_group.rs)

## 文件定位

本文件属于 Cargo 包 `astersql-planner-cascades-task`，包入口是同目录的 `lib.rs`，后者通过 `mod task_opt_group` 装配并通过 `pub use task_opt_group::*` 导出本文件 API。它位于 Cascades 优化器的 Memo 探索链上：`pkg/planner/cascades/cascades.rs` 的 `Optimizer::NewOptimizer` 初始化根 `Group` 后创建 `NewOptGroupTask`，`SimpleTaskScheduler::ExecuteTasks` 再从 LIFO 任务栈弹出并执行它。

文件只负责“展开一个等价组”的调度步骤，不负责模式匹配、规则变换或把新表达式写回 Memo；这些职责分别落在 `task_opt_group_expression.rs`、`task_apply_rule.rs` 和 `Context` 接口中。目标目录不存在 `doc.go`；包边界以 `Cargo.toml`、`lib.rs` 和相邻实现为准。

## 核心职责

- 用 `OptGroupTask` 同时携带共享任务上下文 `BaseTask` 和待探索的 `GroupRef`。
- 在 `Execute` 中以 `Group::IsExplored` 做组级幂等保护；已标记的组不再重复派生任务。
- 对当前组内逻辑表达式句柄做快照，并为每个句柄压入一个 `OptGroupExpressionTask`，把后续规则选择和子组递归交给表达式级任务。
- 在派生任务全部入栈后调用 `Group::SetExplored`，表示这个组的现有表达式已经完成调度，而不是表示所有派生任务已经执行完毕。
- 通过 `Desc` 输出稳定的调试描述 `OptGroupTask{group:GID:<id>}`。

这个文件是调度编排层，而不是优化算法本体。真正的规则筛选、Binder 遍历、`XForm`、`CopyInWithChildren` 和新表达式继续调度由相邻任务实现。

## 主要符号

### `pub struct OptGroupTask`

- `BaseTask: BaseTask`：持有 `ContextRef`。`BaseTask::Push` 最终调用 `Context::PushTask`，只入栈、不内联执行。
- `group: GroupRef`：目标 Memo 等价组。`GroupRef` 是共享、内部可变的组句柄；本文件分别通过不可变和可变借用读取表达式/状态并写入 explored 标志。

两个字段均为 `pub`，但常规构造路径是 `NewOptGroupTask`。字段和函数沿用 Go 风格命名，crate 根的 `#![allow(non_snake_case)]` 明确允许这种移植接口。

### `pub fn NewOptGroupTask(ctx: ContextRef, group: GroupRef) -> Box<dyn Task>`

构造 `BaseTask::New(ctx)`，保存 `group`，并把具体类型装箱为 `Box<dyn Task>`，使调用者可直接交给统一调度器。它不检查空组、explored 状态或上下文有效性，这些行为推迟到任务执行阶段。

### `impl Task for OptGroupTask`

- `Execute(&mut self) -> Result<(), Box<dyn Error>>`：本文件的行为入口。当前实现自身没有可恢复失败分支，因此正常路径均返回 `Ok(())`；返回类型来自统一 `Task` 契约。
- `Desc(&self, writer: &mut dyn StrBufferWriter)`：把任务类型、组描述和闭合括号写给调用方提供的 writer，不持有 writer，也不主动 `Flush`。

## 执行流程

1. `Optimizer::NewOptimizer` 在 Memo 中初始化逻辑计划，取得根表达式所属的 `GroupRef`，再把 `NewOptGroupTask(context, root_group)` 压入调度上下文。
2. `SimpleTaskScheduler::ExecuteTasks` 从栈顶取得任务并调用 `Task::Execute`。
3. `OptGroupTask::Execute` 先读取 `group.IsExplored()`。若为真，立即返回 `Ok(())`，避免循环引用或多个父表达式反复调度同一组。
4. 若尚未探索，调用 `GetLogicalExpressions()`。该方法克隆并返回 `Vec<GroupExpressionRef>`，因此循环遍历的是当下快照，而不是会被后续 `CopyIn`/`RemoveOut` 改动的容器游标。
5. 对快照中的每个表达式，按原向量顺序调用 `BaseTask::Push(NewOptGroupExpressionTask(...))`。因为调度器是 LIFO，同批表达式实际按逆序开始执行；本文件不承诺表达式间的正序执行。
6. 所有表达式级任务入栈后，立即调用 `SetExplored()` 并返回。标志写入早于派生任务实际执行。
7. 表达式级任务随后筛选规则、压入 `ApplyRuleTask`，并按逆序压入子 Group 的 `OptGroupTask`。LIFO 使第 0 个子组先执行，而且子组探索先于该表达式的规则任务。规则产生的新表达式由 `ApplyRuleTask` 直接压入新的 `OptGroupExpressionTask`，不依赖再次运行本组任务。

## 数据与状态

核心持久状态在 Memo 的 `Group` 中：逻辑表达式列表和布尔值 `explored`。`GetLogicalExpressions` 克隆的是 `Rc` 句柄集合，不深拷贝表达式；任务仍与 Memo 共享同一表达式对象。`SetExplored` 是单向地把布尔值置为 `true`，本文件没有复位路径。

`OptGroupTask` 本身没有计数器、缓存或所有权转移。`ContextRef` 和 `GroupRef` 让任务与优化器共享上下文和 Memo 节点；任务出栈销毁只减少句柄引用计数，不销毁仍由 Memo/其他任务持有的数据。

必须区分两个粒度的状态：本文件写的是 `Group::explored`；`ApplyRuleTask` 写的是每个 `GroupExpression` 对具体 rule ID 的 explored 位。组级标志阻止重复枚举整个组，表达式/规则级标志阻止同一规则重复应用。

## 依赖与调用关系

上游直接入口：

- `pkg/planner/cascades/cascades.rs::Optimizer::NewOptimizer`：创建根组任务，是完整应用主链入口。
- `pkg/planner/cascades/task/task_opt_group_expression.rs::OptGroupExpressionTask::Execute`：为每个输入子组递归创建组任务。
- `pkg/planner/cascades/task/task_test.rs::task_chain_uses_real_memo_and_rule_contracts`：测试上下文直接创建根组任务并手动执行 LIFO 栈。

下游直接依赖：

- `BaseTask::New` 与 `BaseTask::Push`：保存上下文并把派生任务交给 `Context::PushTask`。
- `Group::{IsExplored, GetLogicalExpressions, SetExplored, String}`：读取幂等状态、取得快照、提交组级状态并生成描述。
- `NewOptGroupExpressionTask`：为每个已有表达式建立下一层工作单元。
- `Task` 与 `StrBufferWriter`：分别定义执行/描述契约。

`Cargo.toml` 表明本 crate 直接依赖 `cascades-base` 和 `cascades-memo`；本文件使用的任务 trait/writer 来自前者，`GroupRef` 来自后者。`NewOptGroupExpressionTask` 和上下文类型通过本 crate 根再导入。Cargo 元数据将对应 Go 包声明为 `pkg/planner/cascades/task`，且没有为本行为声明 feature 条件；本文件也没有 `cfg` 分支。

## 错误处理与边界

`Execute` 不产生业务错误，也不吞掉下游错误，因为这里只创建任务、尚未执行它们。派生任务的错误会在以后由 `SimpleTaskScheduler::ExecuteTasks` 的 `task.Execute()?` 向上传播，并使调度循环短路。

边界行为如下：

- 已探索组：不压入任何任务，直接成功。
- 未探索但表达式快照为空的组：不压入任务，但仍标记 explored 并成功。
- 调度过程中新增表达式：不在初始快照里；规则路径必须像 `ApplyRuleTask` 一样为新表达式显式创建 `OptGroupExpressionTask`。
- 动态借用冲突：`RefCell` 会 panic，而不是返回 `Result`。当前实现让 `IsExplored`、快照获取和 `SetExplored` 的借用都只活在各自语句内，并在调用 `Push` 前释放组借用，避免持有组借用跨上下文回调。
- `Desc` 假定 writer 可写；writer 契约不提供错误返回，因此写入失败不能由本函数报告。

## 并发与资源生命周期

`ContextRef`/`GroupRef` 基于 `Rc<RefCell<_>>`，明确是单线程共享模型，不是 `Arc<Mutex<_>>`；`SimpleTaskScheduler` 也是串行 LIFO 调度器。因此本文件没有跨线程同步、锁或异步任务。运行时互斥由 `RefCell` 动态借用规则保证，违规将 panic。

生命周期顺序是：构造任务并克隆两个共享句柄；任务被压栈；执行时再为每个表达式克隆上下文和表达式句柄；本组任务返回并被释放；派生任务继续持有所需引用。任务栈由调度器拥有，`Destroy` 清空后归还线程本地栈池；本文件不直接管理该资源。

组级 explored 标志是递归终止和重复调度抑制机制，但不是并发完成屏障。它在子任务执行前置位，因此扩展代码不得把 `IsExplored()` 解释为“所有规则已成功执行”。如果后续派生任务报错，标志不会由本文件回滚。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/task/task_opt_group.go`。Rust 保留了 Go 的类型、构造器、`Execute`/`Desc` 名称和主要顺序：检查 `IsExplored`，遍历组表达式并压表达式级任务，调用 `SetExplored`，最后返回成功；描述文本也一致。

主要表示差异：

- Go 用 `*memo.Group` 和接口 `base.Task`，Rust 用 `GroupRef` 与 `Box<dyn Task>`。
- Go 把 `BaseTask` 匿名嵌入并把 `group` 设为包内字段；Rust 使用命名字段 `BaseTask`、`group`，当前均公开。
- Go 的 `ForEachGE` 遍历链表；Rust 的 `GetLogicalExpressions` 克隆句柄向量后遍历。两者当前都为每个当时可见表达式压任务，但 Rust 明确提供对遍历期间容器变更更稳健的快照语义。
- Go 的 `Group.String` 直接写入 writer；Rust 的 `Group::String` 返回 `String`，`Desc` 再写入。最终格式保持一致。
- Go 返回 `nil`，Rust 返回 `Ok(())`；本文件均无主动错误分支。

Go 的相邻 `task_opt_group_expression.go` 同样逆序压入输入子组，以满足“子组先探索、然后应用当前表达式规则”的 LIFO 前置条件。Rust 对这一调度语义保持一致。

## 扩展指南

若扩展组级探索，优先判断职责应放在哪一层：组枚举/幂等逻辑放在 `OptGroupTask::Execute`；单表达式规则选择放在 `OptGroupExpressionTask`；规则变换和新表达式接线放在 `ApplyRuleTask`；全局入队与 Memo 变更放在 `Context` 实现。

修改时重点维护以下不变量：

1. 在任何可能回调 `Context` 的操作前释放 `GroupRef` 的 `Ref`/`RefMut`，避免 `RefCell` 重入 panic。
2. 若改变压栈顺序，按 `SimpleTaskScheduler` 的 LIFO 语义推导真实执行顺序；不要把源码循环顺序直接当作运行顺序。
3. 若改变 explored 的置位时机，评估循环 Memo、重复父引用、失败回滚和新表达式调度；它既影响终止性，也影响错误后的可重试性。
4. 若不再使用快照遍历，必须证明规则写回或删除表达式不会使遍历失效或漏调度。
5. 新增可失败操作时，让错误通过 `Task::Execute` 返回，避免把“已经 explored”留在与实际调度进度不一致的状态。

测试应继续放在独立文件 `pkg/planner/cascades/task/task_test.rs`，不要内嵌到生产源文件。最直接的新增用例包括：已 explored 时不入栈、空组仍安全完成、多表达式的 LIFO 顺序、派生任务失败后组标志语义，以及规则新增表达式仍被显式调度。对应 Go 行为变更时同步检查 `task_opt_group.go` 及相关 Go 测试；若只是 Rust 特有借用/生命周期回归，应在独立 Rust 测试中说明差异缘由。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；目标目录的 `files --filter` 结果识别出 Rust/Go 成对实现与独立测试文件。
- RustCodeGraph `query OptGroupTask --kind struct`、`query NewOptGroupTask --kind function` 和路径限定 `node`：确认 Rust/Go 符号、签名与目标源码位置。
- RustCodeGraph 路径读取：`task_test.rs` 第 272 行起的 `task_chain_uses_real_memo_and_rule_contracts`，`memo/group.rs` 中 `GetLogicalExpressions`、`IsExplored`、`SetExplored`，以及 `cascades.rs::Optimizer::NewOptimizer`/`Execute`。
- RustCodeGraph 的通用 `Execute` 调用边查询出现同名消歧噪声，未把其跨包结果作为依据；上游和下游关系由路径限定源码与 `rg` 引用结果交叉核对。
- 读取的 Rust 直接证据：`task_opt_group.rs`、`lib.rs`、`base.rs`、`task.rs`、`task_scheduler.rs`、`task_opt_group_expression.rs`、`task_apply_rule.rs`、`task_test.rs`、`task_scheduler_test.rs`、`memo/group.rs`、`cascades.rs`。
- 读取的 Cargo/Go 直接证据：`pkg/planner/cascades/task/Cargo.toml`、`task_opt_group.go`、`task_opt_group_expression.go`、`task_test.go`、`task_scheduler_test.go`、`pkg/planner/cascades/memo/group.go`、`pkg/planner/cascades/cascades.go`。
- 相关独立 Rust 测试 `task_chain_uses_real_memo_and_rule_contracts` 断言任务链结束后组已 explored、组内因规则变换共有两个逻辑表达式、原表达式已记录 rule 0；`TestSimpleTaskScheduler` 证明调度为 LIFO 且错误会短路。按任务约束，本次纯文档分析未运行 Cargo，也未执行测试二进制。
