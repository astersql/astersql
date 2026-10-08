# `pkg/planner/core/rule_generate_column_substitute.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 边界由 `pkg/planner/core/Cargo.toml` 定义，模块由 `pkg/planner/core/lib.rs:96` 以 `pub mod rule_generate_column_substitute` 暴露。它实现一套基于轻量 `task::PlanNode` / `task::Expression` 模型的生成列表达式替换规则：先从计划树中的数据源收集“生成列表达式名 → 列下标”，再把计划其他位置出现的同名表达式改成列引用。

当前 Rust 接线应按代码事实理解：仓库搜索只找到模块声明和 `pkg/planner/core/rule_generate_column_substitute_test.rs` 中的直接调用，没有找到把 `GcSubstituter` 加入 Rust 生产优化器规则序列的代码。相比之下，Go 实现在 `pkg/planner/core/optimizer.go:88-90` 的 `optRuleList` 首位注册。因此，本文件目前是可公开调用且有独立测试覆盖的移植模块，但不能仅凭模块公开就断言它已进入 Rust SQL 优化主链。

## 核心职责

核心职责分为两个阶段，对应 `GcSubstituter::Optimize`：

1. `collectGenerateColumn` 后序遍历计划树，跳过整个 `PlanKind::Cte` 子树，只在 `PlanKind::Other("DataSource")` 节点收集 `virtual_column == true` 且具有 `column` 下标的表达式。
2. 映射非空时，`GcSubstituter::substitute` 遍历整棵计划树，在每个节点的通用表达式容器中尝试替换；规则即使实际改写了表达式，也按 Go 规则契约从 `Optimize` 返回 `false` 作为 `planChanged`。

替换的直接效果由 `tryToSubstituteExpr` 定义：大小写不敏感地比较表达式名称，匹配后写入 `Expression.column = Some(column)`，并把 `Expression.name` 规范化为 `col_<下标>`。该 Rust 实现依赖名称字符串表示表达式等价性，不是 Go 版本的结构化表达式等价判断。

## 主要符号

- `pub type ExprColumnMap = HashMap<String, usize>`：以 ASCII 小写表达式名为键、`PlanNode` 列下标为值。重复键使用 `HashMap::insert` 的后写覆盖语义；遍历顺序决定最终值。
- `pub struct GcSubstituter`：无字段、可 `Default` 构造的规则对象，不保存跨调用状态。
- `GcSubstituter::Optimize(&self, PlanNode) -> (PlanNode, bool)`：取得计划所有权，执行收集与替换，返回改写后的计划和固定为 `false` 的变化标志。
- `GcSubstituter::substitute(&self, &mut PlanNode, &ExprColumnMap) -> bool`：递归改写单棵计划树，并在内部准确累计是否发生替换；当前 `Optimize` 有意忽略该返回值。
- `GcSubstituter::Name(&self) -> &'static str`：返回固定规则名 `generate_column_substitute`。
- `collectGenerateColumn(&PlanNode, &mut ExprColumnMap)`：收集阶段入口，CTE 是硬边界，DataSource 是唯一采集节点种类。
- `tryToSubstituteExpr(&mut Expression, &str, usize) -> bool`：单个候选的匹配和就地改写原语。
- `SubstituteExpression(&mut Expression, &ExprColumnMap) -> bool`：`substituteExpression` 的公开兼容别名；独立测试和基准风格调用可以使用该入口。
- `substituteExpression(&mut Expression, &ExprColumnMap) -> bool`：把条件名转为 ASCII 小写后进行一次哈希查找，再委托给 `tryToSubstituteExpr`。

文件没有模块级常量、trait、条件编译项或异步入口。

## 执行流程

`Optimize` 的完整流程如下：

1. 创建空 `HashMap`。
2. 对输入计划调用 `collectGenerateColumn`。每到一个非 CTE 节点，先递归子节点，再判断自身是否为字符串恰好等于 `DataSource` 的 `PlanKind::Other`；仅该类型节点继续扫描 `expressions`。
3. 对每个虚拟列表达式，只在 `column` 为 `Some` 时，把 `name.to_ascii_lowercase()` 和列下标放入映射。没有列下标的候选被静默忽略。
4. 若映射为空，直接返回原计划；否则调用 `substitute`。
5. `substitute` 在当前节点依次串接 `expressions`、`conditions`、`by_items`、`group_items`、`agg_funcs`，对每个元素调用 `substituteExpression`，然后递归所有 `children`。
6. 返回计划，变化标志始终为 `false`。

