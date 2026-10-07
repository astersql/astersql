# `pkg/planner/cascades/rule/binder.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-rule` crate。其 crate 根 `pkg/planner/cascades/rule/lib.rs` 以私有模块 `mod binder` 装入本文件，再通过 `pub use binder::*` 导出 `BoundPlan`、`Binder` 和 `NewBinder` 等公开接口；`Cargo.toml` 同时表明该 crate 直接依赖 `cascades-memo`、`cascades-pattern`、`logicalop`，并以 Go 包 `pkg/planner/cascades/rule` 为移植来源。

在完整 Cascades 优化链中，本文件位于“规则声明”和“规则执行”之间：规则通过 `Rule::Pattern` 给出模式树，`ApplyRuleTask::Execute`（`pkg/planner/cascades/task/task_apply_rule.rs`）用 `NewBinder` 将该模式绑定到一个指定的 Memo `GroupExpression`，随后对每个 `BoundPlan` 调用 `PreCheck` 和 `XForm`，再把改写结果写回 Memo。绑定器只负责枚举结构匹配，不决定规则语义，也不负责从其他根节点开始全树搜索。

## 核心职责

1. 用 `r#match` 判断一个固定的 `GroupExpressionRef` 的逻辑算子类别是否符合当前 `Pattern.Operand`。
2. 用 `dfsMatch` 从已固定的根表达式向其输入 Group 递归，把模式树与 Memo 中的表达式树对齐。
3. 用 `pickGroupExpression` 枚举每个子 Group 的逻辑等价表达式，并由 `dfsMatch` 对不同子位置的候选做笛卡尔组合。
4. 对 `OperandAny` 实施特殊剪枝：每个 Group 只取第一条逻辑表达式，避免枚举等价实现造成无意义的组合膨胀。
5. 用 `BoundPlan` 提供规则所需的统一视图：既能访问当前 `GroupExpression`/包装的 `LogicalPlan`，也能沿与 Pattern 对齐的孩子树访问和恢复子 Group。
6. 用 `Binder::Next` 顺序交付构造时预计算的匹配结果，并维护与 Go API 可观察状态相近的 `holder`。

本文件不匹配 `Pattern.EngineTypeSet`：`r#match` 只读取 `Pattern.Operand` 并调用 `GetOperand(...).Match(...)`。这与同路径 Go `match` 的当前行为一致，但意味着引擎约束必须由其他层保证，不能把 `Pattern::Match` 的完整“operand + engine”语义误认为已在这里执行。

## 主要符号

- `BoundPlan { expression, children }`：一次成功绑定形成的不可变快照。`expression: GroupExpressionRef` 指向当前 Memo 表达式，`children: Vec<BoundPlan>` 与当前 Pattern 的 `Children` 按位置一一对应。类型实现 `Clone`；克隆只克隆 `Rc` 句柄并递归克隆绑定树，不复制底层 Memo 表达式。
- `BoundPlan::Expression(&self) -> &GroupExpressionRef`：返回当前表达式句柄的共享引用。
- `BoundPlan::Children(&self) -> &[BoundPlan]`：返回已绑定孩子切片；叶 Pattern 或无子模式时为空。
- `BoundPlan::ChildGroups(&self) -> Vec<GroupRef>`：把直接孩子的表达式反查为所属 Group。`ApplyRuleTask` 将这些 Group 作为新逻辑算子的输入写回 Memo。
- `BoundPlan::WithWrappedLogicalPlan<T>(...) -> T`：在持有 `GroupExpression` 的 `RefCell` 不可变借用期间，把包装的 `dyn LogicalPlan` 交给同步闭包读取，并返回闭包结果。
- `Binder { matches, next, holder }`：枚举游标。`matches` 保存构造时收集的全部结果，`next` 指向下一项，`holder` 保存初始根或最近一次成功返回的结果。
- `NewBinder(pattern, expression) -> Binder`：公开构造入口。它立即调用 `dfsMatch` 完成全部搜索；同时把传入的根表达式包装成无孩子 `BoundPlan` 放入初始 `holder`，即使模式没有任何匹配也不清空该状态。
- `r#match(pattern, expression) -> bool`：内部单节点匹配。`OperandAny` 直接成功；其他 operand 通过包装逻辑算子的动态类型映射和 `Operand::Match` 判断。
- `dfsMatch(pattern, expression) -> Vec<BoundPlan>`：核心递归函数，验证根、检查模式孩子数与表达式输入 Group 数一致，并生成所有孩子候选组合。
- `anyHasBeenMatched(pattern, index) -> bool`：当模式为 `OperandAny` 且枚举下标已经越过 0 时返回真，是 Any 截断策略的唯一判据。
- `pickGroupExpression(pattern, group) -> Vec<BoundPlan>`：复制取得 Group 当前逻辑表达式句柄列表，按顺序筛选并递归生成候选；对 Any 只考察第 0 项。
- `Binder::Next(&mut self) -> Option<BoundPlan>`：克隆并返回下一项；成功时递增游标并同步 `holder`，耗尽后反复返回 `None`。
- `Binder::GetHolder(&self) -> Option<&BoundPlan>`：读取当前 holder，不推进枚举。

