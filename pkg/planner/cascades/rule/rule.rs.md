# `pkg/planner/cascades/rule/rule.rs`

## 文件定位

本文件是新 Cascades 逻辑优化器的规则契约层，属于 Cargo crate `astersql-planner-cascades-rule`。crate 入口 `pkg/planner/cascades/rule/lib.rs` 将本文件的公开项全部再导出，因此调度器、规则集合和具体规则通常通过 `cascades_rule::{Rule, BaseRule, NewBaseRule, RuleError}` 使用它们，而不直接引用私有模块 `rule`。

它位于规则模式与任务调度之间：`Pattern` 描述候选计划的结构，`BoundPlan` 表示 Binder 已绑定到 Memo 组表达式的结果，`Rule::XForm` 则产出可重新写入 Memo 的 `LogicalPlanRef`。真实主链见 `pkg/planner/cascades/task/task_opt_group_expression.rs` 和 `pkg/planner/cascades/task/task_apply_rule.rs`。

## 核心职责

1. 用 `Rule` trait 统一规则 ID、跟踪名称、结构模式、前置检查、语义匹配和变换结果的接口。
2. 用 `RuleError` 为规则执行失败提供可显示、可作为标准错误传播的字符串载荷。
3. 用 `BaseRule` 保存所有简单规则共享的 `Type` 与 `Pattern`，并提供与 Go `BaseRule` 相同的保守默认行为：ID 为 0、检查通过、变换不产生替代计划且不删除原表达式。
4. 用 `NewBaseRule` 构造拥有自身 `Pattern` 的 Rust 基础规则对象，供具体规则组合并转发通用方法。

本文件只定义规则协议和默认骨架，不负责枚举 Binder 结果、修改 Memo、选择启用规则或实现具体优化变换。

## 主要符号

- `pub struct RuleError(pub String)`：规则级错误。元组字段公开，调用者可以直接用可读消息构造；派生 `Clone`、`Debug`、`Eq`、`PartialEq`，并实现 `Display` 与 `std::error::Error`。`Display` 原样输出内部字符串，不附加上下文。
- `pub trait Rule`：对象安全的变换规则接口，当前通过 `Rc<dyn Rule>`（任务层）和 `Box<dyn Rule>`（规则集合层）动态分发。
  - `ID(&self) -> usize`：规则掩码和组表达式“已探索”状态的键。正确性要求不同有效规则使用稳定且适当区分的 ID。
  - `String(&self, writer: &mut dyn cascades_util::StrBufferWriter)`：将跟踪名称写入调用者提供的缓冲区；它不是 Rust 的 `Display` 实现。
  - `Pattern(&self) -> &Pattern`：借用规则持有的结构模式，调度器会克隆该模式交给 Binder。
  - `PreCheck(&self, &BoundPlan) -> bool`：每个绑定上的低成本准入检查，默认 `true`。
  - `Match(&self, &BoundPlan) -> bool`：供具体规则或调用方进行额外语义匹配，默认 `true`。当前 `task_apply_rule.rs` 调度链并未调用该方法，这一点与同路径 Go `ApplyRuleTask.Execute` 的现状一致。
  - `XForm(&self, &BoundPlan) -> Result<(Vec<LogicalPlanRef>, bool), RuleError>`：返回等价逻辑计划和“是否移除源组表达式”标志；默认返回空列表与 `false`。
- `pub struct BaseRule`：私有字段 `tp: Type` 与 `pattern: Pattern` 分别保存规则类别和匹配树。字段私有，外部只能经 `Rule` 方法观察。
- `pub fn NewBaseRule(tp: Type, pattern: Pattern) -> BaseRule`：按值取得并保存模式，返回非装箱对象。具体规则通常将它作为字段组合，而非通过继承扩展。
- `impl Rule for BaseRule`：`ID` 固定为 0；`String` 写入 `tp.String()`；`Pattern` 返回内部模式；其余方法采用 trait 默认实现。

本文件没有模块级常量、条件编译项、异步函数或 `unsafe` 代码。

## 执行流程

现行执行链可由 `pkg/planner/cascades/cascades.rs`、`task_opt_group_expression.rs` 和 `task_apply_rule.rs` 复核：

1. `Context::RegisterRules` 按根 `Operand` 保存 `Rc<dyn Rule>`。
2. `OptGroupExpressionTask::getValidRules` 取得当前计划的 operand，只保留根 pattern operand 相等且规则掩码启用的规则，然后创建 `ApplyRuleTask`。
3. `ApplyRuleTask::Execute` 先读取 `Rule::ID`；若该组表达式已探索此 ID 或已废弃，立即结束。
4. 调度器克隆 `Rule::Pattern()`，用 `NewBinder` 枚举该模式在目标 Memo 组表达式上的所有 `BoundPlan` 绑定。
5. 每个绑定先经过 `Rule::PreCheck`；返回 `false` 时只跳过该绑定。
6. 通过检查后调用 `Rule::XForm`。每个返回的新计划用原绑定的孩子组调用 `CopyInWithChildren` 写入同一等价组，并压入后续 `OptGroupExpressionTask`；`remove == true` 时调用 `RemoveOut` 摘除源表达式。
7. 所有绑定处理完成后，以规则 ID 标记源组表达式已探索，防止重复应用。

