# `pkg/planner/cascades/task/task_opt_group_expression.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-task` crate；crate 入口
[`lib.rs`](lib.rs) 将该模块声明为私有模块，再通过
`pub use task_opt_group_expression::*` 导出其公开符号。它位于 Cascades
优化器的任务调度层：上游的 `OptGroupTask` 把一个 Memo `Group` 拆成多个
`GroupExpression` 任务，本文件再为单个表达式安排子 Group 探索与规则应用。

应用主链可由
[`cascades.rs`](../cascades.rs) 的 `Optimizer::NewOptimizer` 和
`Optimizer::Execute` 复核：根 Group 先进入 `NewOptGroupTask`，随后产生本文件的
`NewOptGroupExpressionTask`，最终由串行 LIFO 调度器逐项执行。规则产生新表达式时，
[`task_apply_rule.rs`](task_apply_rule.rs) 还会再次构造表达式级任务，使搜索继续扩展。

## 核心职责

`OptGroupExpressionTask` 只负责“编排”，不在本文件内执行规则变换或修改 Memo：

1. 从表达式根算子的 `Operand` 出发，向共享 `Context` 取得候选规则；
2. 保留根 `Operand` 相同且当前规则掩码启用的规则，并为其压入
   `ApplyRuleTask`；
3. 对表达式的子 Group 逆序压入 `OptGroupTask`，借助 LIFO 保证第 0 个子
   Group 最先完整探索；
4. 提供稳定的任务描述文本，供调试和任务栈展示使用。

这里的关键顺序不变量是：规则任务先入栈、子 Group 任务后入栈，因此真正弹栈时
所有子 Group 位于规则任务之上；这满足 Go 实现注释所述“应用当前表达式规则前，
子 Group 已完成探索”的前置条件。

## 主要符号

- `pub struct OptGroupExpressionTask`：一个表达式级工作单元，包含共享任务基类
  `BaseTask` 和目标 `GroupExpressionRef`。两个字段当前均为 `pub`。
- `pub fn NewOptGroupExpressionTask(ctx, expression) -> Box<dyn Task>`：公开构造入口。
  它用 `BaseTask::New` 保存上下文，以 trait object 返回任务所有权，供调度栈动态分发。
- `fn getValidRules(&self) -> Vec<crate::RuleRef>`：私有候选规则过滤器。它通过
  `GetOperand(GetWrappedLogicalPlan())` 取得根算子类别，经 `Context::RulesFor`
  获取该类别规则，再检查 `rule.Pattern().Operand == operand` 和
  `Context::RuleEnabled(rule.ID())`。
- `Task::Execute`：核心调度入口，只压入派生任务并返回 `Ok(())`。
- `Task::Desc`：写出 `OptGroupExpressionTask{ge:<表达式字符串>}`；不刷新 writer，
  刷新职责留给调用方。

本文件没有模块级常量、枚举、条件编译项或独立错误类型。

## 执行流程

1. `Execute` 调用 `getValidRules`。后者短暂只读借用目标表达式以读取逻辑计划，
   计算根 `Operand`，再从 `BaseTask.ctx` 查询规则。
2. 对过滤后的每条规则，`Execute` 克隆上下文、表达式 `Rc` 句柄和规则 `Rc`
   句柄，调用 `NewApplyRuleTask`，再经 `BaseTask::Push` 压入共享调度栈。
3. `Execute` 克隆 `groupExpression.Inputs`，结束对表达式的借用；随后对快照执行
   `rev()`，为每个子 Group 构造并压入 `NewOptGroupTask`。
4. LIFO 调度器先弹出原始索引 0 的子 Group，再依次处理其余子 Group；全部子任务
   离栈后才轮到规则任务。若候选规则按 `[r0, r1]` 入栈，则规则阶段的弹出顺序是
   `r1, r0`，这是栈语义的直接结果。
5. 当前任务本身不等待也不内联执行派生任务，压栈完成即返回 `Ok(())`。

`Desc` 是旁路流程：读取 `GroupExpression::String()`，依次写前缀、表达式文本和
右花括号，不参与优化状态迁移。

## 数据与状态

