# `pkg/planner/core/rule/rule_constant_propagation.rs`

## 文件定位

本文件位于逻辑优化规则 crate `astersql-planner-core-rule`，由同目录 `lib.rs` 以公开模块 `rule_constant_propagation` 导出。它在 `rule_init.rs` 定义的精简逻辑计划 IR（`Plan`、`PlanKind`、`Expr`、`JoinType`）之上实现跨查询块常量谓词上拉：从 Join 子树中的 Selection（可穿过可消除的 Projection）找到“列与常量比较”谓词，在 Join 上方包一层新的 Selection，供后续谓词下推把约束传播到 Join 另一侧。

`rule_init::default_rule_names()` 把字符串 `constant_propagation` 放在默认规则名序列中、位于 `predicate_simplification` 之后。但在当前 Rust 代码中，仓库搜索只找到测试直接构造 `ConstantPropagationSolver`，没有找到生产路径实例化该类型或把规则名解析为本类型的工厂。因此，本文件已提供可调用的规则实现和模块导出，不能仅凭默认名字列表断言它已接入生产优化器执行链。实际生产逻辑计划上的同类入口还可见于 `pkg/planner/core/optimizer_runtime.rs`；那不是本文件使用的精简 `Plan` 类型。

## 核心职责

- `ConstantPropagationSolver` 实现 `LogicalRule`，暴露稳定名称 `constant_propagation`，并以一次递归改写返回新计划。
- `propagate` 对每个节点先读取当前 Join 的候选谓词，再递归改写原有子节点，最后在当前 Join 外包 Selection。这一顺序对应 Go 版本注释中的前序处理语义，同时以值所有权重建 Rust 计划树。
- `join_candidates` 根据 Join 类型限制候选来源：Inner 读取两侧，LeftOuter 只读取左侧保留端，RightOuter 只读取右侧保留端，Semi/AntiSemi 不处理。
- `pull_up_constant_predicates` 只从 Selection 直接筛选候选，或递归穿过满足形状约束的 Projection 并把列 ID 改写为 Projection 输出列 ID；其他节点形成边界。
- `valid_compare_constant_predicate` 把候选限定为二元 `eq/lt/le/gt/ge`，且一端是列、另一端是常量。

这里的“传播”不是直接替换整棵表达式树中的等价列，也不直接把谓词压入 Join 另一侧；本文件只完成候选上拉和新 Selection 构造。Go 文件 `rule_constant_propagation.go` 明确把后续传播归于 predicate push down 规则。

## 主要符号

- `pub struct ConstantPropagationSolver`：无字段的规则标记类型，是本文件唯一公开定义。
- `impl LogicalRule for ConstantPropagationSolver`：
  - `name(&self) -> &'static str` 返回 `constant_propagation`，与 Go 的 `Name()` 及默认规则名一致。
  - `optimize(&self, plan: Plan) -> Result<(Plan, bool), String>` 调用 `propagate`。它总是返回 `Ok`，且即使插入 Selection，`changed` 仍为 `false`，对应 Go `Optimize` 中始终未置真的 `planChanged`。
- `fn propagate(mut plan: Plan) -> Plan`：递归入口。它保存当前节点候选，递归消费并替换 `children`；存在候选时构造 Selection，复制原计划的 `schema`、`keys`、`estimated_rows`、`used_stats`，并把改写后的原节点作为唯一子节点。
- `fn join_candidates(plan: &Plan) -> Vec<Expr>`：只识别 `PlanKind::Join`，根据 `JoinType` 选择最多两个子节点并收集候选。
- `fn pull_up_constant_predicates(plan: &Plan) -> Vec<Expr>`：Selection 分支克隆合法谓词；Projection 分支构造“输入列 ID -> 输出列 ID”的 `BTreeMap`，递归取得候选并逐个改列。
- `fn valid_compare_constant_predicate(expression: &Expr) -> bool`：候选形状守卫。
- `fn replace_column(expression: &mut Expr, source: i64, target: i64)`：递归遍历 `Column`、`Scalar.args` 和 `Cast.expr`；常量不变。当前调用方只传入已经验证为单列比较的候选。