本文件没有模块级常量、trait、条件编译项或异步入口。公开 API 沿用 Go 风格的大写命名，因为 crate 根启用了 `#![allow(non_snake_case, non_upper_case_globals)]`。

## 执行流程

典型主链如下：

1. `ApplyRuleTask::Execute` 取得规则的 `Pattern` 和待处理 `GroupExpressionRef`，调用 `NewBinder`。
2. `NewBinder` 调用 `dfsMatch(&pattern, &expression)`。这一步已经完成全部匹配并把结果存入 `Binder.matches`；后续 `Next` 不再访问 Group 以寻找新候选。
3. `dfsMatch` 先调用 `r#match` 检查当前表达式。根不匹配立即返回空列表，所以传入的根是“固定根”，绑定器不会向下搜索另一个可作为根的子树。
4. 若 `pattern.Children` 为空，当前表达式本身构成唯一 `BoundPlan`，即使表达式实际拥有输入 Group，也不会继续约束或展开这些输入。
5. 若模式孩子数不等于表达式 `Inputs` 数，返回空列表。这保证成功结果的 `children` 与模式孩子严格按位对齐。
6. 对每一对 `(child_pattern, input_group)`，`pickGroupExpression` 依 Group 当前顺序取表达式，先做 operand 过滤，再对候选递归调用 `dfsMatch`。任何一个子位置没有候选时，整棵绑定失败。
7. `dfsMatch` 从 `combinations = [[]]` 开始，逐子位置扩展前缀：已有每个前缀分别追加该位置的每个候选。因此两个子 Group 分别有 2 个候选时，结果顺序是 `(左1,右1)、(左1,右2)、(左2,右1)、(左2,右2)`；`binder_test.rs::TestBinderMultiNext` 明确验证了该顺序。
8. 每组完整孩子组合被包装为一个根 `BoundPlan`。`Binder::Next` 仅按向量顺序克隆交付它们。
9. `ApplyRuleTask` 对返回值执行规则检查/变换，用 `ChildGroups` 取得直接孩子所属 Group，将新表达式接回原目标 Group，并继续调度优化任务。

`OperandAny` 的流程有一处刻意的不对称：`r#match` 让它匹配任意单节点，但 `pickGroupExpression` 的 `take_while` 只允许索引 0 进入后续筛选和递归。因此 Any 表示“引用这个 Group 即可”，不是“枚举 Group 中全部具体表达式”。

## 数据与状态

