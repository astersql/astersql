# [`pkg/planner/cascades/rule/join/join_to_apply.rs`](join_to_apply.rs)

## 文件定位

本文件属于 `astersql-planner-cascades-rule-join` crate，是 Cascades 优化器 Join 变换规则子模块中的 Rust 实现。模块入口 `pkg/planner/cascades/rule/join/lib.rs` 私有声明 `join_to_apply` 后公开再导出其符号；`pkg/planner/cascades/rule/join/Cargo.toml` 声明该 crate 直接依赖 Pattern、Rule、Cascades 工具层和逻辑算子层。

它对照同目录 Go 文件 `join_to_apply.go` 移植 `XFJoinToApply` 和 `NewJoinToApply`。当前文件并没有真正生成 Apply：它只建立规则元数据和匹配形状，并保持 Go 侧尚未实现的空变换行为。`pkg/planner/cascades/rule/ruleset/rule_set.rs::DefaultRuleSets` 当前返回空映射，因此该规则也没有注册进默认优化流程；现阶段的直接使用者是独立单元测试，而不是生产规则集。

## 核心职责

1. 用 `NewJoinToApply` 构造规则的结构模式：根为 TiDB 引擎的 Join，左孩子允许任意引擎/任意算子，右孩子必须是 TiDB 引擎的 Join。
2. 用组合字段 `XFJoinToApply::BaseRule` 保存规则类型和 Pattern，并把 `ID`、`String`、`Pattern` 委托给 `BaseRule`。
3. 实现 `Rule` 契约中的 `Match` 与 `XForm`，但目前仅保留占位语义：`Match` 恒为 `true`，`XForm` 成功返回“无候选计划、不要移除原表达式”。

因此，该文件存在的价值是固定 Go/Rust 公共结构与未来扩展入口，而不是提供已可用的 Join→Apply 优化。源码顶部注释、`XForm` 内注释及 Go 文件中的 TODO 都支持这一结论。

## 主要符号

- `pub struct XFJoinToApply { BaseRule: BaseRule }`：规则对象。字段不公开，外部只能通过 `Rule` trait 观察规则 ID、名称、Pattern 和变换行为。该类型没有额外可变状态。
- `pub fn NewJoinToApply() -> XFJoinToApply`：唯一构造入口。先以 `NewPattern(OperandJoin, EngineTiDBOnly)` 创建根，再用 `SetChildren` 安装 `OperandAny@EngineAll` 和 `OperandJoin@EngineTiDBOnly` 两个孩子，最后用 `NewBaseRule(RuleType, pattern)` 组装规则。
- `RuleType`：导入自 `cascades_rule::XFJoinToApply` 的别名，对应 `pkg/planner/cascades/rule/rule_type.rs::Type::XFJoinToApply`。类型判别值是 1，字符串为 `join_to_apply`。
- `Rule for XFJoinToApply::ID`：委托给 `BaseRule::ID`。按当前 `pkg/planner/cascades/rule/rule.rs` 实现，返回值是 0，而不是枚举判别值 1；独立测试也明确断言 0。这是当前事实，扩展时不可假定 ID 已与 `Type` 判别值关联。
- `Rule for XFJoinToApply::String`：把 `BaseRule` 的规则名写入调用者提供的 `cascades_util::StrBufferWriter`，当前结果为 `join_to_apply`。
- `Rule for XFJoinToApply::Pattern`：借用并返回构造时保存的 Pattern，不复制或修改它。
- `Rule for XFJoinToApply::Match`：忽略 `BoundPlan` 并恒返回 `true`。
- `Rule for XFJoinToApply::XForm`：忽略 `BoundPlan`，返回 `Ok((Vec::new(), false))`；空向量表示没有等价表达式，`false` 表示不移除原表达式。

本文件没有模块级常量、条件编译项、异步函数或内部辅助函数。

## 执行流程

构造阶段由 `NewJoinToApply` 完成：创建根 Join Pattern，设置两个有序孩子，再将规则类型与完整 Pattern 封装到 `BaseRule` 中。孩子顺序是语义的一部分：左边是任意计划，右边是嵌套 Join；不是“任一侧存在 Join”即可。

