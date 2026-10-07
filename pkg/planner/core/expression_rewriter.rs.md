# `pkg/planner/core/expression_rewriter.rs`

## 文件定位

本文件是 `astersql-planner-core` crate 中的 AST 表达式重写实现。`pkg/planner/core/lib.rs` 以私有模块 `mod expression_rewriter;` 装配它，仅把 `subQueryCtx` 与 `unknowClause` 以 `pub(crate)` 重导出；因此大量 `pub fn` 主要是 crate 内部可见的移植接口，而不是跨 crate 的稳定公共 API。它位于 SQL 规划链的“解析器 AST → 可求值 `expression::ExprBox`，必要时同时改写逻辑计划”边界。

`pkg/planner/core/Cargo.toml` 将该目录定义为 `astersql-planner-core`，默认无 feature，`nextgen` 只向配置依赖传递。本文涉及的直接依赖包括 parser AST、expression/aggregation/expropt、logicalop/physicalop、planner base/rule/coreusage、infoschema/model/table、session variable/vardef、hint、plannererrors 与 stringutil。包内没有 `doc.go`；最近的契约文档 `pkg/planner/core/base/doc.go` 只约束 base 接口抽象，不应被误读为本文件的行为定义。

源文件带有 `// Copyright 2026 AsterSQL.`，不是门面、生成代码或桩：其 5,585 行包含完整访问器、子查询计划改写、表达式构造及错误处理逻辑。文件内没有条件编译项。

## 核心职责

1. 通过 `expressionRewriter` 实现 `ast::ExprNodeVisitor`，以 `Enter`/`Leave` 深度优先遍历 AST，并维护表达式栈与名称栈。
2. 为纯表达式场景提供 `evalAstExpr`、`buildSimpleExpr`；为需要规划器能力的场景提供 `evalAstExprWithPlanCtx`、`rewriteAstExprWithPlanCtx`、`rewrite` 与 `rewriteWithPreprocess`。
3. 将列、常量、参数、变量、函数、算术/布尔/行比较、`IN`、`BETWEEN`、`LIKE`、`REGEXP`、`CASE`、CAST、DEFAULT、聚合及窗口引用转换为 expression 树。
4. 对 EXISTS、IN、比较量词和标量子查询构建子计划，并按相关性、SQL 语境、Hint 与会话变量选择常量求值、Apply、SemiApply 或 InnerJoin+Distinct/Aggregation 改写。
5. 为 `MATCH ... AGAINST` 在原生 TiFlash FTS 与严格受限的 ILIKE 回退之间做可行性判定，并维护计划缓存安全性。
6. 保持 Go `pkg/planner/core/expression_rewriter.go` 的 SQL 三值逻辑、列名解析、collation、Hint 警告和错误语义，同时解决 Rust 所有权、AST clone 与可变计划替换问题。

## 主要符号