Memo 的共享结构来自 `cascades-memo`：`GroupExpressionRef = Rc<RefCell<GroupExpression>>`，`GroupRef = Rc<RefCell<Group>>`。`GroupExpression` 持有包装的逻辑算子和按输入顺序排列的 `Vec<GroupRef>`；Group 强持有其逻辑表达式，而表达式用 `Weak` 回指所属 Group，避免所有权环。

`BoundPlan` 保存的是对这些表达式的强 `Rc` 句柄以及一棵独立的绑定孩子向量。因此结果不会复制逻辑算子，但会延长被绑定表达式的生命期。孩子树描述的是“本次 Pattern 选择了哪些具体 GroupExpression”，不等同于修改 Memo 内的计划孩子。

`Binder` 的状态转换为：

- 构造后：`next = 0`，`matches` 已完整确定，`holder = Some(传入根的无孩子 BoundPlan)`。
- 成功 `Next` 后：返回 `matches[next]` 的克隆，`next += 1`，`holder` 更新为同一匹配的另一份句柄克隆。
- 耗尽后：返回 `None`，`next` 和 `holder` 均保持不变。因此“从未成功”时 holder 仍是初始根；“曾成功后耗尽”时 holder 仍是最后一个成功结果。

搜索复杂度取决于模式深度、每个 Group 的候选数以及子位置候选的乘积。非 Any 子模式会产生完整笛卡尔积；所有结果又在 `NewBinder` 阶段一次性存入内存。Any 的首项剪枝可显著降低组合数，但非 Any 的等价表达式很多时仍可能有时间和内存膨胀。

## 依赖与调用关系

直接上游：

- `pkg/planner/cascades/task/task_apply_rule.rs::ApplyRuleTask::Execute` 是生产主调用者：`NewBinder` 后循环 `Next`，并消费 `BoundPlan::ChildGroups`。
- `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs` 等规则通过 `BoundPlan::WithWrappedLogicalPlan` 和 `Children` 检查已绑定算子/子树。
- `pkg/planner/cascades/rule/binder_test.rs`、`join/join_to_apply_aster_unit_test.rs`、`ruleset/rule_set_aster_unit_test.rs` 构造绑定器验证规则相关行为。

直接下游：

- `cascades_pattern::{Pattern, OperandAny, GetOperand}`：提供模式树、通配 operand 和逻辑算子到 operand 的映射。
- `cascades_memo::{GroupExpressionRef, GroupRef}`：提供 Memo 表达式/等价组的共享句柄。`dfsMatch` 克隆 `GroupExpression.Inputs`，`pickGroupExpression` 调用 `Group::GetLogicalExpressions`，`ChildGroups` 调用 `GroupExpression::GetGroup`。
- `logicalop::LogicalPlan`：`WithWrappedLogicalPlan` 暴露只读动态 trait 视图，供规则下转具体逻辑算子。

RustCodeGraph 对本文件确认的内部边包括 `NewBinder → dfsMatch`、`dfsMatch → r#match/pickGroupExpression`、`pickGroupExpression → r#match/dfsMatch/anyHasBeenMatched`。索引的 callers 查询在本次执行中超时且没有输出，因此生产上游另由直接引用搜索和 `task_apply_rule.rs` 源码核实。

## 错误处理与边界

本文件的匹配 API 不返回 `Result`：正常的不匹配、孩子数量不一致、某个子 Group 无候选以及枚举耗尽，分别表现为空 `Vec` 或 `None`，不是错误。

需要调用方维护的边界和潜在 panic 如下：

