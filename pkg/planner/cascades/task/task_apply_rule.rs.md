# `pkg/planner/cascades/task/task_apply_rule.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-task` crate；crate 入口 `pkg/planner/cascades/task/lib.rs` 将本模块声明为 `task_apply_rule` 并公开再导出其符号。它位于 Cascades 逻辑优化的任务链中段：`OptGroupExpressionTask::Execute` 为一个 `GroupExpression` 选择启用且 operand 匹配的规则，构造 `NewApplyRuleTask`；`ApplyRuleTask::Execute` 枚举规则模式的绑定、执行变换并把结果写回 Memo；新增的组表达式再交给 `NewOptGroupExpressionTask` 继续探索。

该文件只负责“一条规则作用于一个组表达式”的调度步骤，不负责规则选择、模式匹配算法、Memo 去重/合并的具体实现，也不负责最终物理计划选择。直接证据见 `pkg/planner/cascades/task/task_opt_group_expression.rs`、`pkg/planner/cascades/rule/binder.rs` 和 `pkg/planner/cascades/memo/memo.rs`。

## 核心职责

1. 以 `RuleRef = Rc<dyn Rule>` 在任务和优化上下文之间共享规则对象，并用 `ApplyRuleTask` 保存上下文、目标 `GroupExpressionRef` 与规则。
2. 在执行前用 `GroupExpression::IsExplored(rule_id)` 和 `IsAbandoned()` 去掉重复或失效任务。
3. 用 `NewBinder(rule.Pattern().clone(), gE)` 对目标表达式及其 Memo 子组枚举全部结构绑定；对每个绑定依次执行 `Rule::PreCheck` 和 `Rule::XForm`。
4. 把每个变换结果通过 `Context::CopyInWithChildren` 写入原表达式所属的等价 Group，保留 `BoundPlan::ChildGroups` 提供的 Memo 子组连接，并为返回的表达式压入新的 `OptGroupExpressionTask`。
5. 按规则返回的 `remove` 标志调用 `Context::RemoveOut` 删除原表达式；所有绑定成功处理后，以规则 ID 标记本表达式已探索。
6. 实现 `Task::Desc`，并用私有 `RuleWriter` 适配两个 crate 中同名但不同类型的字符串 writer trait。

## 主要符号

- `RuleRef = Rc<dyn Rule>`：规则 trait 对象的单线程共享句柄。`Rc` 与本 crate 的 `ContextRef = Rc<RefCell<dyn Context>>`、Memo 引用模型一致。
- `ApplyRuleTask { BaseTask, gE, rule }`：可压入调度栈的任务。三个字段当前均为公开字段；`BaseTask` 提供共享 `ContextRef` 和 `Push`，`gE` 是被改写的组表达式，`rule` 是本次唯一应用的规则。
- `NewApplyRuleTask(ctx, expression, rule) -> Box<dyn Task>`：公开构造入口，隐藏具体任务类型的返回使用方式，使调度栈统一存储 `Box<dyn Task>`。
- `impl Task for ApplyRuleTask::Execute`：文件的行为核心。返回 `Result<(), Box<dyn Error>>`，与通用任务调度器的失败短路契约一致。
- `impl Task for ApplyRuleTask::Desc`：输出 `ApplyRuleTask{gE:<表达式>, rule:<规则>}`，用于任务栈诊断。
- `RuleWriter<'a>(&'a mut dyn cascades_base::util::StrBufferWriter)`：私有借用适配器；其 `WriteString` 和 `Flush` 原样转发给调用方 writer，使 `Rule::String` 所要求的 `cascades_util::StrBufferWriter` 能写入任务描述缓冲区。

本文件没有模块级常量、条件编译项或固有 `impl ApplyRuleTask`；公开行为全部经构造函数和 `Task` trait 暴露。

## 执行流程

