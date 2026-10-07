# `pkg/parser/parser_actions/query.rs`

## 文件定位

`query.rs` 是 `astersql-parser` crate 的查询语法语义动作模块。crate 入口由 `pkg/parser/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定；本文件经 `pkg/parser/parser_actions/mod.rs` 中的私有 `mod query` 接入总语义动作分派器，不是独立解析入口。生成的主解析器完成一次归约时，`parser_actions::apply` 按模块顺序询问 `query::owns(rule_id)`；若该稳定规则 ID 属于查询语法，再调用 `query::apply` 把归约右部 `Rhs` 转换为 `yySymType` 中的 AST、表达式或中间语义值。

文件覆盖 `Field`、`SelectStmt`、`TableRefs`、窗口函数、CTE、集合运算、排序/分组/限制/锁、表采样及优化器 Hint 等查询非终结符。它不是 SQL 执行器，也不负责词法扫描或语法表生成；输入来自解析栈，输出是 `parser_ast` 节点，后续才会进入 planner/executor。直接证据是 `pkg/parser/parser_actions/mod.rs::{apply,has_semantic_action}`、本文件的 `owns`/`apply`，以及 `pkg/parser/grammar/main.astergram`（Rust 稳定 RuleId 的来源）；Go 对照语义位于 `pkg/parser/parser.y`。

## 核心职责

1. **稳定规则识别。** `identify(RuleId)` 将形如 `selectstmt_...--<hash>` 的生成期稳定 ID 映射为私有 `QueryRule` 枚举，避免依赖会随语法表重排而变化的数字归约号。`query_expression_modules_have_no_numeric_fallback` 明确禁止 `legacy_rule_number`、`apply_numeric` 和 `rhs_index` 回退。
2. **构造查询 AST。** `apply_rule` 根据规则从 `Rhs` 的 `ident`、`expr`、`statement`、`item`、`offset` 槽取值，构造或补全 `SelectStmt`、`SetOprStmt`、`TableSource`、`Join`、`WindowSpec`、`Limit`、`SelectField` 等 `parser_ast` 类型。
3. **维护组合语义。** 列表规则采用“首项建向量、后续项追加”；可选规则输出 `None`、空向量或显式默认枚举；复合规则先取出已有节点再附加 `WHERE`、`GROUP BY`、`ORDER BY`、`LIMIT`、锁、`INTO`、CTE 等字段。
4. **保留源文本与语法语义。** `FieldListAlt01/02` 根据 `yySymType.offset`、`Parser.yylval.offset` 和 `Parser.src` 保存表达式字段的 `OriginalText`；集合运算分支保留括号内外 `ORDER BY`/`LIMIT` 的归属；`ByItem` 对整数位置引用设置 `FLAG_HAS_REFERENCE`。
5. **报告查询专属诊断。** 合并 `SelectStmtOpts` 时拒绝同时出现 `ALL` 与 `DISTINCT`；限定表名时拒绝非法数据库名；优化器 Hint 通过 `Parser::parseHint` 解析，并将非致命问题转为 lexer warning。

## 主要符号

- `enum QueryRule`（第 21 行）：私有、可排序的 235 个查询动作标签。变体按非终结符和候选式命名，例如 `SelectStmtAlt01`、`WindowFuncCallAlt07`、`JoinTableAlt05`；派生的顺序只在窗口边界分组逻辑中用于判断 preceding/following 范围。
- `fn identify(rule_id: RuleId) -> Option<QueryRule>`（第 259 行）：稳定 ID 到枚举的穷举表。未知 ID 返回 `None`，不消费语义栈。
- `pub(super) fn owns(rule_id: RuleId) -> bool`（第 728 行）：供同级总分派器判断所有权；本模块之外不可见。
- `pub(super) fn apply(...) -> Option<Result<bool, isize>>`（第 732 行）：再次识别 ID；非本模块规则返回 `None`，已识别规则把 `apply_rule` 的结果包在 `Some` 中。
- `fn apply_rule(...) -> Result<bool, isize>`（第 741 行）：核心大分派。成功写入输出后返回 `Ok(true)`；所需动态类型或节点缺失时返回 `Ok(false)`；明确语法错误返回 `Err(1)`。
- `Context { output, parser_state, lexer }` 与 `Rhs` 定义/实现位于 `pkg/parser/parser_actions/mod.rs`：`output` 是本次归约的 `yySymType`，`parser_state` 提供源 SQL、当前 token 偏移和 Hint 解析器，`lexer` 提供 SQL mode、Hint 位置及错误/警告通道。
- `SubquerySemantic`、`GroupBySemantic`、`SetOprSemantic` 位于 `pkg/parser/parser_semantic_support.rs`：分别跨规则携带共享查询节点、`GROUP BY` 项及 rollup 标志、集合运算节点与对应操作符。

## 执行流程

1. 主解析器用稳定 `RuleId` 调用 `parser_actions::apply`；总分派器先用 `query::owns` 确认所有权，再进入 `query::apply`。`pkg/parser/parser_actions/query_expression_aster_unit_test.rs::query_expression_rule_coverage` 从 `grammar/main.astergram` 重新计算需动作的查询规则，并断言数量为 235、与 expression 规则不重叠。
2. `identify` 选择 `QueryRule`；`apply_rule` 解构 `Context`，并以 `rhs_len - back` 方式读取当前归约右部。这里的 `Rhs` 下标由 `pkg/parser/parser_actions/mod.rs` 的 `Index/IndexMut` 转给语义栈借用器，`back = 0` 表示最右侧符号。
3. 叶子规则把 token 转成布尔值、枚举、`CIStr`、参数标记或简单 AST；列表规则创建/追加 `Vec`；可选规则写 `None` 或语义规定的默认值。动态载荷统一放在 `yySymType.item: Box<dyn Any>`，表达式和语句则优先走专用 `expr`/`statement` 槽。
4. 中层规则形成查询片段：字段与别名形成 `SelectField`，排序项形成 `ByItem`，窗口边界形成 `FrameBound/FrameExtent`，表名/别名/索引 Hint/采样形成 `TableSource`，左右输入及 `ON`/`USING` 形成 `Join`。
5. `SelectStmtBasicAlt01` 先把 options、字段和 Hint 组装成基础 `SelectStmt`；`SelectStmtFromDualTableAlt01`、`SelectStmtFromTableAlt01` 补充来源、过滤、分组、HAVING 和窗口定义；`SelectStmtAlt01..05` 再附加排序、限制、锁和 `INTO`，或构造 `TABLE`/`VALUES` 形式，最终写入 `out.statement`。
6. `SubSelectAlt01..04` 用 `NodeRef` 同时生成 `ExprKind::Subquery` 和 `SubquerySemantic`。CTE 分支构造 `WithClause/CommonTableExpression` 并标记递归；集合运算分支用 `SetOprSemantic` 累积节点/操作符，并把括号子查询自己的 `WITH`、`ORDER BY`、`LIMIT` 封装在 `SetOprSelectList`，避免错误提升到外层。
7. 窗口函数分支把函数名、实参、`IGNORE NULLS`、`FROM LAST` 与 `WindowSpec` 写入 `ExprKind::WindowFunction`；表连接分支统一计算左右输入、连接类型、`ON`/`USING`、natural/straight 标志。

## 数据与状态

- **输入状态：** `Rhs<'_>` 是当前归约右部的借用视图。每个 `yySymType` 可含 `ident`、`expr`、`statement`、动态 `item` 和源码 `offset`。大量 `.take()` 表明语义值按所有权向父归约移动；只需复用时用 `clone()`。
- **输出状态：** `Context.output` 指向父符号的 `yySymType`。动作只填充与该非终结符匹配的槽；例如窗口函数写 `out.expr`，顶层 SELECT/集合查询写 `out.statement`，中间对象写 `out.item`。
- **动态类型不变量：** `item` 的具体类型由语法产生式决定，如 `Vec<ByItem>`、`SelectStmtOpts`、`TableName`。`downcast`/`downcast_ref` 是这项约定的运行时检查；新增或调整规则时，生产者与消费者必须同步类型和右部位置。
- **查询中间态：** `GroupBySemantic` 同时保存 items 与 `WITH ROLLUP`，因为当前 Rust AST 将它们分别写入 `SelectStmt.GroupBy` 和 `GroupByRollup`；`SetOprSemantic` 用平行的 `nodes`/`operators` 向量保留每个集合项之前的操作符；`SubquerySemantic` 持有 `NodeRef`，使表达式节点和 FROM/CTE 路径共享查询节点。
- **解析器状态：** 只有字段原文截取和 Hint 解析读取/使用 `parser_state`；错误及 warning 写入 `yylex`。文件没有全局可变状态。