`Rule::Match` 不在上述现行调度链中。具体规则可以覆盖它（例如 `join/join_to_apply.rs`），测试也可显式调用，但不能据此断言生产调度会自动执行该过滤。

## 数据与状态

`BaseRule` 是纯规则元数据：`Type` 决定跟踪字符串，`Pattern` 是 Binder 使用的模式树。构造后本文件没有修改这两个字段的方法，`Pattern()` 只返回共享借用；调度器在进入 Binder 前自行克隆模式，所以绑定过程不会改写规则持有的原始模式。

`Rule` 本身不持有 Memo 或优化器状态。跨执行步骤的状态位于外部：规则注册表在 `cascades.rs::Context`，启用集合在规则掩码中，规则是否已执行记录在 `GroupExpression`，生成的候选计划及等价组位于 Memo。

`XForm` 的布尔值是影响 Memo 生命周期的重要协议：`false` 表示保留源表达式并添加等价候选，`true` 表示生成候选后还应移除源表达式。空候选加 `false` 是无操作；`BaseRule` 和尚为占位的 Join→Apply 规则都使用这一安全默认值。

## 依赖与调用关系

直接依赖如下：

- crate 内部 `crate::{BoundPlan, Type}`：前者由 `binder.rs` 再导出，后者由 `rule_type.rs` 再导出。
- `cascades-pattern::Pattern`：规则结构模式；Cargo manifest 将其映射到相邻 crate `astersql-planner-cascades-pattern`。
- `cascades-util::StrBufferWriter`：规则描述输出接口。
- `logicalop::LogicalPlanRef`：变换产物；manifest 映射到 `astersql-planner-core-operator-logicalop`。
- 标准库 `std::fmt` 与 `std::error::Error`：错误展示和错误类型集成。

RustCodeGraph 对本文件给出的直接使用文件包括 `pkg/planner/cascades/cascades_test.rs` 与 `pkg/planner/cascades/rule/binder_test.rs`；精确符号查询还确认 `NewBaseRule` 被二者调用。源码搜索补充了实际生产调用：`join/join_to_apply.rs` 组合 `BaseRule` 并实现 `Rule`，`ruleset/rule_set.rs` 以 `Box<dyn Rule>` 保存与过滤规则，`task/task_apply_rule.rs` 以 `Rc<dyn Rule>` 驱动 Binder 和 Memo 写回。

需要区分 `apply/decorrelateapply/xf_decorrelate_apply_base.rs` 中另一套局部 `Rule`/`BaseRule` 类型；那是 Apply 解相关辅助模型，不是本文件定义的同名符号。接入新 Cascades 主调度链时应导入 `cascades_rule::Rule`，避免误用同名接口。

## 错误处理与边界

`RuleError` 不分类错误，也没有自动的来源链；错误语义完全由消息字符串承担。`ApplyRuleTask::Execute` 会将它的 `Display` 文本转换为 `TaskError`，因此具体规则应给出包含操作和失败对象的上下文，避免只返回模糊短语。

默认实现刻意宽松：`PreCheck` 和 `Match` 恒为真，`XForm` 成功但无输出。这让 `BaseRule` 可用作协议骨架和调度测试桩，但不能代替真实规则逻辑。尤其是 `ID == 0` 会让多个未覆盖 ID 的规则共享探索位；生产规则若直接继承这一默认值，可能被彼此错误去重。

本文件不验证返回的新计划是否与源计划语义等价，不验证 pattern 与 `BoundPlan` 是否一致，也不限制 `remove == true` 时是否至少生成一个候选；这些是不变量，由具体规则及其独立测试负责。`Pattern()` 返回引用，规则对象必须保证其生命周期覆盖调度使用期。

## 并发与资源生命周期

本文件没有线程、锁、通道、异步任务或显式 I/O。规则方法只接受共享借用，因此单次调用不会通过本接口可见地修改规则对象；但 trait 没有要求 `Send` 或 `Sync`，不能假定规则可跨线程共享。

当前任务层使用单线程引用计数 `Rc<dyn Rule>`，`ApplyRuleTask` 和 `Context` 在串行栈式调度中共享规则实例；规则集合则用拥有型 `Box<dyn Rule>`。`BaseRule` 按值拥有 `Pattern`，析构时由 Rust 自动释放模式树。`RuleError` 拥有消息 `String`，向任务错误转换后原错误可正常释放。