本文件没有模块级常量、枚举、条件编译项或额外 trait 定义。

## 执行流程

1. 调用者构造 `ConstantPropagationSolver`，通过 `LogicalRule::optimize` 传入拥有所有权的 `Plan`。
2. `optimize` 调用 `propagate`，并把结果包装为 `Ok((plan, false))`；本实现没有可产生 `Err` 的分支。
3. `propagate` 先用 `join_candidates(&plan)` 检查当前节点。非 Join 立即得到空候选；Join 则按类型选择允许读取的一侧或两侧。
4. 对被选中的子节点调用 `pull_up_constant_predicates`：
   - Selection 仅保留 `eq/lt/le/gt/ge` 的列-常量二元比较；
   - Projection 只有在表达式数量等于输出 schema 数量且恰有一个子节点时才可穿过。映射仅收录 Projection 表达式为纯 `Expr::Column` 的位置；候选的唯一列不在映射中时丢弃；
   - 其他计划节点返回空集，递归上拉在此终止。
5. 当前节点候选确定后，`propagate` 递归改写全部子节点。因为候选是在递归前从原子树提取的，本层不会重复吸收本轮递归新建的 Selection。
6. 无候选时原样返回改写过子节点的当前计划；有候选时，以当前计划为唯一子节点新建 Selection。多个候选保持子节点遍历顺序及各 Selection 内的谓词顺序，不去重。
7. 上层递归继续处理，最终返回可能增加若干 Selection 的整棵树。

例如独立测试构造左侧含 `a > 1` 的 Inner Join：规则把 `a > 1` 克隆到 Join 上方的新 Selection，原左侧 Selection 仍保留；若中间有 `Projection(a AS a')`，候选列 ID 会从输入 ID 改成输出 ID。

## 数据与状态

- 输入输出计划都使用 `rule_init::Plan` 的值语义。`optimize` 消费输入，递归过程中不使用共享引用或内部可变性。
- 候选谓词是 `Vec<Expr>`。Selection 分支克隆原谓词，所以原子树条件不会被删除；Projection 分支只改写克隆后的候选。
- Projection 映射使用 `BTreeMap<i64, i64>`，键是 Projection 中纯列表达式的源列 ID，值是对应 `plan.schema` 输出列 ID。确定性有序容器不影响语义，只提供稳定映射行为。
- 新 Selection 继承被包裹 Join 的 `schema`、`keys`、`estimated_rows` 和 `used_stats`，谓词来自候选，子节点只有该 Join。统计信息没有重新估算，这是精简 IR 中的直接复制。
- `Expr::columns()` 返回集合；Projection 分支取其中第一个列。安全性依赖候选先经过 `valid_compare_constant_predicate`，该守卫保证表达式只有一个 `Column` 操作数和一个常量操作数。
- 规则本身无字段，无跨调用缓存、全局变量或注册状态。

## 依赖与调用关系

直接依赖只有 `crate::rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind}` 与标准库 `BTreeMap`。`Cargo.toml` 表明本文件属于 `astersql-planner-core-rule`，crate 根为 `lib.rs`；该 crate 的普通依赖包含 meta model、logicalop 和 rule-util，但本文件没有直接引用这些外部 crate。

内部调用链为：

`LogicalRule::optimize` → `propagate` → `join_candidates` → `pull_up_constant_predicates` → `valid_compare_constant_predicate` / `replace_column`。

其中 `propagate`、`pull_up_constant_predicates` 和 `replace_column` 都有自递归边：前者沿计划子树，第二个沿 Projection 链，第三个沿表达式树。RustCodeGraph 能确认上述被调用边；其泛型/标准库解析还把 `collect`、`len`、`get` 指向仓库内同名定义，这些是索引歧义，不能视为真实跨模块业务依赖。