- `ContextRef` 是 `Rc<RefCell<dyn Context>>`；多个任务共享规则表、规则掩码、Memo
  接口和调度入口。`BaseTask::Push` 对该上下文做可变借用并调用 `PushTask`。
- `GroupExpressionRef` 是 `Rc<RefCell<GroupExpression>>`。本文件只读取其包装逻辑
  计划、`Inputs` 和字符串表示，不修改表达式的 explored mask、abandoned 标志或
  所属 Group。
- `Inputs` 的克隆只复制 `Rc` 子 Group 句柄，不复制 Group 内容。该快照避免在后续
  任务可能改动 Memo 时仍持有表达式借用，也固定了本次调度看到的子输入集合。
- `RuleRef` 是 `Rc<dyn Rule>`；规则对象在 Context 与多个任务间共享，不在此处复制
  规则内部状态。
- explored/abandoned 状态由 `GroupExpression` 保存，但在
  [`task_apply_rule.rs`](task_apply_rule.rs) 中检查和更新，而非本文件处理。

## 依赖与调用关系

上游调用者有两类：

- [`task_opt_group.rs`](task_opt_group.rs) 的 `OptGroupTask::Execute` 为 Group 中每个
  逻辑表达式创建本任务；这是从根 Memo 开始的常规入口。
- [`task_apply_rule.rs`](task_apply_rule.rs) 的 `ApplyRuleTask::Execute` 将规则输出
  `CopyInWithChildren` 到 Memo 后，为新 `GroupExpression` 再创建本任务；这是搜索
  扩展形成的反馈边。

直接下游为 `GetOperand`、`Context::RulesFor`、`Context::RuleEnabled`、
`NewApplyRuleTask`、`NewOptGroupTask`、`BaseTask::Push` 与
`GroupExpression::String`。`Cargo.toml` 说明这些接口分别来自本 crate、
`cascades-base`、`cascades-memo`、`cascades-pattern` 和 `cascades-rule`；本文件未直接
使用 crate 还声明的 `cascades-util` 与 `logicalop`。

RustCodeGraph 对目标文件识别出 6 个符号，并给出 `getValidRules` 由 `Execute`
调用的边。当前索引对跨文件 callers/callees 的精确命令未返回结果，因此上述跨文件
边又由对应构造函数调用点的局部源码搜索核验。

## 错误处理与边界

- `Execute` 的签名允许返回动态错误，但本实现没有可失败操作，正常路径恒为
  `Ok(())`；真正的规则绑定、`XForm`、Memo `CopyIn` 错误由后续
  `ApplyRuleTask::Execute` 产生并由调度器短路传播。
- `RulesFor` 返回空列表时不会压入规则任务；`Inputs` 为空时不会压入子 Group
  任务，两者都不是错误。
- 过滤条件只检查规则根 `Operand` 与启用掩码，不在这里完成整棵模式绑定，也不检查
  `GroupExpression::IsExplored` 或 `IsAbandoned`。完整绑定和 explored/abandoned
  防重逻辑位于 `ApplyRuleTask`，因此不能把 `getValidRules` 描述为完整匹配成功。
- `Rc<RefCell<_>>` 的动态借用冲突会 panic，而不是转换成 `Result`；当前实现通过先
  克隆 `Inputs`、再逐项压栈来缩短表达式借用，但要求调用者不要在持有同一 Context
  可变借用时重入 `Execute`/`Push`。
- `Desc` 依赖表达式仍可被只读借用；它只写缓冲区，不做外部 I/O，也不调用
  `Flush`。

## 并发与资源生命周期

该链路是单线程、非 `Send`/`Sync` 的：共享状态使用 `Rc<RefCell<_>>`，调度器使用
串行 `Vec<Box<dyn Task>>` 栈。本文件不创建线程、异步任务、通道、锁或事务。

构造函数把上下文和表达式的强引用放入装箱任务；任务在 LIFO 栈中拥有这些句柄，
执行并弹出后随 `Box` 释放。派生任务通过克隆 `Rc` 延长相关 Context、GroupExpression、
Group 和 Rule 的生命周期。Memo 表达式对所属 Group 使用弱回指以避免所有权环，
而表达式对子 Group 的 `Inputs` 是强引用。调度栈最终由 scheduler 的 `Destroy`
清空并归还线程本地栈池，这一资源回收发生在相邻模块而非本文件。