Memo 的插入、移除和探索标记都发生在本文件之外。规则实现不应在 `XForm` 中私自长期持有 `BoundPlan` 内部引用；应返回拥有型 `LogicalPlanRef`，让调度器统一管理写入和后续任务生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/rule/rule.go`，Rust 保留了 Go 风格符号名和方法顺序，核心默认语义一致：

- Go `Rule` 的 `ID/String/Pattern/PreCheck/Match/XForm` 对应 Rust 同名 trait 方法。
- Go `BaseRule` 保存 `Type` 与 `*pattern.Pattern`；Rust `BaseRule` 保存 `Type` 与拥有型 `Pattern`，通过借用返回模式，避免空指针状态。
- Go `NewBaseRule` 返回指针；Rust 返回按值对象，具体规则再组合或装入 `Rc`/`Box`。
- Go `XForm` 返回 `([]LogicalPlan, bool, error)`；Rust 用 `Result<(Vec<LogicalPlanRef>, bool), RuleError>` 把错误放入类型化结果。
- Go 默认 `XForm` 返回 `nil, false, nil`；Rust 返回空 `Vec`、`false` 和 `Ok`，调度效果相同。
- Go `ID` 是 `uint`，Rust 是 `usize`；两者都被规则掩码和探索状态使用。

`pkg/planner/cascades/task/task_apply_rule.go` 也只在 Binder 绑定后调用 `PreCheck` 与 `XForm`，没有调用 `Match`；Rust 当前行为并非额外删减。`rule_type.rs` 与 `rule_type.go` 共同证明 `BaseRule::String` 的映射：`XFJoinToApply` 为 `join_to_apply`，其他当前类型回落为 `default_none`。

## 扩展指南

新增规则时应在独立源文件定义具体类型并 `impl cascades_rule::Rule`，将 `BaseRule` 作为字段复用 `String` 与 `Pattern`，同时显式实现稳定且唯一的 `ID` 和真实 `XForm`。如果需要过滤，优先在现行调度确实调用的 `PreCheck` 中实现；若只覆盖 `Match`，还必须同步接线调用方，否则生产路径不会生效。

构造 pattern 时应选择准确的根 `Operand` 与引擎集合，因为 `OptGroupExpressionTask` 先按根 operand 筛选，Binder 再验证完整模式。变换返回的计划必须与源表达式等价，并明确是否应删除源表达式；任何新错误应通过 `RuleError` 写出足够上下文。

同步测试应放在独立测试文件，不要嵌入 `rule.rs`。可按关注点扩展：

- 基础默认值和 Go 对齐：`pkg/planner/cascades/rule/binder_test.rs`。
- 具体规则的 pattern、匹配和 XForm：相应规则目录的 `*_test.rs`，例如 `join/join_to_apply_aster_unit_test.rs`。
- 动态规则列表与 ID/顺序：`ruleset/rule_set_aster_unit_test.rs`。
- Binder→XForm→Memo CopyIn/RemoveOut 的完整任务链：`pkg/planner/cascades/task/task_test.rs`。
- 优化器注册、掩码与探索状态：`pkg/planner/cascades/cascades_test.rs`。

主要兼容风险是改变 ID、规则名称或默认返回语义；正确性风险是产生不等价计划或错误设置 `remove`；性能风险是过宽 pattern、昂贵 `PreCheck/XForm` 或生成大量重复候选。若计划让 `Match` 进入主链，应同时核对 Go 行为并新增能证明调用顺序与过滤效果的任务级回归测试。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/planner/cascades/rule` 确认目标与邻接 Go/Rust/测试文件；`node --file pkg/planner/cascades/rule/rule.rs` 读取完整 91 行源码并报告直接使用文件；`query/node NewBaseRule` 确认 Rust/Go 定义及 Rust 测试调用者。宽泛 `explore` 结果存在同名符号噪声，因此调用链结论以精确节点和下列源码为准。
- 目标与 crate 边界：`pkg/planner/cascades/rule/rule.rs`、`lib.rs`、`Cargo.toml`、`rule_type.rs`。
- Go 对照：`pkg/planner/cascades/rule/rule.go`、`rule_type.go`、`pkg/planner/cascades/task/task_apply_rule.go`。
- 生产调用链：`pkg/planner/cascades/cascades.rs`、`task/task_opt_group_expression.rs`、`task/task_apply_rule.rs`、`rule/ruleset/rule_set.rs`、`rule/join/join_to_apply.rs`。
- 独立测试：`pkg/planner/cascades/rule/binder_test.rs` 验证 BaseRule 默认 ID、Pattern 和类型字符串；`rule/ruleset/rule_set_aster_unit_test.rs` 验证 trait object、规则 ID/顺序与真实 XForm；`task/task_test.rs` 验证 Binder、XForm、Memo 写回和探索标记；`cascades_test.rs` 验证注册规则经优化器调度。
- 人工复核结论：本文件存在于“规则定义”与“调度执行”边界；安全扩展的关键是唯一 ID、准确 Pattern、在 `PreCheck`/`XForm` 实现真实逻辑、保持 Go 协议，并通过独立规则测试和任务链测试验证。