## 依赖与调用关系

上游调用链是“生成解析器归约 → `pkg/parser/parser_actions/mod.rs::apply` → `query::owns` → `query::apply` → `apply_rule`”。RustCodeGraph 索引确认本文件包含 242 个符号，并定位 `QueryRule`、`identify`、`apply_rule`；私有 `apply_rule` 的 caller/callee 图查询未返回可用边，模块入口的直接源码则明确给出调用关系。`remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 进一步断言所有需要动作的生成规则恰有一个模块所有者。

下游主要依赖如下：

- `parser_ast`：绝大多数输出类型，包括 SELECT、集合运算、表引用/连接、窗口、字段、排序、限制、锁、Hint 和表达式节点；由 `pkg/parser/Cargo.toml` 的本地依赖 `parser-ast = { package = "astersql-parser-ast", path = "ast" }` 提供。
- `pkg/parser/parser_semantic_support.rs`：提供 `RuleId` 相关公共导入、`SubquerySemantic`、`GroupBySemantic`、`SetOprSemantic`、`semantic_value_text`、`result_set_from_item`、`getUint64FromNUM`、`isInCorrectIdentifierName` 等跨动作辅助。
- `Parser` 与 `yyLexer`：前者提供 SQL 源串、当前 token 和 `parseHint`，后者提供 SQL mode、Hint 偏移及诊断通道。
- `mysql` 与 parser terror 错误：构造 `ErrWrongUsage`、`ErrWrongDBName`，并以 lexer 错误列表作为解析失败/警告的对外载体。

文件本身不调用 planner、executor、存储或网络模块；它的产物通过 `parser_ast::Node`/`ExprNode` 被解析 API 返回，随后才由上层消费。

## 错误处理与边界

- `Ok(true)` 表示规则被成功执行；`Ok(false)` 表示规则虽被识别，但所需语义槽为空或动态类型不符合预期，例如缺失 `SelectStmt`、`WindowSpec`、`FrameBound`、`SubquerySemantic` 或左右 join 输入。多数可选值则有意使用 `unwrap_or_default`，所以“缺省语义”与“必需类型错误”必须按现有分支区分，不能机械统一。
- `Err(1)` 只用于已经写入 lexer 诊断的硬错误：`SelectStmtOptsListAlt01` 检出 `ALL` 与 `DISTINCT` 同用时追加 `ErrWrongUsage`；`TableNameAlt02/TableNameOptWildAlt02` 检出错误 schema 标识符时追加 `ErrWrongDBName`。
- `TableOptimizerHintsAlt01` 的 Hint 解析错误按 Go 行为降级为 warning：逐个 `AppendError` 后立即 `LastErrorAsWarn`，仍返回 AST。`pkg/parser/parser_test.rs::test_hint_error` 验证未知 Hint 产生 warning 而合法 Hint 保留。
- 源文本切片通过 `.get(range).unwrap_or_default()`，越界不会 panic，而会得到空文本；RHS 下标本身依赖生成语法与动作表一致，`Rhs` 的 `Index` 实现对不合法位置使用 `expect("semantic RHS position")`，因此规则 ID、产生式和 `back` 偏移必须原子同步。
- 集合运算、CTE 和 subquery 只接受 `SelectStmt`/`SetOprStmt` 等预期节点；未知节点类型返回 `Ok(false)`，不臆造替代 AST。

## 并发与资源生命周期

该模块没有线程、异步任务、通道、锁、文件句柄或事务。一次动作通过带生命周期的 `Rhs<'_>` 和 `Context<'_>` 独占借用当前解析栈输出、`Parser` 与 lexer，因此变更仅作用于当前解析过程；并发安全取决于调用方不要跨线程共享同一个可变 parser/lexer，而不是本文件内部同步。

资源主要是堆上的 AST 与向量。`.take()` 将右部所有权迁移到父节点，减少重复持有；`.clone()` 用于共享语义或需要保留右部时。子查询通过 `NodeRef` 管理共享节点；CTE 的 `WithClause` 用 `into_shared()` 接入 AST。动作完成后，未迁移的临时向量和中间语义按 Rust 所有权自动释放，不存在显式清理流程。

## 与 Go 版本的对应关系

Rust 文件不是 Go 同路径文件的逐行翻译；其直接语义来源是 `pkg/parser/parser.y` 的同名非终结符动作，生成后的 Go 展开代码位于 `pkg/parser/parser.go`。对应关系包括：

- Go `Field`/`FieldList` 构造通配符或表达式字段并保存原始文本；Rust 对应 `FieldAlt01..04`、`FieldListAlt01/02`。
- Go `SelectStmtBasic`、`SelectStmtFromDualTable`、`SelectStmtFromTable` 和 `SelectStmt` 分阶段组装 SELECT；Rust 对应同名前缀规则，并额外通过 `GroupBySemantic`、动态 `item` 与专用 `statement` 槽适配 Rust 类型系统。
- Go `TableSampleOpt`、`WindowFuncCall`、`TableFactor`、`JoinTable`、`SelectLockOpt` 的枚举与字段在 Rust 中逐项映射为 `TableSample`、`ExprKind::WindowFunction`、`TableSource`、`Join`、`SelectLockInfo`。
- Go `SetOprStmt*` 会在 SELECT、子查询和集合列表之间重新归属 `WITH`/`ORDER BY`/`LIMIT`；Rust 通过 `SetOprSemantic` 和 `SetOprSelectList` 保留相同结构意图。`pkg/parser/parser_test.rs::test_union_order_by` 专门检查内外层排序归属。
- Go 使用接口断言和 panic 风格的生成动作类型约定；Rust 用 `Box<dyn Any>` downcast、`Option` 和 `Result<bool,isize>` 表达同一边界。Rust 当前在类型不匹配时通常返回 `Ok(false)`，这是承载方式差异，不代表可以删减 Go 分支。

行为对照测试同时存在：Go 的 `pkg/parser/parser_test.go` 包含 `TestSubquery`、`TestSetOperator`、`TestUnionOrderBy`、`TestWithRollup`、`TestIndexHint`、`TestWindowFunctions`、`TestCTE` 等；Rust 的独立 `pkg/parser/parser_test.rs` 包含 `test_subquery`、`test_union_order_by`、`test_table_sample`、`test_window_functions`、`test_cte_bindings` 等，并通过 contract table 复用 Go 用例。本文只做静态分析，未声称这些测试在本次会话运行通过。

## 扩展指南

新增或修改查询语法时应沿完整链路同步，不能只在 `apply_rule` 增加一个 match arm：

1. 在 `pkg/parser/grammar/main.astergram` 修改产生式及动作需求，并以 `pkg/parser/parser.y` 的对应行为核对字段、默认值、错误与边界。
2. 为新产生式增加 `QueryRule` 变体，并在 `identify` 登记生成的稳定 `RuleId`；若变更非终结符归属，还要同步 `query_expression_aster_unit_test.rs` 的 `QUERY_NONTERMINALS` 与预期库存数。
3. 在 `apply_rule` 使用与产生式右部一致的 back 偏移和语义类型；新增跨规则载荷时放到 `parser_semantic_support.rs`，不要把测试或大型通用逻辑内嵌进本文件。
4. 保持 Go 语义：尤其注意字段原文偏移、`ALL`/`DISTINCT` 冲突、Hint warning、非法 schema、窗口帧方向与单位、JOIN 标志，以及集合运算括号内外 `WITH`/`ORDER BY`/`LIMIT` 的归属。
5. 测试放在独立文件。规则所有权/结构测试优先扩展 `pkg/parser/parser_actions/query_expression_aster_unit_test.rs`；端到端 AST 行为优先扩展 `pkg/parser/parser_test.rs`，并与 `pkg/parser/parser_test.go` 对应测试或 contract table 保持一致。不要在 `query.rs` 内加入 `#[cfg(test)]` 测试模块。