- `clauseCode`、`fieldList` 至 `partitionByClause`、`clauseMsg`：记录当前 SQL 子句，用于未知列等诊断；值与消息数组下标必须同步。
- `subQueryCtx` 及 `handlingExistsSubquery`、`handlingCompareSubquery`、`handlingInSubquery`、`handlingScalarSubquery`：向 `PlanBuilder` 标识正在构建的子查询种类。
- `AggregateMapper` / `WindowMapper`：AST 到输出列偏移的映射。`AggregateMapperKey` 先清除位置/标志、规范标识符大小写，再用完整结构调试表示作为 key，补偿 Rust visitor 克隆子节点后无法沿用 Go 指针键的问题。
- `expressionRewriter<'a>`：核心访问器。`ctxStack` 与 `ctxNameStk` 平行保存 expression 和 `FieldName`；另持有 schema/names、首个错误、构建/请求上下文、标量模式、CAST ARRAY 开关、常量折叠计数器、参数顺序、AST 祖先栈及可选 `planCtx`。
- `exprRewriterPlanCtx<'a>`：规划侧状态，包含当前逻辑计划、`PlanBuilder`、子句、聚合/窗口映射、INSERT 计划及 rollup expand。
- `PlanBuilderPtr`：由调用者活跃的 `&mut PlanBuilder` 构造的 `NonNull` 包装；同步重写期间通过 `Deref`/`DerefMut` 提供 Go 指针式访问。
- `MovableLogicalPlan`：`Option<LogicalPlanRef>` 包装，通过 `take` 后立即 `replace` 模拟 Go 接口值替换；缺失状态会 panic，表示内部不变量破坏。
- `rewriteExprNode` / `rewriteSimpleExprNode`：访问器驱动与收尾。前者允许计划变化并隐藏子查询临时输出列名；后者明确要求无 `planCtx`。
- `Enter` / `Leave`：访问器分派中心。`Enter` 提前处理映射后的聚合/窗口与各类子查询、维护折叠/祖先状态；`Leave` 根据节点种类消费子表达式并压入结果。
- `constructBinaryOpFunction`：处理标量和行值比较；行的 `=`/`<=>` 合成 CNF，`!=` 合成 DNF，大小比较展开为字典序 DNF。
- `buildSubquery`、`handleExistSubquery`、`handleInSubquery`、`handleCompareSubquery`、`handleScalarSubquery`：子查询主干。
- `matchAgainstToExpression`、`ftsNativeViable`、`matchAgainstToBuiltin`、`matchAgainstToLike`：全文检索双路径。
- `toColumn`、`findFieldNameFromNaturalUsingJoin`、`resolveRedundantColumnFromNaturalUsingJoinPlan`：普通列、相关列以及 NATURAL/USING 冗余列解析。
- `evalDefaultExprForTable`、`evalDefaultExprWithPlanCtx`、`evalFieldDefaultValue`：DEFAULT 名字空间与元数据求值。

## 执行流程

入口分为两条。简单表达式由 `buildSimpleExpr` 解析 `BuildOption`，校验 `InputSchema` 与 `InputNames` 同时存在且等长；必要时从 `SourceTable` 生成公开列 schema，随后用无 `planCtx` 的 rewriter 调用 `rewriteSimpleExprNode`。规划器表达式由 `rewrite` 转交 `rewriteWithPreprocess`，后者经 `getExpressionRewriter` 绑定当前计划、schema/names、builder、聚合/窗口映射和预处理回调，再进入 `rewriteExprNode`。

遍历时，AST 的 `Accept` 调用 `Enter` 后递归子节点，再调用 `Leave`。每个普通叶子或复合节点最终向 `ctxStack`/`ctxNameStk` 同步压入一个结果；复合节点按子节点数弹栈。`rewriteExprNode` 检查错误与最终栈深度，拒绝多列 ROW 作为标量，并返回“可选表达式 + 可能已插入 Apply/Join 的计划”。非标量过滤语境允许子查询只改变计划而不留下表达式。

子查询流程是：`buildSubquery` 暂存并恢复 outer schema/names、window specs、subquery context 与 hint flags；具体 handler 检查左右列数和 collation，提取相关列，解释 `NO_DECORRELATE`/`SEMI_JOIN_REWRITE`。相关子查询或含 CTE consumer 的子计划通常进入 Apply/SemiApply；无相关标量子查询可调用 `DoOptimize` 后通过 `eval_subquery_first_row` 规划期求值，EXPLAIN 非 ANALYZE 且启用不求值选项时则注册 `ScalarSubQueryExpr`。`IN` 在满足会话开关、非 NOT、非标量、无相关列且 collation 兼容时可改写为 InnerJoin + 比较域上的 Distinct，否则构造 SemiApply。

普通表达式在 `Leave` 中分派。常量会深拷贝类型并修正 NULL/NotNull、字符 repertoire 和 utf8mb4 collation；参数用遍历顺序 `nextParamOrder`；函数经 `newFunctionWithInit` 依据折叠计数器选择初始化/折叠策略；`IN`、行比较、BETWEEN 会先做列数、比较类型与 collation 处理。列解析依次检查输入 schema、单表 source、NATURAL/USING 完整 schema、外层 schema，失败时按当前 clause 产生兼容诊断。