- `ChildGroups` 要求每个直接孩子表达式仍属于一个存活的 Group；`GetGroup()` 为 `None` 时以 `expect("bound expression must belong to a group")` panic。正常 Memo 插入路径会设置该弱回指，但分离/删除表达式后再调用不满足此前提。
- `Rc<RefCell<_>>` 的借用规则在运行时检查。`WithWrappedLogicalPlan` 的闭包执行期间仍持有对表达式的不可变借用；闭包若经别名尝试可变借用同一表达式会触发 `RefCell` panic。闭包不应保存超出调用期的计划引用，签名也通过借用生命周期阻止这种逸出。
- `dfsMatch` 克隆输入 Group 列表后释放表达式借用，`pickGroupExpression` 克隆表达式句柄列表后释放 Group 借用；正常递归不跨层长期持有 `RefCell` 借用。
- 固定根语义是重要边界：根 operand 不匹配时不会扫描其后代。全树遍历应由外部调度器逐个 GroupExpression 发起，而不是修改此处使 Binder 偷偷换根。
- 空子模式表示只约束当前节点，不要求表达式也无输入；相反，只要模式含孩子，就要求孩子数量与输入 Group 数完全相等。
- 当前匹配忽略 `EngineTypeSet`。若新需求要求引擎过滤，应先确认 Go 兼容预期和调用层责任，不能只在 Rust 侧无测试地加入过滤。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件句柄或网络资源。`Rc<RefCell<_>>` 使 `Binder`/`BoundPlan` 天然面向单线程执行，通常既非 `Send` 也非 `Sync`；调度器应在同一线程内消费它们。

资源生命周期由引用计数控制：`Binder.matches`、`holder` 和调用方取得的 `BoundPlan` 都会强持有相应 `GroupExpressionRef`。`Next` 的克隆会让返回值独立于 Binder 存活；Binder 被丢弃后，调用方持有的 BoundPlan 仍可读表达式。表达式到所属 Group 的回边是 `Weak`，所以 `ChildGroups` 只能在 Group 仍存活时成功升级。

构造时预计算意味着 Group 的后续增删不会改变已经创建的 Binder 的候选集合；已收集的表达式句柄仍可能存活，但其所属 Group 弱回指可能因删除而失效。安全用法是在 `NewBinder` 后立即、同步地消费全部 `Next` 结果，不在枚举期间结构性修改相关 Memo Group。

## 与 Go 版本的对应关系

Rust 文件对齐 `pkg/planner/cascades/rule/binder.go` 的外部意图：给定一个固定根和 Pattern，按 Group 中的等价表达式枚举匹配；Any 对每个 Group 只选择首项；根不会自动下移；构造时 holder 先指向传入根；匹配成功后规则可观察选中的表达式树。

实现策略存在明确差异：

- Go `Binder` 保存 `p`、`traceID`、`stackInfo` 和可变 `holder`，`Next` 每次借助链表元素游标进行惰性 DFS/回溯；Rust `Binder` 用 `dfsMatch` 在 `NewBinder` 中一次性生成 `Vec<BoundPlan>`，`Next` 只是向量游标。
- Go 把 `GroupExpression` 当作 `base.LogicalPlan`，通过动态 placeholder 的 `Children/SetChild` 组装当前绑定；Rust 不修改 Memo 表达式的计划孩子，而以显式 `BoundPlan { expression, children }` 适配规则接口。
- Go 的 `anyHasBeenMatched` 通过 Group 的首 operand 元素与当前链表元素比较；Rust 依 `GetLogicalExpressions()` 的枚举下标，仅允许索引 0。对当前 Any 语义二者测试结果一致，但 Rust 策略直接依赖组内向量顺序。
- Go 的 `match` 和 Rust 的 `r#match` 都只做 operand 匹配，均未使用 Pattern 的引擎集合。
- Rust 的预计算实现更简单且结果顺序由嵌套循环稳定决定，但峰值内存为全部绑定之和；Go 惰性实现只维护回溯状态，更适合巨大组合空间。

Rust 独立测试 `pkg/planner/cascades/rule/binder_test.rs` 对照 Go 的 `binder_test.go` 覆盖成功/失败、顶层节点、孤立节点、子树结构、2×2 多结果、单 Any 和双 Any。Rust 测试还显式断言失败后初始 holder 不清空，并验证固定根不会递归寻找替代根。

## 扩展指南

