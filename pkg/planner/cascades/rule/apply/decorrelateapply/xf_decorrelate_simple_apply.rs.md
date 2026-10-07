# [`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs`](./xf_decorrelate_simple_apply.rs)

## 文件定位

本文件属于 `astersql-planner-cascades-rule-apply-decorrelateapply` 子 crate；其 crate 入口 `lib.rs` 将本模块公开并再导出全部符号。它实现 Cascades 逻辑变换规则 `XFDeCorrelateSimpleApply`：当 `Apply` 的内侧不再引用外侧输出列时，把 `Apply` 降为普通 `Join`，让后续连接规则和物理优化能够处理它。

文件同时保留两套接口。`impl cascades_rule::Rule` 是接入真实 `BoundPlan`、`logicalop` 和 memo 的运行接口；`impl xf_decorrelate_apply_base::Rule` 及固有方法 `xform` 使用相邻基类文件中的轻量计划模型，主要用于机械迁移阶段的独立语义测试。二者实现同一判断和改写意图，但数据类型与元数据恢复方式不同，阅读时不能混为一条调用路径。

规则已经由 `pkg/planner/cascades/rule/ruleset/rule_set.rs` 的 `OperandApplyRulesMap` 和 `OperandApplyRulesList` 构造并注册到 Apply 专用集合；不过 `DefaultRuleSets()` 当前仍为空，因此不能据此声称该规则已在所有默认优化流程中全局启用。

## 核心职责

- `new_xf_decorrelate_simple_apply` 构造 `Apply(Any, Any)`、仅限 TiDB 引擎的匹配模式，并同时初始化轻量 `BaseRule` 与真实 `CascadesBaseRule`。
- 两个 `PreCheck` 路径都拒绝 `NoDecorrelate`/`no_decorrelate` 为真的 Apply，尊重上游禁止解相关的语义标志。
- 两个 `XForm` 路径都以外侧 schema 为边界，从内侧计划抽取相关列；只有集合为空时才生成 Join 替代计划。
- 改写通过浅复制 Apply 内嵌的 `LogicalJoin` 骨架完成，不就地清空 Apply 的相关列状态。源码注释明确指出，就地修改会改变 memo 哈希并要求重新插入 Group。
- 若原 Apply 带 `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG`，返回值中的 `remove` 为真，通知调用者删除这个由解相关链生成的中间 Apply；普通 Apply 则保留原表达式并增加 Join 候选。

## 主要符号

- `XFDeCorrelateSimpleApply { apply_base, cascades_base }`：规则对象。`apply_base` 保存轻量规则 ID、Pattern 和通用预检；`cascades_base` 保存真实 Cascades 的规则类型、名称和 Pattern。
- `new_xf_decorrelate_simple_apply() -> XFDeCorrelateSimpleApply`：Rust 风格构造函数。两套 Pattern 都是根 `Apply` 加两个 `Any` 子模式，且引擎为 TiDB-only。
- `NewXFDeCorrelateSimpleApply()`：Go 风格公开别名，规则集通过此函数构造 trait object。
- 固有 `xform(&GroupExpression)`：轻量模型的可执行改写，用于验证双孩子检查、相关列判断、Join 复制、Cascades 元数据重分配和 `remove` 语义。
- `impl xf_decorrelate_apply_base::Rule`：提供固定 ID `XF_DECORRELATE_SIMPLE_APPLY_ID`（值 2）、轻量 `BaseRule`、共享 `pre_check` 及固有 `xform` 转发。
- `impl cascades_rule::Rule`：真实规则接口。`ID` 返回 `CascadesRuleType as usize`，`String`/`Pattern` 委托 `cascades_base`，`PreCheck` 检查实际 `logicalop::LogicalApply`，`XForm` 操作 `BoundPlan`。
- `ID`、`XForm`：轻量路径的 Go 风格方法别名；不要与真实 trait 上同名方法混淆，调用端的静态类型决定分派目标。

本文件没有模块级可变状态、条件编译项或自定义错误类型；错误类型分别来自相邻轻量基类和 `cascades_rule::RuleError`。

## 执行流程

构造阶段先建立轻量 `Pattern::new(Apply, TiDbOnly)`，挂两个 `Any` 孩子，再用相同形状建立真实 `cascades_pattern::Pattern`。规则集把 `NewXFDeCorrelateSimpleApply()` 放入 Apply 的完整列表和 `XFSetDeCorrelateApply` 子集；带中间标志的组表达式会被规则集过滤器导向后者。

真实 `CascadesRule::XForm` 的流程如下：