若未来把该规则注册到规则集，当前调度链会是 `ApplyRuleTask::Execute` → `NewBinder(rule.Pattern(), group_expression)` → 对每个绑定调用 `PreCheck` → `XForm`。`pkg/planner/cascades/task/task_apply_rule.rs` 的当前实现并未调用 `Rule::Match`，所以本文件的恒真 `Match` 对这条 Rust 调度链没有实际过滤作用。Binder 负责根据 Pattern 做结构匹配；本文件的 `XForm` 随后返回空候选，调度器不会执行 `CopyInWithChildren`，也不会因 `remove` 删除原 GroupExpression，只会最终把该规则 ID 标记为已探索。

不过，这条流程目前只是“注册后会发生什么”：`DefaultRuleSets` 为空，仓库内 `NewJoinToApply` 的 Rust 调用只出现在 `join_to_apply_aster_unit_test.rs`。测试自行构造一个无孩子的 `LogicalJoin` 绑定到一个仅约束根 Join 的测试 Pattern，再直接调用本规则的 `Match` 和 `XForm`；它没有通过本规则自身要求两个孩子的 Pattern 驱动 Binder。

## 数据与状态

规则对象唯一持久状态是 `BaseRule`，其内部持有规则类型 `Type` 与拥有型 `Pattern`。构造完成后，本文件只提供共享借用，没有内部可变性、缓存或惰性初始化。

Pattern 的关键不变量是：

- 根：`OperandJoin` + `EngineTiDBOnly`；
- 孩子数：恰好 2；
- 左孩子：`OperandAny` + `EngineAll`；
- 右孩子：`OperandJoin` + `EngineTiDBOnly`。

`BoundPlan` 只以不可变引用传入 `Match`/`XForm`，当前完全不读取。返回值中的候选计划向量每次新建且为空，不与 Memo 或输入计划共享资源；`remove == false` 保留原表达式。

## 依赖与调用关系

直接依赖与用途如下：

- `cascades_pattern::{NewPattern, Pattern, OperandAny, OperandJoin, EngineAll, EngineTiDBOnly}`：定义并构建结构/引擎匹配条件；对应 Cargo 依赖 `astersql-planner-cascades-pattern`。
- `cascades_rule::{BaseRule, NewBaseRule, Rule, BoundPlan, RuleError, XFJoinToApply}`：提供规则骨架、trait、输入绑定、错误类型和规则种类；对应父目录 rule crate。
- `cascades_util::StrBufferWriter`：`String` 方法的输出抽象；对应 util crate。
- `logicalop::LogicalPlanRef`：`XForm` 候选输出类型；对应逻辑算子 crate。

直接上游是 `lib.rs` 的公开再导出与 `join_to_apply_aster_unit_test.rs` 的构造调用。RustCodeGraph 将目标文件标为被 `join_to_apply_aster_unit_test.rs` 和 `rule/binder_test.rs` 使用；其中后者验证的是共享 `NewBaseRule`/规则类型行为，并不直接构造 `XFJoinToApply`。生产调度器 `task_apply_rule.rs` 面向 `dyn Rule` 调用，不静态引用本类型。当前默认规则集未注册该构造器，所以不存在从正常优化入口到 `NewJoinToApply` 的已接通调用边。

## 错误处理与边界

构造函数没有可失败分支。`Match` 对任何 `BoundPlan` 都返回成功，不能承担相关列、Join 类型、等价性或代价条件检查。`XForm` 的签名允许返回 `RuleError`，但当前只返回 `Ok`，不会生成错误信息。

真正的边界首先由 Pattern/Binder 控制：必须是指定引擎集合和有序树形。即使结构匹配成功，当前也不会产生 Apply。若未来实现转换，必须在生成候选前验证相关表达式能否安全传给 Apply probe side、Join 类型与条件能否保持语义，以及孩子和属性如何映射；Go 的 TODO 仅写明“检查 join 是否可转换”，不能当作已有算法。

还需注意两个现状边界：一是 `BaseRule::ID()` 当前恒为 0，多个规则若共用该实现可能在探索掩码中冲突；二是 `ApplyRuleTask` 当前没有调用 `Match`。这两点若影响未来实现，应在规则基础设施中单独解决，不能只在本文件里增加判断后假定调度器会执行。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。`XFJoinToApply` 是普通拥有型值；Pattern 随对象创建并由对象持有，对外仅返回 `&Pattern`。`XForm` 返回的新 `Vec` 由调用者拥有，当前为空，离开作用域即可释放。