`substituteExpression` 不遍历表达式内部结构。它只处理传入的单个 `Expression.name`；因此所谓递归发生在计划树层面，不发生在标量表达式树层面。`pkg/planner/core/rule_generate_column_substitute_test.rs` 分别覆盖 Selection 条件、聚合函数、DataSource/CTE 收集边界、大小写归一化和固定变化标志。

## 数据与状态

规则的持久状态为空；`GcSubstituter` 是零大小对象。每次 `Optimize` 都新建局部 `ExprColumnMap`，并取得、返回一个独立的 `PlanNode` 值。计划树的数据结构来自 `pkg/planner/core/task.rs:73-81,91-126,150-175`：

- `Expression.name` 同时承担表达式标识和替换后的显示名。
- `Expression.column` 表示列下标；候选收集要求它已存在，替换会写入它。
- `Expression.virtual_column` 决定表达式能否作为生成列候选。
- `PlanNode` 的五组表达式向量是替换面，`children` 形成递归计划树。

映射键只做 ASCII 小写归一化，未处理空白、括号、限定名、函数参数结构或非 ASCII 大小写。两个不同数据源若产生相同小写名称，后遍历并插入者覆盖先前值；代码没有记录来源 schema，也没有歧义检测。

## 依赖与调用关系

直接依赖很小：标准库 `std::collections::HashMap`，以及同 crate 的 `crate::task::{Expression, PlanNode}` 和 `PlanKind`。`pkg/planner/core/Cargo.toml` 表明文件归属 `astersql-planner-core`，但本文件本身不直接使用该清单列出的外部 crate，也没有受 `nextgen` feature 控制。

RustCodeGraph 将目标文件识别为 12 个符号，并显示源文件直接被 `pkg/planner/core/rule_generate_column_substitute_test.rs` 使用；原始仓库搜索确认 `lib.rs` 负责模块公开，测试通过 `GcSubstituter::Optimize`、`collectGenerateColumn` 和 `SubstituteExpression` 调用本模块。没有检索到 Rust 生产规则列表调用 `GcSubstituter`，所以应用主链位置目前只能表述为“意图对应逻辑优化的生成列替换阶段，尚未验证生产接线”。

下游调用链为 `Optimize → collectGenerateColumn` 和（映射非空时）`Optimize → substitute → substituteExpression → tryToSubstituteExpr`；`substitute` 还递归调用自身，`collectGenerateColumn` 也递归调用自身。

## 错误处理与边界

所有函数都不返回 `Result`，不存在显式错误传播。无法收集或无法匹配均以“不修改”表示：空映射跳过替换，非 DataSource 节点不收集，非虚拟列或缺失列下标被忽略，名称不匹配返回 `false`。

关键边界包括：

- `PlanKind::Cte` 在收集阶段立即返回，CTE 内的数据源不会污染外层映射；替换阶段本身并不跳过 CTE，因此外层已收集的名称仍会遍历到 CTE 节点及子树。这是当前源码的精确行为。
- DataSource 由 `PlanKind::Other("DataSource".to_owned())` 这一字符串约定识别；拼写或大小写不同不会命中。
- 候选匹配只比较名称，未验证返回类型、列是否在当前 schema、索引是否存在、表达式是否含输入列、TiFlash 偏好或可变常量/计划缓存安全性。
- `substitute` 的内部 `bool` 会反映实际替换，但公开 `Optimize` 固定返回 `false`。调用者不能用第二返回值判断计划值是否已经改变。

这些差异意味着当前 Rust 文件适合其轻量 `task` 模型，不能宣称与 Go 生产实现的所有安全条件等价。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。`GcSubstituter` 无共享可变状态；每次调用的映射和计划均由当前栈帧独占，因此不同规则实例或对不同计划的调用可由外部并行调度而不在本模块内共享状态。