1. 从 `BoundPlan::Children()` 取得绑定后的输入组，要求恰好两个；顺序固定为外侧、内侧。
2. 克隆外侧 schema，并调用 `coreusage::ExtractCorColumnsBySchema4LogicalPlan(inner, outer_schema)` 检查内侧仍引用哪些外侧列。
3. 将根计划向下转型为 `logicalop::LogicalApply`；同时读取中间 Apply 标志，浅复制其中的 `LogicalJoin`，并保存原 Apply 的 schema 与 output names。
4. 如果相关列非空，返回空候选与 `remove = false`，不删除原 Apply。
5. 如果相关列为空，在可取得 session context 时重建 Join 的 `BaseLogicalPlan`；随后恢复 schema、output names，调用 `ReAlloc4Cascades("Join")` 重置 Cascades 相关元数据，返回唯一 Join 候选及先前计算的 `remove`。

轻量固有 `xform` 采用相同步骤，但直接匹配 `GroupExpression.children` 的两个元素，通过 `extract_correlated_columns` 检查字符串列名，复制 `logical_join` 后调用 `realloc_for_cascades`。它断言复制后的 Join 仍有孩子；真实路径输出的 Join 本体则不直接携带孩子，因为 `Memo::CopyIn` 会沿绑定输入组重建子关系，`rule_set_aster_unit_test.rs` 对此断言 `join.Children().is_empty()`。

## 数据与状态

规则对象只持有不可变的规则元数据，不在执行间积累状态。关键输入状态来自计划本身：外侧 schema 决定相关列判定范围，`NoDecorrelate` 控制预检，`APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG` 控制成功改写后是否移除原 Apply。

Join 改写必须保留 Apply 作为 Join 时的关系语义和输出契约。真实路径显式保存并恢复 `Schema()` 与 `OutputNames()`；session context 存在时用原查询块偏移构造新的 Join base，再由 `ReAlloc4Cascades` 重分配计划身份与内部缓存。轻量模型的 `realloc_for_cascades` 会分配新 `plan_id`、把类型改成 `Join` 并清空统计信息，其测试还验证原孩子顺序不变。

返回对 `(alternatives, remove)` 是对 memo 调度器的协议：空 alternatives 表示本次不适用；非空 alternatives 是可插入的等价计划；`remove` 只在成功生成 Join 且源 Apply 是解相关中间态时为真。相关列仍存在时即使 Apply 带标志也返回 `false`，避免在没有等价替代计划时删除它。

## 依赖与调用关系

上游直接证据：

- `rule_set.rs::OperandApplyRulesMap` 和 `OperandApplyRulesList` 调用 `NewXFDeCorrelateSimpleApply`，并以 `Box<dyn cascades_rule::Rule>` 保存规则。
- `OperandRules::Filter` 检查 `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG`；中间 Apply 只走 `XFSetDeCorrelateApply` 子集，从而再次尝试本规则。
- `rule_set_aster_unit_test.rs::registered_decorrelation_rule_rewrites_uncorrelated_apply` 经 Pattern binder 取得 `BoundPlan`，再通过 trait object 调用真实 `XForm`，证明注册、绑定和变换链可以贯通。

下游直接依赖由 `Cargo.toml` 声明：`cascades-pattern` 负责 Pattern/Operand/Engine，`cascades-rule` 提供规则 trait、`BoundPlan` 和错误类型，`logicalop` 提供真实 Apply/Join 及标志，`coreusage` 抽取相关列，`cascades-util` 提供规则名称写入接口。相邻 `xf_decorrelate_apply_base.rs` 是 crate 内轻量迁移模型，不是这些真实类型的替代实现。

RustCodeGraph 对目标文件给出的文件级使用者是 `pkg/planner/cascades/rule/binder_test.rs`；该处只验证规则类型 2 的字符串仍为 Go 默认值 `default_none`。名称级调用边还需结合 `rg` 结果解释：规则的生产注册点位于 `rule_set.rs`，而不是 binder 测试。

## 错误处理与边界

- 真实和轻量 `XForm` 都显式拒绝孩子数不为 2 的输入，分别返回 `RuleError` 和轻量 `Error`，错误信息为 “decorrelate Apply requires exactly two children”。
- 真实路径对根节点不是 `LogicalApply` 返回 `RuleError`；轻量路径在 Pattern 应已保证根类型的前提下使用 `expect`，若绕过匹配器传入错误类型会 panic。
- `PreCheck` 同样依赖 Pattern 先保证 Apply：真实路径安全 downcast 并在失败时返回 `false`，轻量共享基类则在错误根类型上 `expect`。
- 相关列非空不是错误，而是规则不适用：返回空替代计划且不移除源表达式。
- 轻量路径断言浅复制的 Join 保留非空孩子；真实路径的孩子由 memo 输入组恢复，所以不使用这一断言。
- `String` 保持 Go `BaseRule.String` 的现状：规则类型 2 显示为 `default_none`。这看似不具描述性，但已有 Rust 回归测试锁定兼容行为，不应在本文件单独更名。

## 并发与资源生命周期