上游证据分两层：`lib.rs` 公开导出模块，`rule_init::default_rule_names` 声明规则名字顺序；直接类型调用只在 `rule_constant_propagation_test.rs` 中被找到。另有 `optimizer_logical_entry_aster_unit_test.rs` 通过 `FLAG_CONSTANT_PROPAGATION` 验证真实逻辑计划优化入口，但它调用的是 `optimizer_runtime::LogicalOptimizeForTest`，不是本文件的 `LogicalRule::optimize`。

## 错误处理与边界

- `optimize` 的签名允许 `String` 错误，但当前实现始终 `Ok`，没有验证错误或运行期错误传播。
- 非 Join、Semi、AntiSemi、没有候选的 Join 都保持节点形状不变（子节点仍会递归处理）。Go 当前 switch 的 default 同样不为这些 Join 类型创建 Selection。
- LeftOuter 只上拉左侧，RightOuter 只上拉右侧，避免从非保留端上拉谓词改变 NULL 扩展语义。
- Selection 只接受函数名严格为小写 `eq/lt/le/gt/ge`、参数数目恰为 2、且参数形状为列-常量或常量-列的 `Expr::Scalar`。`ne`、多列表达式、常量-常量、嵌套算术、非 Scalar 都被拒绝。
- Projection 必须满足表达式数等于 schema 数且只有一个子节点；只为纯列 Projection 项建立映射。计算表达式、缺失输出列映射或候选无列时丢弃该候选。
- `join_candidates` 对 Inner Join 只查看前两个子节点；外连接用 `first()` / `get(1)` 安全处理缺失子节点，不会 panic。正常 Join 的二叉结构由调用者负责保证。
- 新 Selection 不删除下层原谓词，也不去重；后续规则或执行链应承担下推与可能的规范化工作。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。一次优化的所有状态都位于栈上或由 `Plan`、`Vec`、`BTreeMap` 拥有；递归返回后临时映射和候选按 Rust 所有权规则释放。

规则类型是无状态零大小类型，从实现本身看可独立并发调用，因为每次调用都消费各自的 `Plan`，没有共享可变状态。不过本文件未显式实现并发调度，也没有声明递归深度上限；计划树、Projection 链或表达式树极深时，递归栈深度是资源风险。`plan.clone()` 在包 Selection 时会复制当前 Join 子树，可能带来与子树大小相关的内存和 CPU 开销；这是扩展或接入生产链时应重点评估的性能点。

## 与 Go 版本的对应关系

Rust `ConstantPropagationSolver::{name,optimize}` 对应 Go `rule_constant_propagation.go` 的 `Name`、`Optimize`；二者名称一致，且 Go 的 `planChanged` 始终为 `false`，Rust 测试显式锁定该行为。Go `Optimize` 先对根调用 `ConstantPropagation(nil, 0)`，再递归子节点；Rust `propagate` 也先提取当前节点候选，再递归子节点，保持“当前节点优先”的意图。

Go 把节点特定逻辑分散在逻辑算子方法中：

- `LogicalJoin.ConstantPropagation` 按 Join 类型选择子侧并调用 `PullUpConstantPredicates`，再由 `addCandidateSelection` 接入父节点；Rust 集中为 `join_candidates` 和 `propagate`，通过值重建在 Join 外包 Selection。
- `LogicalSelection.PullUpConstantPredicates` 调用表达式层 `ValidCompareConstantPredicate`；Rust 用本地 `valid_compare_constant_predicate` 对精简 `Expr` 做形状判断。
- `LogicalProjection.PullUpConstantPredicates` 先要求 `canProjectionBeEliminatedLoose`，按表达式哈希映射列并用 rule-util 改写克隆表达式；Rust 用“表达式数匹配 schema、单子节点、Projection 项为纯列”的局部条件和列 ID 映射近似该语义。