FTS 流程先用 `inDirectMatchBooleanContext` 判定是否处在 WHERE/HAVING/ON 下仅由括号、AND/OR/NOT 包围的直接布尔位置。原生路径要求默认 modifier、TiFlash replica 可用且每列存在 public FULLTEXT index；替代计划回合可在直接布尔位置改走 ILIKE。回退只接受常量字符串及受支持的搜索子集，遇到可变常量会 `SetSkipPlanCache`，NULL 保持三值逻辑，标量评分位置仍保留原生浮点评分语义。

## 数据与状态

最重要的不变量是 `ctxStack.len() == ctxNameStk.len()`；`ctxStackAppend` 同时压栈，`ctxStackPop` 同时截断。成功结束时简单表达式必须恰有一个元素；计划重写若 `as_scalar == false` 可返回空表达式，否则也必须恰有一个。

`err: Option<errors::Error>` 是 visitor 无法直接返回 `Result` 时的短路通道，首个错误在遍历边界由 `rewriteExprNode` 或 `rewriteSimpleExprNode` 取出并 `Trace`。`disableFoldCounter` 与 `tryFoldCounter` 是嵌套作用域计数，而不是布尔值；`Enter` 增、`Leave` 减，防止嵌套函数提前恢复折叠。

计划状态通过 `MovableLogicalPlan` 转移所有权；每次 `take` 后必须在所有正常路径上立即 `replace`。`buildSubquery` 对 builder 的 outer scope、window specs、subquery flags 做成对保存/恢复；各 handler 还用 `prepareCTECheckForSubQuery` / `resetCTECheckForSubQuery` 包围工作。标量子查询优化会临时禁用 force-nth-plan 并在之后恢复。

`AggregateMapperKey` 使用完整规范化 AST key 避免哈希碰撞和标识符中分隔符造成的别名。参数 marker 不沿用 SQL token offset，而按从左到右访问顺序编号。相关列的数据槽使用 `Arc<RwLock<Datum>>`，供后续 Apply 执行阶段填值。

## 依赖与调用关系

RustCodeGraph 的直接调用证据显示：`rewriteInsertOnDuplicateUpdate` 由 `pkg/planner/core/planbuilder_runtime.rs::buildInsert` 调用；`rewrite` 由同文件的 `buildInsert`、`buildUpdate` 调用；`rewriteWithPreprocess` 由 `pkg/planner/core/logical_plan_builder_runtime.rs` 调用。文件内部 `rewrite → rewriteWithPreprocess → getExpressionRewriter → rewriteExprNode → ast::ExprNode::Accept → Enter/Leave` 形成主链。

下游方面，expression crate 提供表达式类型、函数工厂、类型/行长度/collation/计划缓存判定；logicalop 提供 Dual、Projection、Aggregation、Join、Apply、Limit 等节点；physicalop 与 `DoOptimize` 支持独立子查询规划；coreusage 提取相关列；infoschema/model/table 用于 FTS 和 DEFAULT 元数据；variable/vardef、hint 与 rule 决定改写开关和优化 flag。

`pkg/planner/core/lib.rs` 把 `expression_rewriter` 保持为私有模块，但规划器运行时模块处于同一 crate，可以直接调用其符号。`scalar_subq_expression.rs` 提供 `ScalarSubQueryExpr`、`ScalarSubqueryEvalCtx` 和首行求值桥接。本文件也通过 `PlanBuilder` 的 `buildSemiApply`、`buildApplyWithJoinType`、`buildDistinct`、`buildLimit`、`buildResultSetNode` 等方法把表达式重写接回逻辑计划构建链。

## 错误处理与边界