兼容风险主要是 AST 字段遗漏、默认值变化和集合运算层级改变；性能风险主要来自列表分支不必要的深拷贝及大查询中的反复向量扩容；诊断兼容风险来自把 warning 升级为 error 或改变错误码/参数。扩展后应检查稳定 RuleId 唯一所有权与 235 库存基线是否按预期变化。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/parser/parser_actions/query.rs` 显示本文件有 242 个符号；`query QueryRule` 定位枚举、`identify`、`apply_rule`；`query apply_rule --kind function --json` 确认本函数签名和第 741 行位置。私有函数的 `callers/callees` 查询在时限内没有产生可用输出，因此调用边改由入口源码验证，未把空图结果当成“无调用者”。
- Rust 源与 crate 边界：`pkg/parser/parser_actions/query.rs`、`pkg/parser/parser_actions/mod.rs`、`pkg/parser/parser_semantic_support.rs`、`pkg/parser/Cargo.toml`、`pkg/parser/grammar/main.astergram`。
- Go 对照：`pkg/parser/parser.y` 的 `Field`、`SelectStmt*`、`TableSample*`、窗口、表引用/连接、锁和 `SetOpr*` 动作；生成展开可在 `pkg/parser/parser.go` 追溯。
- 独立 Rust 测试：`pkg/parser/parser_actions/query_expression_aster_unit_test.rs`（235 条规则库存、唯一归属及无数字回退）、`pkg/parser/parser_actions/remaining_aster_unit_test.rs`（全动作规则唯一所有者）、`pkg/parser/parser_test.rs`（Hint、子查询、UNION 排序、TABLESAMPLE、窗口函数、CTE 等行为）。
- Go 回归测试：`pkg/parser/parser_test.go` 中的 `TestHintError`、`TestOptimizerHints`、`TestSubquery`、`TestSetOperator`、`TestUnionOrderBy`、`TestWithRollup`、`TestIndexHint`、`TestWindowFunctions`、`TestCTE`、`TestCTEBindings`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付时仅执行任务指定的 11 章节结构检查，并人工复核文档回答了文件存在原因、运行路径和安全扩展点。