本规则不创建线程、异步任务、锁、通道、事务或外部资源；一次变换仅借用 `BoundPlan`/`GroupExpression` 并返回新计划值。真实路径克隆 schema、output names 和 Join 骨架，避免延长对输入闭包借用的生命周期。

规则对象可由 `OperandApplyRulesMap/List` 每次重新构造，`rule_set.rs` 明确以拥有型 trait object 代替 Go 的包级共享 map，因此没有本文件维护的全局可变规则实例。轻量 `realloc_for_cascades` 内部使用 `AtomicU64` 分配测试模型的 plan ID，但该原子位于相邻基类文件；这里既不拥有也不重置它。真实计划 ID 和缓存生命周期由 `ReAlloc4Cascades` 与 memo 管理。

## 与 Go 版本的对应关系

Go 对照文件是同目录的 `xf_decorrelate_simple_apply.go`。构造 Pattern、固定规则 ID、外/内孩子顺序、相关列抽取、禁止原地修改相关列、浅复制 Join、重新分配 Cascades 元数据，以及中间 Apply 的 `remove` 语义均逐项对应。

Rust 真实实现补足了 Go 代码依赖隐含前置条件的显式错误：Go 直接索引两个孩子并强制类型断言，Rust 对孩子数与根类型返回 `RuleError`。Rust 还显式恢复 schema 与 output names，并在 session context 可用时重建 Join base，以适配当前 Rust `BoundPlan`/`CopyIn` 所有权模型；这不是改变“无相关 Apply 转 Join”的规则语义。

测试状态需要区分：Go 的 `TestXFDeCorrelateShouldDeleteIntermediaryApply` 当前开头调用 `t.Skip`，其完整优化器级断言描述普通 Apply 产生两个 memo 计划、中间 Apply 改写后只剩 Join。Rust 测试文件保留了这段未接线参考，同时另有可执行的轻量测试验证核心语义；`rule_set_aster_unit_test.rs` 还直接验证真实注册规则能把无相关 Apply 变成 Join。因此当前有规则级与注册链证据，但不能把被跳过的 Go 端完整优化器测试写成已恢复通过。

## 扩展指南

- 若扩展可解相关的条件，主要修改真实 `CascadesRule::XForm`，并同步轻量固有 `xform`，保证两者的相关列判定、候选数量和 `remove` 协议一致；不要仅改测试桩路径。
- 若改变 Pattern 或规则 ID，同时更新轻量/真实两个 base、`rule_type.rs`、`ruleset` 注册与过滤测试。Pattern 必须继续保证孩子顺序和根类型，否则现有类型假设会失效。
- 若新增 Join 元数据，真实路径应在浅复制后明确保留或重建，并检查 `ReAlloc4Cascades` 会清空哪些字段；轻量模型及独立测试也应表达同样的不变量。
- 若调整中间 Apply 标志语义，必须联动 `rule_set.rs::OperandRules::Filter`，确保只有存在等价 Join 时才请求移除旧表达式。
- 回归测试应放在独立文件：规则局部语义扩展同目录 `xf_decorrelate_apply_test.rs`，真实注册/绑定行为扩展 `pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs`；不要把测试嵌入生产 `.rs` 文件。
- 性能上应避免遍历无关子树或深复制完整计划；正确性上重点检查 schema、output names、query block offset、孩子顺序、统计/任务缓存重置和 memo 哈希稳定性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录下识别出 Rust/Go 实现、基类及两侧测试。
- RustCodeGraph `query`：定位 `XFDeCorrelateSimpleApply`、`new_xf_decorrelate_simple_apply`、`NewXFDeCorrelateSimpleApply`；`node --file` 完整读取目标文件，并读取 Go 对照、轻量基类、Rust 测试、Go 测试、规则集及其真实接口测试、binder 字符串测试。
- 源与配置路径：`xf_decorrelate_simple_apply.rs`、`xf_decorrelate_apply_base.rs`、`lib.rs`、同目录 `Cargo.toml`、`xf_decorrelate_simple_apply.go`。
- 测试路径：同目录 `xf_decorrelate_apply_test.rs` 与 `xf_decorrelate_apply_test.go`，以及 `pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs`、`pkg/planner/cascades/rule/binder_test.rs`。
- 关键调用边经 RustCodeGraph 与精确文本检索交叉核对：`OperandApplyRulesMap/List → NewXFDeCorrelateSimpleApply`，构造别名转发到 `new_xf_decorrelate_simple_apply`，真实规则测试经 `NewBinder → Rule::XForm` 产生 Join；轻量 `Rule::xform` 转发到固有 `xform`，后者调用 `extract_correlated_columns`、`shallow_ref`、`realloc_for_cascades`。
- 本任务是纯文档分析，按计划未运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查，并人工核对“存在原因、运行流程、安全扩展”三项均有源码或测试依据。