1. `Execute` 先读取 `rule.ID()`。若目标表达式已经记录该 ID，或已经被 Memo 标记为 abandoned，立即成功返回，不创建 Binder、不改 Memo，也不调度后续任务。
2. 克隆规则的 `Pattern`，调用 `NewBinder`。Binder 在构造阶段通过 `dfsMatch` 对根表达式和子 Group 的逻辑表达式做深度优先匹配，并预先保存所有 `BoundPlan`；子模式组合可能产生多个绑定，`OperandAny` 在一个 Group 内只取首个候选以抑制组合爆炸。
3. 循环调用 `binder.Next()`。`PreCheck` 为假时只跳过当前绑定；本文件不显式调用 `Rule::Match`，因此是否需要额外语义匹配必须由具体规则的 `PreCheck`/`XForm` 契约或 Binder 外层设计保证，不能在文档中假定 `Match` 已执行。
4. `XForm(&holder)` 返回 `(new_expressions, remove)`。错误立即转换为 `TaskError` 并向调度器传播；此时不执行函数末尾的 `SetExplored`。
5. 在处理变换输出前取得 `holder.ChildGroups()`。对每个新逻辑计划，重新取得原 `gE` 的 owning Group，调用 `CopyInWithChildren(target, expression, child_groups.clone())`。这个接口允许只返回浅层逻辑算子的规则复用已经绑定的 Memo 子 Group；实际 Memo 会负责插入、去重以及必要的 Group 合并。
6. 每次写入成功后立即调用 `BaseTask::Push(NewOptGroupExpressionTask(...))`。`Push` 只入栈、不内联执行；`SimpleTaskScheduler` 是 LIFO，因此同一绑定产生多个表达式时，最后压入的表达式任务先运行。
7. 若当前绑定的 `remove` 为真，则从 owning Group 移除原 `gE`。Binder 的匹配列表已在构造时生成，循环仍按该快照继续；后续具体规则不应依赖“remove 后重新计算绑定”。
8. Binder 耗尽后调用 `gE.SetExplored(rule_id)`。即使所有绑定都被 `PreCheck` 跳过或 Binder 没有匹配，只要过程无错误，规则仍被视为已经探索，避免重复调度。

## 数据与状态

- `gE` 是 `Rc<RefCell<GroupExpression>>` 形式的共享引用。其内部 `mask` 按规则 ID 记录探索状态，`abandoned` 表示表达式已从 Memo 删除；定义和访问器位于 `pkg/planner/cascades/memo/group_expr.rs`。
- `rule` 是不可变 trait 对象共享引用。任务只读取 `ID`、`Pattern`，并调用 `PreCheck`、`XForm`、`String`；规则若需要内部可变性，必须自行实现安全的内部状态管理。
- `holder: BoundPlan` 是 Binder 返回的一次匹配快照，包含根表达式及按 Pattern 对齐的孩子绑定。`ChildGroups()` 从每个孩子表达式取得 owning Group，作为 Rust 侧保持 Memo 图连接的显式数据。
- 新表达式的所有权交给 `Context::CopyInWithChildren`，返回值是 Memo 管理的 `GroupExpressionRef`；任务自身不缓存这些结果，只把引用转交给后续任务。
- `SetExplored` 是成功完成标志而不是启动标志：任何 `XForm` 或 CopyIn 错误都会在标记前返回，所以同一规则可由上层在失败处理后重新尝试。相反，已经成功写入并压栈的早期结果不会因后续结果失败而回滚，本函数不提供事务性批量提交。
- `remove` 对每个绑定单独生效。`RemoveOut` 会删除 Group 边、清理 Memo 全局表达式表和父子关系，并将原表达式标为 abandoned；其具体状态转换见 `Memo::RemoveOut`。

## 依赖与调用关系

上游主链为：

`Optimizer::NewOptimizer` → `NewOptGroupTask` → `NewOptGroupExpressionTask` → `NewApplyRuleTask` → `ApplyRuleTask::Execute`。

其中 `OptGroupExpressionTask::getValidRules` 先按逻辑算子的 `Operand` 查询 `Context::RulesFor`，再检查 Pattern 根 operand 和 `RuleEnabled(rule.ID())`；因此本文件收到的通常已经是启用且根类型适用的规则，但 `Execute` 仍需 Binder 完成整棵模式绑定。

主要下游为：

- `cascades-rule`：`Rule`、`NewBinder`、`BoundPlan`，提供规则契约和绑定枚举。
- `cascades-memo`（经本 crate 的 `GroupExpressionRef` 及 `Context` 间接访问）：保存 owning Group、探索 mask、abandoned 状态和变换结果。
- `crate::Context`：抽象 `CopyInWithChildren`、`RemoveOut` 和 `PushTask`；生产实现位于 `pkg/planner/cascades/cascades.rs`，测试实现位于 `pkg/planner/cascades/task/task_test.rs`。
- `NewOptGroupExpressionTask`：接续探索每个变换产物。
- `cascades-base` 与 `cascades-util`：分别定义任务描述 writer 和规则描述 writer；`RuleWriter` 消除两者接口边界。