## 与 Go 版本的对应关系

直接对照文件是
[`task_opt_group_expression.go`](task_opt_group_expression.go)。结构和调度次序基本一一
对应：两边都有 `BaseTask`、目标 GroupExpression、构造函数、`Execute`、`Desc` 和
私有规则过滤器；两边都先压规则任务，再从最后一个输入向第一个输入压子 Group
任务，以便 LIFO 先执行第 0 个子 Group。

需要明确记录的迁移差异如下：

- Go 构造函数返回具体指针；Rust 返回 `Box<dyn Task>`。
- Go 的 `getValidRules` 从包级 `ruleset.DefaultRuleSets` 取规则，先调用
  `OperandRules.Filter(groupExpression)` 处理表达式标志对应的规则子集，再按 Context
  rule mask 过滤。Rust 从可注入的 `Context::RulesFor(operand)` 取 `Vec<RuleRef>`，
  再检查根 Operand 和 `RuleEnabled`；本文件自身没有 Go 那个表达式敏感的
  `OperandRules.Filter` 步骤。
- Go `Desc` 把 writer 传给 `GroupExpression.String(w)`；Rust 的 `String()` 先返回
  `String`，再写入 writer。
- Rust 显式克隆 `Inputs` 与各 `Rc` 句柄以满足借用和所有权规则；Go 直接持有指针并
  索引切片。

因此，调度骨架已对齐，但若未来把 Go 的表达式敏感规则集接入 Rust，不能只依赖
当前的根 Operand 过滤。

## 扩展指南

- 新增规则选择条件时，优先扩展 `Context::RulesFor`/规则注册或本文件的
  `getValidRules`，并保持规则顺序与 LIFO 实际执行顺序可解释。若条件依赖完整表达式
  标志，应对照 Go `OperandRules.Filter`，避免只检查根 Operand。
- 修改子 Group 调度时必须维持“子 Group 在规则前完成”和“输入 0 先执行”两个
  不变量；改变压栈顺序前应同时检查 `Stack::Push/Pop` 和 scheduler 的 LIFO 实现。
- 若为本任务新增错误分支，应通过 `Task::Execute` 返回错误，让
  `ExecuteTasks` 的 `?` 统一短路，不要在此吞错。
- 回归测试应继续放在独立文件 [`task_test.rs`](task_test.rs)，不要内嵌到生产源文件。
  建议补充：禁用规则不生成任务、根 Operand 不匹配被过滤、多个规则的 LIFO 顺序、
  多个子 Group 的先后次序，以及 Go 表达式敏感规则集语义的等价覆盖。
- 更改 `Desc` 时同步检查任务栈描述测试或新增独立断言，保持文本对诊断工具稳定。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、目标文件 6 个符号；`explore`/`node`
  读取了目标文件全貌，并确认 `getValidRules → Execute` 的文件内调用关系。
- 目标与装配：[`task_opt_group_expression.rs`](task_opt_group_expression.rs)、
  [`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 上下游实现：[`task_opt_group.rs`](task_opt_group.rs)、
  [`task_apply_rule.rs`](task_apply_rule.rs)、[`base.rs`](base.rs)、
  [`task_scheduler.rs`](task_scheduler.rs)、[`../cascades.rs`](../cascades.rs)、
  [`../memo/group_expr.rs`](../memo/group_expr.rs)。
- Go 对照：[`task_opt_group_expression.go`](task_opt_group_expression.go)，并结合
  `pkg/planner/cascades/rule/ruleset/rule_set.rs` 核对表达式敏感过滤语义。
- 测试证据：[`task_test.rs`](task_test.rs) 的
  `task_chain_uses_real_memo_and_rule_contracts` 通过真实 Memo、规则、
  `NewOptGroupTask` 间接覆盖本任务链，并断言新表达式写入及规则 explored 状态；
  [`task_scheduler_test.rs`](task_scheduler_test.rs) 的 `TestSimpleTaskScheduler` 验证
  LIFO 与错误短路。当前没有同名独立测试，也没有逐项断言本文件所有过滤和顺序边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证使用任务规定的 11 章节结构命令，
  并人工核对上述路径、符号和调用边。
