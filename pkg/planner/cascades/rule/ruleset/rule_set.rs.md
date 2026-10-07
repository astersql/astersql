# `pkg/planner/cascades/rule/ruleset/rule_set.rs`

## 文件定位

本文件实现 Cascades 变换规则的“按算子归组、按子集选择、按规则 ID 过滤”数据结构，是 Go 包 `pkg/planner/cascades/rule/ruleset` 的 Rust 对照实现。crate 入口 `pkg/planner/cascades/rule/ruleset/lib.rs` 私有加载 `rule_set` 模块并公开再导出全部符号；`Cargo.toml` 将其定义为 `astersql-planner-cascades-rule-ruleset`，依赖 memo、pattern、rule、Apply 解相关规则和逻辑算子五个相邻 crate。

当前接线状态必须与功能实现分开理解：该文件的类型、工厂函数和行为测试均已存在，但仓库内生产 Rust 代码没有引用这个 crate；根 `Cargo.toml` 仅用 `facade_planner_cascades_rule_ruleset` 将其列为 workspace facade。Rust 的 `pkg/planner/cascades/task/task_opt_group_expression.rs::getValidRules` 目前通过 `Context::RulesFor` 和 `RuleEnabled` 选规则，并未走这里的 `DefaultRuleSets`/`OperandRules::Filter`。相对地，Go 的 `pkg/planner/cascades/task/task_opt_group_expression.go::getValidRules` 直接消费本包的默认映射与两级过滤。

## 核心职责

1. 用 `SetType`、`DefaultNone` 和 `XFSetDeCorrelateApply` 标识规则子集；目前唯一有实际成员的子集是 Apply 解相关子集。
2. 用 `ListRules` 拥有 `Box<dyn Rule>` 的有序列表，提供长度、借用迭代和按 `RuleMask` 过滤，过滤结果保持注册顺序。
3. 用 `OperandRules` 同时保存某个 Operand 的完整规则列表和按 `SetType` 索引的子集；遇到带 `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG` 的中间 Apply 时，只返回解相关子集，避免应用无关规则。
4. 通过 `OperandApplyRulesMap`、`OperandApplyRulesList`、`OperandApplyRules` 构造 Apply 的规则配置；子集和完整列表都注册 `NewXFDeCorrelateSimpleApply()`。
5. 通过 `DefaultRuleSets` 提供 Operand 到规则集合的默认映射。它当前返回空 `HashMap`，与 Go 文件中被注释的 Apply 注册一致，因此不能据此声称默认优化主链已启用 Apply 规则。

## 主要符号

- `pub type SetType = usize`：规则子集键。`DefaultNone = 0` 是默认/未指定值，`XFSetDeCorrelateApply = 1` 是解相关 Apply 子集。常量命名保留 Go 风格，由 `lib.rs` 的 lint allow 支持。
- `DefaultRuleSets() -> HashMap<Operand, OperandRules>`：每次调用创建空映射；返回拥有型值，没有共享全局可变状态。
- `RuleMask(BTreeSet<usize>)`：允许列表式掩码。`New` 去重并排序规则 ID，`Test` 做成员查询。这里的“mask out”实际语义是保留集合内 ID，而不是排除集合内 ID，见 `ListRules::Filter` 和测试 `list_filter_preserves_order_and_rule_ids`。
- `ListRules(Vec<Box<dyn Rule>>)`：拥有异构规则对象。`New` 接管规则列表，`Len` 返回元素数，`Iter` 以 `&dyn Rule` 借用迭代，`Filter` 返回借用切片式结果 `Vec<&dyn Rule>`，不复制或转移规则。
- `OperandRules { setMap, setList }`：字段私有，强制调用方通过构造函数建立完整列表与子集。`NewOperandRules` 只组装数据，不验证 map/list 一致性。
- `OperandRules::Filter(&GroupExpressionRef) -> &ListRules`：读取组表达式包装的逻辑计划基座并检查标志；有标志时取 `XFSetDeCorrelateApply` 子集，否则取完整列表。
- `OperandApplyRulesMap()` / `OperandApplyRulesList()`：分别创建解相关子集映射和完整列表。二者各自新建规则对象，但注册的是同一规则类型且 ID 都为 2；这由 `apply_rule_sets_register_the_decorrelation_rule` 验证。
- `OperandApplyRules()`：以以上两个工厂函数调用 `NewOperandRules`，形成可供表达式级选择的完整配置。

## 执行流程

按本文件设计，完整的选择链分两层：先按表达式状态选规则列表，再按会话/配置允许的 ID 过滤。