`pkg/planner/cascades/task/Cargo.toml` 声明本 crate 直接依赖 `cascades-base`、`cascades-memo`、`cascades-pattern`、`cascades-rule`、`cascades-util` 和 `logicalop`，没有 feature 条件。目标文件直接使用其中的 base、rule、util；Memo 和 logicalop 类型通过本 crate 的上下文与再导出进入调用链。

## 错误处理与边界

- `Rule::XForm` 的 `RuleError` 被转为仅保留 `to_string()` 的 `TaskError`，再装箱传播；原始具体错误类型和错误链不会保留。
- `CopyInWithChildren` 的 `TaskError` 通过 `?` 原样传播。`SimpleTaskScheduler::ExecuteTasks` 遇到任一任务错误立即停止，因此后续已入栈任务暂不执行。
- 两处 `GetGroup().expect("applied group expression must belong to a group")` 把“被应用的表达式必须已属于 Memo Group”作为硬不变量；构造调用者若传入游离表达式会 panic，而不是返回可恢复错误。
- `BoundPlan::ChildGroups` 同样要求每个绑定孩子属于 Group，否则 panic。正常 Binder 输入来自 Memo，这一条件由调用链保证。
- `PreCheck == false` 是普通过滤，不是错误；空输出、无绑定也都是成功路径，并最终设置 explored。
- `remove == true` 没有返回错误。本函数允许规则同时返回新表达式并移除旧表达式，顺序固定为先 CopyIn/压任务、后 RemoveOut。
- 本文件不验证规则 ID 的范围或唯一性；探索 mask 直接以 `usize` ID 为键，ID 契约由规则注册层负责。
- 当前直接任务测试覆盖成功路径；未见专门验证 `PreCheck == false`、`XForm` 失败、CopyIn 失败、`remove == true` 或缺失 owning Group panic 的 `ApplyRuleTask` 独立测试，因此这些分支的说明来自实现和 Memo 测试，而非该任务的专门回归用例。

## 并发与资源生命周期

本任务模型是单线程、串行、栈式的。`RuleRef` 使用 `Rc`，`ContextRef` 使用 `Rc<RefCell<_>>`，`GroupExpressionRef` 也由 Memo 的 `Rc<RefCell<_>>` 图管理；这些类型没有跨线程 `Send/Sync` 保证。`SimpleTaskScheduler` 在当前线程循环弹出 `Box<dyn Task>`，任务执行期间新任务被压回同一 LIFO 栈。

`Execute` 将 `RefCell` 借用限制在短表达式内：检查状态、取得 Group、修改 Context、设置 explored 分步进行，避免在 `CopyInWithChildren` 或 `RemoveOut` 期间仍持有 `gE` 的长借用。违反动态借用规则会 panic，因此扩展时不能把 `borrow()` 结果跨上下文可变调用保存。