对外入口返回 `Result<_, errors::Error>`；visitor 内函数多把错误写入 `rewriter.err`，由上层统一传播。明确的用户边界包括：schema/name 缺一或长度不符、最终栈深度错误、行比较列数不符、多列 ROW 用作标量、非法窗口函数位置、缺失 plan context、未知/歧义列、禁止的 CAST ARRAY、超范围时间精度、DEFAULT 无原始表列，以及尚不支持的 `<=> ANY/ALL`。

子查询必须保持 SQL NULL/空集语义：量词计划额外构建 `SUM(IS NULL)` 与 `COUNT(1)`；IN/SemiApply 对可能为 NULL 的操作数设置 `InOperand`；标量子查询无行时生成与输出列数相同的 NULL；多行约束交给 `LogicalMaxOneRow`。EXISTS 会剥离不影响“是否有行”的 Projection/Sort，并把无 GROUP BY 的标量聚合归约为单行 Dual。

内部 `expect`/`assert!` 表示不可恢复的不变量，例如构造好的聚合必须有返回类型、子查询输出应是 Column/ROW、计划不能在 `take` 后悬空、简单表达式不能同时持有 `planCtx`。这些不是面向用户的校验，扩展时不得让外部输入触发它们。

FTS 明确拒绝非字符串列、非常量/非字符串搜索值、LIKE 回退不支持的 tokenizer 语法和无法安全下推的 modifier；只有替代计划确定会丢弃第一回合时，才暂时容许不可下推 modifier 生成原生节点。列解析与 Hint 冲突会生成 TiDB 兼容错误或 warning，而不是静默降级。

## 并发与资源生命周期

整个 rewriter 是单次、同步、栈上使用的可变访问器，没有线程、异步任务或通道。`PlanBuilderPtr` 是本文件最敏感的生命周期点：它用 `unsafe` 解引用由调用者 `&mut PlanBuilder` 创建的裸指针；安全性依赖“rewriter 不逃逸本次同步调用且同一时刻只有一个可变访问路径”。任何把 rewriter 保存、跨线程发送或在回调中重入 builder 的修改都会破坏该前提。

共享对象主要使用 `Arc`：请求上下文、InfoSchema、物理计划、FieldName 和标量子查询上下文可被后续阶段持有。相关列值用 `Arc<RwLock<Datum>>` 表达执行时共享可变槽；本文件只创建，不负责长期调度。`PlanColumnID` 用 `SeqCst` 原子读写；MPP 标量子查询对齐计划 ID 后会恢复 checkpoint 并调整列 ID frontier，避免后续分配冲突。

资源恢复必须覆盖错误路径：`buildSubquery` 在闭包结果返回后恢复 builder 状态；子查询 handler 在闭包外重置 CTE 检查；独立子查询优化后恢复 nth-plan hint。新增提前返回应沿用这种“闭包收集 Result，外层统一恢复”的结构。

## 与 Go 版本的对应关系

权威对照是同目录 `pkg/planner/core/expression_rewriter.go`。Go 文件的 `evalAstExprWithPlanCtx`、`buildSimpleExpr`、`rewriteWithPreprocess`、`expressionRewriter.Enter/Leave`、四类子查询 handler、FTS 双路径、表达式转换、列解析和 DEFAULT 求值，在 Rust 中均有同名或显式 receiver 参数版本；Rust 源码注释也逐段注明“对应 Go 的同名函数”。

Rust 的结构性差异有四类：Go receiver 被改为自由函数加 `&mut expressionRewriter`；Go 的 `map[*ast.AggregateFuncExpr]int` 改为规范化结构字符串键；Go 可直接替换接口计划值，Rust 用 `MovableLogicalPlan`；Go 指针共享 builder，Rust 用受严格同步生命周期约束的 `PlanBuilderPtr`。这些是语言适配，不应改变 SQL 行为。