1. 调用方先按根算子的 `Operand` 找到一个 `OperandRules`。Go 主链在 `task_opt_group_expression.go::getValidRules` 中从 `DefaultRuleSets` 查找；当前 Rust 主链尚未调用本文件。
2. `OperandRules::Filter` 短暂不可变借用 `GroupExpressionRef`，经 `GetWrappedLogicalPlan().base().HasFlag(...)` 判断计划是否带解相关中间态标志。
3. 普通表达式直接借用 `setList`。带标志的中间 Apply 则从 `setMap[XFSetDeCorrelateApply]` 借用短路子集，使后续只尝试解相关规则。
4. 若继续调用 `ListRules::Filter`，它按注册顺序迭代每条规则，以 `rule.ID()` 查询 `RuleMask::Test`，只收集允许的规则引用。因此筛选稳定，且规则对象仍归原列表所有。
5. Apply 配置中的唯一规则由 `NewXFDeCorrelateSimpleApply` 构造。其真实 `Rule::XForm` 在 `xf_decorrelate_simple_apply.rs` 中检查两个孩子与相关列：没有相关列时将 Apply 改写为 Join；仍有相关列时不产出候选；原 Apply 带中间态标志时返回 `remove = true`。

## 数据与状态

- `DefaultRuleSets` 使用 `HashMap`，因为按 `Operand` 查找，不承诺迭代顺序；当前为空。
- `OperandRules::setMap` 使用 `BTreeMap`，使子集键顺序确定。当前只含键 1。`setList` 与 map 中的列表分别拥有独立规则实例，并非共享指针。
- `RuleMask` 使用 `BTreeSet`，构造时自动去重；成员测试的语义是“启用”。其有序性不改变 `ListRules::Filter` 的输出顺序，输出仍由原规则列表决定。
- `ListRules` 是唯一拥有规则 trait object 的容器；`Iter`、`Filter` 和 `OperandRules::Filter` 都只返回受宿主生命周期约束的借用，不产生悬空所有权或额外克隆。
- 表达式状态来自 `BaseLogicalPlan` 的 `u64` flags；`APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG` 定义为 `1 << 0`。本文件只读取该位，不修改 memo、表达式或计划。
- 所有工厂函数按调用创建新容器和规则对象，没有进程级缓存。代价是重复分配，但消除了 Go 包级 map/slice 的共享对象生命周期。

## 依赖与调用关系

直接下游依赖如下：

- `cascades_pattern::Operand`：`DefaultRuleSets` 的键类型。
- `cascades_rule::Rule`：规则对象接口；本文件依赖 `ID` 进行掩码过滤，并通过 trait object 保存异构规则。
- `cascades_memo::GroupExpressionRef`：表达式级规则子集选择的输入；内部可变性由引用类型负责，本文件仅进行短时不可变借用。
- `logicalop::APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG` 与逻辑计划 `base().HasFlag`：识别解相关过程中生成的中间 Apply。
- `decorrelateapply::NewXFDeCorrelateSimpleApply`：当前唯一注册的具体规则。
- 标准库 `HashMap`、`BTreeMap`、`BTreeSet`：分别承担 Operand 映射、确定性子集映射和规则 ID 集合。

RustCodeGraph 对精确符号和仓库文本搜索都表明，当前直接上游调用只在同 crate 的 `rule_set_aster_unit_test.rs`：测试调用全部工厂、两种 `Filter` 以及规则的 `Pattern`/`XForm`。生产 Rust 优化路径 `task_opt_group_expression.rs::getValidRules` 是并行实现，不是本文件调用方。Go 上游则是 `task_opt_group_expression.go::getValidRules`，其链路为 `DefaultRuleSets[operand] -> OperandRules.Filter(expression) -> ListRules.Filter(context mask) -> NewApplyRuleTask`。

## 错误处理与边界

- `DefaultRuleSets` 为空是当前实现事实，不是错误。调用方必须处理 Operand 未注册；Go 任务在查找失败时返回 `nil`，Rust 当前主链由另一套 Context API 处理。
- `OperandRules::Filter` 对“表达式有中间态标志但 map 未注册解相关子集”使用 `expect("de-correlate Apply subset must be registered")`，会 panic。构造自定义 `OperandRules` 时必须维持不变量：只要可能传入带该标志的表达式，map 就必须含 `XFSetDeCorrelateApply`。
- `NewOperandRules` 不检查 map/list 是否包含相同规则、是否为空或 ID 是否重复；一致性由注册者和测试负责。
- `ListRules::Filter` 对空列表、空 mask 和未知 ID 都安全，结果为空；重复 mask ID 被 `BTreeSet` 消除。列表若含重复规则 ID，则每个匹配元素都会保留。
- `OperandRules::Filter` 实际只检查基座 flag，没有再次验证根 Operand 确为 Apply。因此上游必须先按 Operand 正确分派，否则任何误带该 flag 的逻辑计划也会触发短路分支。
- 具体解相关规则的形状和转换错误不在本文件产生；其 `XForm` 可返回“孩子数不是 2”或“根不是 LogicalApply”的 `RuleError`。本文件只是注册该规则。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。工厂函数返回拥有型容器，天然限定规则对象生命周期；过滤结果是对容器的不可变借用，不能脱离容器存活。

`GroupExpressionRef` 具有内部可变性，`OperandRules::Filter` 只在局部块内调用 `borrow()`，在返回规则列表前释放表达式借用，不会把 `Ref` 泄漏到调用方，也不会跨规则执行保持借用。并发安全没有在本文件中声明：`Box<dyn Rule>` 未附加 `Send + Sync`，`GroupExpressionRef` 的实际引用模型也由 memo crate 决定，因此不应把这些规则集合跨线程共享，除非上游接口另行证明安全。