资源生命周期由 Rust 所有权自然约束：`Optimize` 消费输入 `PlanNode`，在局部可变变量中就地改写，再把所有权交还调用者；递归函数只借用计划和映射。风险主要是递归深度与工作量，而非同步：收集和替换各遍历一次计划树；每个待替换表达式通常进行一次字符串小写分配和一次哈希查找。极深计划树可能受到调用栈限制，代码没有显式深度保护。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/rule_generate_column_substitute.go`，规则名和“两阶段收集后替换”、CTE 收集边界、`planChanged == false`、Selection/Projection/Sort/Aggregation 的替换意图与 Go 一致；Go 规则在 `pkg/planner/core/optimizer.go` 注册，Go 独立测试 `pkg/planner/core/rule_generate_column_substitute_test.go` 主要提供 `SubstituteExpression` 的性能基准。

Rust 是明显的轻量移植，以下 Go 行为尚未在本文件中实现：

- Go 只从非表路径索引的虚拟、非 stored 生成列收集候选，并跳过偏好 TiFlash 的数据源；Rust 只检查 `virtual_column` 和列下标。
- Go 使用真实表达式对象、类型与 schema，要求结构等价、求值类型一致、目标列在当前 schema；Rust 只按 ASCII 不区分大小写的名称匹配。
- Go 跳过不引用任何列的纯常量生成表达式，并按会话变量检查类型兼容；Rust 没有这些信息与门槛。
- Go 对比较、`IN`、`LIKE`、逻辑与/或、`NOT` 有定向递归规则，替换后重算表达式哈希，并在可变常量影响索引选择时标记跳过计划缓存；Rust 不建模表达式树、哈希或计划缓存。
- Go 针对不同逻辑算子使用相应 schema；Rust 无条件扫描每个 `PlanNode` 的五组通用表达式容器。

因此，后续若要接入生产优化主链，不能仅增加注册；还需先确认轻量 `PlanNode` 的表达式等价、schema、索引和缓存安全能力是否足够。

## 扩展指南

安全扩展应从真实差异出发：

1. 若增加候选安全门槛，优先修改 `collectGenerateColumn`，并先为索引存在性、stored/virtual 区分、CTE、TiFlash 和常量表达式分别补充 `pkg/planner/core/rule_generate_column_substitute_test.rs` 中的独立回归用例。
2. 若增强表达式匹配，修改 `substituteExpression` / `tryToSubstituteExpr`，但应避免继续把复杂语义编码进 `name` 字符串；需要同步扩充 `task::Expression` 或复用真实表达式模型，并覆盖类型不一致、schema 不含列、嵌套布尔表达式和计划缓存安全场景。
3. 若新增可替换的计划字段，在 `GcSubstituter::substitute` 的迭代链中接入，并用对应算子测试证明既改写目标字段又不误改无关字段。
4. 若接入 Rust 生产优化器，应在真实规则注册点增加最小接线，并验证规则顺序；当前 Go 对照把它放在 `optRuleList` 首位。接线任务超出本文档分析范围。
5. 保持测试逻辑在独立的 `rule_generate_column_substitute_test.rs`，不要把单元测试内嵌到生产文件；新增 Rust 行为也应尽量保持 Go 安全条件与测试意图，而不是为了通过测试删减门槛。

兼容风险集中在同名表达式歧义、类型/作用域错误替换和变化标志契约；性能风险集中在每个表达式的小写字符串分配、两遍计划遍历以及深树递归。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/planner/core/rule_generate_column_substitute.rs` 确认目标被索引；`node --file ... --offset 1 --limit 260` 读取并核对目标文件全部 89 行；`query GcSubstituter`、`query collectGenerateColumn`、`query substituteExpression`、`query tryToSubstituteExpr` 核对主要符号；同名 `Optimize` 的全局图查询存在歧义，因此调用边又以目标文件源码和仓库精确搜索交叉验证。
- Rust 源与模块边界：`pkg/planner/core/rule_generate_column_substitute.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`。
- Rust 独立测试：`pkg/planner/core/rule_generate_column_substitute_test.rs`，覆盖生产树遍历、大小写、规则元数据、CTE/DataSource 边界和聚合表达式。
- Go 对照与接线：`pkg/planner/core/rule_generate_column_substitute.go`、`pkg/planner/core/optimizer.go`、`pkg/planner/core/rule_generate_column_substitute_test.go`。
- 人工复核结论：该文件存在是为了把重复的生成列表达式改写成列引用，以便后续规划阶段利用对应列/索引；当前 Rust 代码如何运行、哪些边界未建模、最安全的扩展入口及需同步的独立测试均已在以上章节明确说明。