测试证据需区分覆盖层级。`pkg/planner/core/expression_rewriter_test.rs` 目前只有 `planner_factory_installation_is_idempotent`，验证 `InstallPlannerExpressionFactory` 重复安装；它不覆盖本文件大部分重写分支。Go 的 `pkg/planner/core/expression_test.go` 覆盖 BETWEEN、CASE、CAST、CAST 类型不共享、IN、IS NULL、行比较、真值和一般表达式构建；`pkg/planner/core/tests/subquery/subquery_test.go`、`pkg/planner/core/casetest/scalarsubquery/cases_test.go`、`pkg/planner/core/casetest/windows/window_with_exist_subquery_test.go` 进一步覆盖 collation、EXPLAIN 标量子查询及窗口内子查询。维护 Rust 行为时应保持这些 Go 意图，并在独立 Rust `*_test.rs` 文件中补对应回归，不能把测试嵌入本源文件。

## 扩展指南

新增 AST 节点时，先决定它是否需要 `planCtx`、是否在 `Enter` 跳过默认子遍历、以及 `Leave` 应消费/产生多少栈元素；同时维护 `ctxStack`/`ctxNameStk`、`astNodeStack` 与折叠计数。若新增节点可能出现在无计划的 `buildSimpleExpr`，不得无条件调用 `rewriteExprNode` 或 `requirePlanCtx`。

新增子查询改写应接入相应 handler，并同步处理：outer scope 保存恢复、CTE 检查、相关列提取、列数/collation、NULL 与空集、`asScalar`、Hint 冲突、输出 names 截断及 optimizer flags。计划 `take` 后每条成功路径都要 `replace`；不要用编译通过或空桩替代 Go 行为。

新增函数或运算符时优先复用 `newFunction`/`newFunctionWithInit`、`constructBinaryOpFunction`、`notToExpression` 与现有类型/collation 检查。修改 FTS 时必须同时审视直接布尔语境、评分语境、modifier 下推协议、TiFlash/fulltext index 可行性、NULL 三值逻辑和 plan-cache 可变常量。

测试应放在同目录独立 Rust 文件，例如扩展 `pkg/planner/core/expression_rewriter_test.rs`，并与 `lib.rs` 的 `#[cfg(test)] mod ...` 装配保持一致；复杂 SQL 行为可在既有 `tests/subquery`、`casetest/scalarsubquery` 或窗口测试面增加用例。任何语义修改都应逐项对照 `expression_rewriter.go` 和相关 Go 测试，除非任务明确要求有意分叉。

性能风险集中在 AST key 字符串化、行比较组合爆炸、IN 列表表达式构造、子查询规划期优化/执行和 FTS 元数据查询；兼容风险集中在 NULL、collation、Hint、列名空间与计划缓存。修改前后应针对这些分支做聚焦回归。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件已索引为 5,585 行。
- RustCodeGraph `node --file pkg/planner/core/expression_rewriter.rs`：分段核对了入口、状态结构、visitor、子查询、普通表达式、FTS、列解析和 DEFAULT 的实现；`files --filter pkg/planner/core` 确认模块、Go 对照与测试位置。
- RustCodeGraph 精确调用探索：确认 `planbuilder_runtime.rs::buildInsert/buildUpdate`、`logical_plan_builder_runtime.rs` 到 `rewriteInsertOnDuplicateUpdate`、`rewrite`、`rewriteWithPreprocess` 的上游边，并确认 `Enter` 到子查询 handler 的内部关系。泛化查询存在同名噪声，本文没有把噪声结果当作事实。
- 读取的直接文件：`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`、`pkg/planner/core/base/doc.go`、`pkg/planner/core/expression_rewriter.go`、`pkg/planner/core/expression_rewriter_test.rs`。
- 查阅的相关测试入口：`pkg/planner/core/expression_test.go`、`pkg/planner/core/tests/subquery/subquery_test.go`、`pkg/planner/core/casetest/scalarsubquery/cases_test.go`、`pkg/planner/core/casetest/windows/window_with_exist_subquery_test.go`。
- 本任务是纯文档分析，按任务要求未运行 Cargo。交付前运行固定 11 章节结构检查，并人工复核本文能够回答文件为何存在、主流程如何运行、状态/错误/生命周期边界以及如何安全扩展。