规则工厂每次创建独立的 `Box<dyn Rule>`，所以不存在包级单例的并发修改问题；相应地，调用方若频繁重建集合会重复分配，应在真正接入生产主链时明确集合的缓存/所有权边界。

## 与 Go 版本的对应关系

核心语义逐项保持：Go `SetType uint` 对应 Rust `usize`；两个常量值仍为 0/1；Go `ListRules.Filter(bitset)` 和 Rust `ListRules::Filter(RuleMask)` 都保留 mask 中的 ID 并维持注册顺序；Go `OperandRules.Filter` 与 Rust 同样对带解相关标志的表达式返回专用子集；Apply map/list 都各注册一个 `NewXFDeCorrelateSimpleApply`。

所有权实现有意不同。Go 使用包级 `var`、slice 和接口引用，共享 `OperandApplyRulesMap/List`；Rust 用函数每次返回拥有型 `HashMap`/`BTreeMap`/`ListRules`，避免全局可变状态。Rust `ListRules::Filter` 返回 `Vec<&dyn Rule>`，而 Go 返回新的接口 slice；二者都不克隆具体规则。

默认注册状态一致：Go `DefaultRuleSets` 的 `OperandApply` 行被注释，Rust `DefaultRuleSets()` 返回空 map。不过主链迁移状态不同：Go `OptGroupExpressionTask.getValidRules` 确实读取 ruleset；Rust 同名任务改读 Context，且本 crate 没有生产调用方。因此本文件目前是可测试的移植组件/边界实现，不是 Rust 优化器实际规则分派的唯一事实来源。

测试对应方面，Rust 独立测试 `rule_set_aster_unit_test.rs` 覆盖了 Go 文件自身没有专门测试覆盖的容器不变量，并进一步调用注册规则完成一次无相关列 Apply 到 Join 的真实改写。具体解相关行为还分别由 `xf_decorrelate_apply_test.go` 和 `xf_decorrelate_apply_test.rs` 覆盖，但它们测试的是下游规则而非本文件的集合接线。

## 扩展指南

- 新增规则子集时，先增加不会冲突的 `SetType` 常量，再在相应 Operand 的 map 工厂注册；若选择条件不只是当前 flag，应扩展 `OperandRules::Filter`，并在 `rule_set_aster_unit_test.rs` 增加普通路径、命中路径和缺失注册边界测试。
- 给现有 Operand 增加规则时，要同步其完整列表与所需子集，保持规则顺序和 ID 一致；`NewOperandRules` 不会自动校验。至少扩展 `apply_rule_sets_register_the_decorrelation_rule` 一类测试，验证 map/list 的规则 ID 和顺序。
- 启用 `DefaultRuleSets` 中的 Apply 或其他 Operand 前，必须先决定 Rust 生产主链的唯一规则来源：接回本 crate，或将规则同步到 `Context::RulesFor` 的注册处。不能只修改空 map 并声称规则已生效，因为当前 `task_opt_group_expression.rs` 不读取它。
- 修改 mask 语义时要避免名称误导：当前集合表示允许 ID。若改为排除掩码，会影响筛选结果和 Go 对齐，必须同步 `list_filter_preserves_order_and_rule_ids` 及任务层测试。
- 若允许并行优化，需要先在 `Rule` trait、规则实现、memo 引用类型和集合缓存层整体证明 `Send`/`Sync`；不能只在此处替换容器。
- 测试继续放在独立文件 `pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs`，不要嵌入生产源文件。具体规则变换的边界测试则同步到 `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs`。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；索引包含 `rule_set.rs`、`lib.rs`、Go 对照和独立 Rust 测试。
- RustCodeGraph 源码/符号核验：`rule_set.rs` 全部 141 行；主要符号为 `DefaultRuleSets`、`OperandApplyRulesMap`、`OperandApplyRulesList`、`RuleMask`、`ListRules`、`OperandRules`、`NewOperandRules`、`OperandApplyRules` 和两个 `Filter` 实现。
- crate 边界：`pkg/planner/cascades/rule/ruleset/Cargo.toml`、`lib.rs`、根 `Cargo.toml` 的 facade 条目。
- Go 对照与生产调用链：`pkg/planner/cascades/rule/ruleset/rule_set.go`、`pkg/planner/cascades/task/task_opt_group_expression.go::getValidRules`。
- Rust 当前生产选择链：`pkg/planner/cascades/task/task_opt_group_expression.rs::getValidRules`；仓库搜索未发现生产 Rust 对本文件公开符号或 ruleset crate 的引用。
- 下游状态与行为：`pkg/planner/core/operator/logicalop/base_logical_plan.rs::APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG`、`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs::NewXFDeCorrelateSimpleApply`/`Rule::XForm`。
- 独立测试：`pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs` 的四个测试分别验证顺序与 mask、普通/中间 Apply 分流、map/list 注册一致性、实际 Apply→Join 改写。按任务约束，本次是纯文档分析，未运行 Cargo；仅将测试源码作为行为证据。