实际调度器用 `Rc<dyn Rule>` 在单线程任务与 Context 间共享规则实例，而不是 `Arc`；这表明当前规则生命周期面向本地任务栈，不提供跨线程共享保证。本规则自身没有内部可变状态，因此重复调用不会改变对象，但“是否已探索”记录在 Memo 的 GroupExpression 中，由 `ApplyRuleTask` 管理，不属于本文件。

## 与 Go 版本的对应关系

Rust 与 `pkg/planner/cascades/rule/join/join_to_apply.go` 保持以下语义一致：类型名和构造器名相同；Pattern 都是 `Join@TiDBOnly(Any@EngineAll, Join@TiDBOnly)`；`Match` 都恒真；`XForm` 都不返回候选、不移除原计划且不报错。Go 用嵌入的 `*rule.BaseRule`，Rust 用拥有字段 `BaseRule` 并显式委托 trait 方法；Go 的 `nil` 候选对应 Rust 的空 `Vec`。

Go 注释描述了目标方向：把 Join 转为 Apply，使运行时标量属性传入 probe side，从而增加使用更好索引扫描的机会；但 Go `XForm` 仍有 TODO。因此 Rust 明确保持空实现是忠实移植，不代表上述优化已可用。

当前 Rust 独立测试比 Go 同文件提供了更明确的回归契约：`join_to_apply_aster_unit_test.rs::join_to_apply_keeps_go_pattern_and_todo_result` 检查完整 Pattern、当前 ID 为 0、`Match == true`、空候选及 `remove == false`。未发现同名 Go 测试；相关 Binder 行为由 `pkg/planner/cascades/rule/binder_test.rs` 等独立测试覆盖。

## 扩展指南

实现真正转换时，最可能修改 `XFJoinToApply::Match` 和 `XFJoinToApply::XForm`；若结构要求变化，再同步修改 `NewJoinToApply`。实现前应先确认调度器是否应补上 `Match` 调用，否则将必要检查放在 `Match` 中不会影响生产路径。还应确认规则 ID 是否应由 `BaseRule` 的 `tp` 派生，并在默认 `OperandJoin` 规则集中显式注册，否则实现仍不会生效。

测试必须继续放在独立文件 `pkg/planner/cascades/rule/join/join_to_apply_aster_unit_test.rs`，不要嵌入生产源文件。至少应新增：可转换与不可转换 Join、左右孩子顺序、不同 Join 类型/条件、相关列传递、输出 Apply 属性、`remove` 语义、错误传播，以及通过真实 `ApplyRuleTask` 和规则集的端到端接线测试。若同步 Go 行为，也应在同目录新增或扩展独立 Go 测试。

兼容性风险主要是错误改写导致 SQL 语义变化；性能风险包括对不适合的 Join 生成 Apply、扩大候选空间或阻碍索引选择；基础设施风险包括 ID 冲突、规则重复探索和 `Match` 未执行。扩展时应以 Go 的实际增量为准，不凭目标注释补建完整子系统。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录查询识别出源文件、Go 对照、独立 Rust 测试及模块入口。
- RustCodeGraph `node --file pkg/planner/cascades/rule/join/join_to_apply.rs`：核对全部 69 行、8 个符号和两个文件级使用关系。
- RustCodeGraph `query NewJoinToApply` / `query XFJoinToApply`：分别找到 Rust/Go 构造器和类型，并定位独立测试调用。
- 已读生产与装配文件：`join_to_apply.rs`、`join/lib.rs`、`join/Cargo.toml`、`rule/rule.rs`、`rule/rule_type.rs`、`rule/ruleset/rule_set.rs`、`task/task_apply_rule.rs`。
- 已读对照与测试：`join_to_apply.go`、`join_to_apply_aster_unit_test.rs`、`rule/binder_test.rs`、`rule/ruleset/rule_set_aster_unit_test.rs`；`rg` 复核仓库内 `NewJoinToApply`、`XFJoinToApply`、`.XForm` 和默认规则集引用。
- 人工复核结论：文件当前是未注册的占位变换；构造、Pattern、空结果和 Go 对齐均有直接源码/测试依据；尚未实现的 Join→Apply 判定与生成逻辑没有被描述为已支持。
- 本任务为纯文档分析，依计划不运行 Cargo。结构验证命令及结果记录在任务交付证据中。