因此两者核心意图、Join 侧选择、比较谓词筛选、Projection 列改写和 `changed=false` 已对齐，但 Rust 文件基于精简 IR，缺少 Go 的 session/eval context、完整表达式哈希、父节点原位接线以及所有真实 LogicalPlan 类型。不能把本文件视为 Go 生产实现的逐类型完整替代。Go 注释还明确后续由 predicate push down 把上拉谓词传播到另一侧；Rust 本文件没有执行该后续阶段。

## 扩展指南

- 新增可上拉比较形式时，优先修改 `valid_compare_constant_predicate`，并在独立文件 `rule_constant_propagation_test.rs` 增加接受与拒绝用例；需同时核对 Go `expression.ValidCompareConstantPredicate` 的语义，尤其 NULL、类型转换、非确定函数和复合表达式，不能只扩充函数名列表。
- 支持新的 Join 类型时，修改 `join_candidates`。必须先证明从哪一侧上拉不改变外连接/半连接的行保留与 NULL 语义，并增加每种 Join 类型两侧谓词的回归测试。
- 扩大可穿透算子范围时，在 `pull_up_constant_predicates` 添加明确分支；Projection 扩展应保持“一条候选恰含一个可映射列”的不变量，并覆盖缺列、计算表达式、重复列、schema 长度不匹配和多子节点。
- 若将本规则接入生产优化器，需要在规则名到实现的构造/调度处增加真实接线，而不只是编辑 `default_rule_names`；同时增加从优化器入口触发本类型的测试，以弥补当前只有直接单测的证据缺口。
- 若改变新 Selection 的元数据继承方式，修改 `propagate` 并验证 schema、keys、统计信息及估算行数的契约。当前整棵 `plan.clone()` 有潜在复制成本，优化所有权重建时必须保持递归后子树和候选提取时机一致。
- Rust 测试必须继续放在独立的 `rule_constant_propagation_test.rs`，不要内嵌回生产文件；如对齐 Go 行为有变化，还应同步检查 `rule_constant_propagation.go`、`logical_join.go`、`logical_selection.go`、`logical_projection.go` 及真实优化入口测试。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标 Rust 文件可按文件节点读取。
- 目标实现：`pkg/planner/core/rule/rule_constant_propagation.rs`，符号查询确认 `ConstantPropagationSolver`、`propagate`、`join_candidates`、`pull_up_constant_predicates`、`valid_compare_constant_predicate`、`replace_column`；callees 查询确认核心内部调用边和递归关系。
- 公共 IR 与规则顺序：`pkg/planner/core/rule/rule_init.rs` 中的 `Expr`、`JoinType`、`PlanKind`、`Plan`、`LogicalRule`、`default_rule_names`。
- crate 边界与模块导出：`pkg/planner/core/rule/Cargo.toml`、`pkg/planner/core/rule/lib.rs`。
- 独立 Rust 单测：`pkg/planner/core/rule/rule_constant_propagation_test.rs`，覆盖 Inner Join 上拉及 `changed=false`、LeftOuter 保留侧、Semi 不改写、Projection 列映射和拒绝 `ne`。
- Rust 真实优化入口旁证：`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 的 `constant_propagation_pulls_derived_table_predicate_above_join`，验证 `FLAG_CONSTANT_PROPAGATION` 路径会在真实逻辑计划上增加 Selection；该测试不直接调用本文件类型。
- Go 对照：`pkg/planner/core/rule/rule_constant_propagation.go` 的 `Optimize`、`execOptimize`、`Name`；`pkg/planner/core/operator/logicalop/logical_join.go` 的 `ConstantPropagation`；`logical_selection.go` 与 `logical_projection.go` 的 `PullUpConstantPredicates`。
- 调用搜索限制：RustCodeGraph 未给出本文件公开类型的生产 caller；仓库 `rg` 搜索同样只发现模块导出、默认名字和测试直接调用。因此文档把生产接线标为未验证，而非推测已支持。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证在文档生成后单独执行。