- 新增或调整匹配语义时，优先修改 `r#match`，并同步检查 `cascades-pattern` 的 `GetOperand`/`Operand::Match`。若涉及引擎过滤，必须补充 Pattern engine 的独立测试，并评估是否会偏离 Go 当前行为。
- 改变 Group 候选选择或 Any 行为时，入口是 `pickGroupExpression` 和 `anyHasBeenMatched`。必须保留并扩展 `TestBinderAny`、`TestBinderMultiAny`，同时覆盖“首项不符合/组为空/多种 operand 混排”等顺序风险。
- 改变组合顺序或枚举策略时，入口是 `dfsMatch` 与 `Binder::Next`。`TestBinderMultiNext` 的四项顺序是现有可观察行为；若改成惰性回溯，应保证失败回退、穷尽和 holder 状态兼容，并避免为通过测试而减少组合。
- 扩展 `BoundPlan` 读取接口时，保持对 Memo 的只读适配职责。需要改变 Memo 所有权或表达式归属时，应在 `cascades-memo` 处理，不应在 Binder 内制造新的 Group 或绕开弱回指不变量。
- 新规则通常不应修改 Binder：在规则的 `Pattern` 声明结构，在 `PreCheck`/`XForm` 中通过 `Expression`、`Children`、`WithWrappedLogicalPlan` 读取绑定，再由任务层用 `ChildGroups` 接回 Memo。
- 测试逻辑必须继续放在独立的 `pkg/planner/cascades/rule/binder_test.rs`，不要内嵌到生产源文件；若行为旨在对齐 Go，同步核对 `binder.go` 和 `binder_test.go`。
- 性能扩展的主要风险是预计算笛卡尔积。若真实规则出现大候选空间，应以等价的惰性迭代器/显式回溯状态替换，而不是静默截断非 Any 候选；还需验证 Memo 在枚举期间的修改策略和结果稳定性。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/planner/cascades/rule/binder.rs`（完整 184 行），主要符号为 `BoundPlan`、`Binder`、`NewBinder`、`r#match`、`dfsMatch`、`anyHasBeenMatched`、`pickGroupExpression`、`Next`、`GetHolder`。
- crate 装配：`pkg/planner/cascades/rule/Cargo.toml` 与 `pkg/planner/cascades/rule/lib.rs`；确认 crate 名、路径依赖、Go 来源包、模块私有装入和公开再导出。
- 生产调用链：`pkg/planner/cascades/task/task_apply_rule.rs::ApplyRuleTask::Execute`；确认 `NewBinder → Next → Rule::PreCheck/XForm → ChildGroups → CopyInWithChildren`。
- 直接数据结构：`pkg/planner/cascades/memo/group_expr.rs`、`pkg/planner/cascades/memo/group.rs`、`pkg/planner/cascades/pattern/pattern.rs`、`pkg/planner/cascades/rule/rule.rs`；确认 `Rc<RefCell>`/`Weak` 所有权、Group 表达式顺序、Pattern 字段和规则消费契约。
- Rust 测试：`pkg/planner/cascades/rule/binder_test.rs`；确认成功/失败、固定根、叶模式、子树、笛卡尔积顺序以及 Any 首项语义。
- Go 对照：`pkg/planner/cascades/rule/binder.go` 与 `pkg/planner/cascades/rule/binder_test.go`；确认目标行为，并识别惰性栈回溯与 Rust 预计算快照的实现差异。
- RustCodeGraph：`status` 显示索引包含目标仓库；`node --file` 读取目标、测试、Go 对照和主调用者；精确 `query` 找到 Rust/Go 同名符号；`callees` 确认内部调用边。`callers` 查询本次超时无输出，因此上游关系以直接引用搜索复核，未把缺失图结果写成事实。
- 结构验证要求：目标文件存在，并且恰好包含本页所示的 11 个固定二级标题。任务为纯文档分析，按计划不运行 Cargo 或代码测试。