任务持有 `Rc` 会延长 Context、GroupExpression 和 Rule 的生命周期，直到任务执行完并从栈中释放。新任务各自克隆 Context 和表达式引用，不复制底层 Memo 节点。`RuleWriter` 仅在 `Desc` 调用期间借用外部 writer，不保存到调用结束之后。任务没有锁、异步任务、通道、文件句柄或显式事务；Memo 更新也没有回滚守卫。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/task/task_apply_rule.go`。两版保持了相同主语义：先检查 `IsExplored(rule.ID())`/`IsAbandoned`，再以规则 Pattern 创建 Binder；每个 holder 依次经过 `PreCheck`、`XForm`；逐个 CopyIn 并调度 `NewOptGroupExpressionTask`；按 `remove` 删除旧表达式；循环结束后 `SetExplored`；`Desc` 输出同样的字段顺序。

Rust 版的必要类型适配包括：

- Go 返回 `*ApplyRuleTask`，Rust 返回 `Box<dyn Task>`，直接满足异构任务栈的所有权要求。
- Go 通过接口/指针共享 Context、GroupExpression 和 Rule；Rust 分别使用 `Rc<RefCell<dyn Context>>`、`GroupExpressionRef` 和 `Rc<dyn Rule>`。
- Go 的 `binder.Next()` 以 `nil` 表示耗尽；Rust 返回 `Option<BoundPlan>`。
- Go 直接 `GetMemo().CopyIn(a.gE.GetGroup(), ne)`。Rust 调用 `CopyInWithChildren` 并传入 `holder.ChildGroups()`，这是 Rust `BoundPlan` 显式表示 Memo 子树、浅层规则输出需要恢复子 Group 连接所必需的局部接线；若计划本身已有 children 或没有绑定孩子，Memo 实现会退回普通 `CopyIn`。
- Go 原样返回 `XForm` 错误；Rust 将其文本包装为 `TaskError`。CopyIn 错误在两版都短路返回。
- Go 的同一 `util.StrBufferWriter` 可直接传给 `rule.String`；Rust 的 task/base writer 与 rule/util writer 来自不同 crate，故增加私有 `RuleWriter` 转发层。

未发现 Rust 版删减 Go 主循环或把实际变换替换为桩；差异集中在 Rust 所有权、错误类型和跨 crate trait 适配。

## 扩展指南

- 新增规则通常不应修改本文件：实现 `cascades_rule::Rule`，提供稳定且唯一的 `ID`、正确的 `Pattern`、必要的 `PreCheck` 和 `XForm`，再通过 `Context::RegisterRules` 接入 operand 对应规则表。同步增加独立规则测试，并在 `pkg/planner/cascades/task/task_test.rs` 或更上层 `cascades_test.rs` 覆盖真实任务链。
- 若改变规则输出的子树表达方式，重点审查 `BoundPlan::ChildGroups`、`Context::CopyInWithChildren` 和 `Memo::CopyInWithGroupChildren`。错误地丢弃孩子 Group 会让浅层逻辑算子失去 Memo 输入；错误复用则可能破坏等价类或触发 Group 合并。
- 若改变删除语义，需同时验证“先插入后删除”的顺序、Binder 预计算快照、Memo 父边清理、全局表达式索引和 abandoned 标记；相关现有测试是 `pkg/planner/cascades/memo/memo_test.rs::TestInsertGE`，任务侧应另加 `remove == true` 回归测试。
- 若要增强失败原子性，不能只调整错误映射；必须明确已 CopyIn 的表达式和已压栈任务如何回滚，并评估 Context trait、Memo 与调度栈的共同事务边界。
- 若增加并行执行，需整体替换 `Rc<RefCell<_>>` 与线程本地/LIFO 假设，审查 Rule trait 对象、Memo 图和任务栈的同步策略；不能仅给 `ApplyRuleTask` 添加 `Send`。
- 修改诊断格式时同步维护 `RuleWriter` 适配和任务栈描述测试。Rust 单元测试应继续放在独立的 `task_test.rs`/`task_scheduler_test.rs`，不要内嵌到本生产文件。
- 性能风险主要来自 Binder 的绑定组合数、每个输出的 Memo 去重/合并，以及对每个新表达式继续入栈；新增规则应避免过宽 Pattern 和无界输出，并用多子组、多等价表达式场景测量任务数量。

## 验证依据

- 目标源码：`pkg/planner/cascades/task/task_apply_rule.rs`。核对了 `RuleRef`、`ApplyRuleTask`、`NewApplyRuleTask`、`Task::Execute`、`Task::Desc`、`RuleWriter::{WriteString, Flush}` 的完整定义；无条件编译项。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`query ApplyRuleTask`、`query NewApplyRuleTask --kind function`、`query RuleWriter` 确认 Rust 与 Go 对照符号；`explore` 返回目标文件完整源码。由于 `Execute`/`Desc` 是常见名称，图的 callers/callees 查询出现跨仓库同名歧义，调用关系改由精确引用搜索和相邻源码核验，不把歧义结果当作证据。
- crate 与装配：`pkg/planner/cascades/task/Cargo.toml`、`pkg/planner/cascades/task/lib.rs`。
- 上下游实现：`pkg/planner/cascades/task/task_opt_group_expression.rs`、`base.rs`、`task_scheduler.rs`、`task.rs`，`pkg/planner/cascades/rule/rule.rs`、`binder.rs`，`pkg/planner/cascades/memo/group_expr.rs`、`memo.rs`，`pkg/planner/cascades/cascades.rs`。
- Go 对照：`pkg/planner/cascades/task/task_apply_rule.go` 的 `ApplyRuleTask`、`NewApplyRuleTask`、`Execute` 和 `Desc`。
- 独立 Rust 测试：`pkg/planner/cascades/task/task_test.rs::task_chain_uses_real_memo_and_rule_contracts` 验证真实 Memo、Binder、规则变换、CopyIn、继续调度以及 explored 状态；`pkg/planner/cascades/cascades_test.rs::test_registered_rule_is_explored` 验证注册规则经完整优化入口被探索；`pkg/planner/cascades/memo/memo_test.rs::TestInsertGE` 验证去重、Group 合并以及 RemoveOut 后 abandoned/索引清理。未运行 Cargo，符合本计划的纯文档范围。
