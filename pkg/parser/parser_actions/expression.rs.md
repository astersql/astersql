# `pkg/parser/parser_actions/expression.rs` 逻辑说明

## 文件定位

`expression.rs` 是 `astersql-parser` crate 中主 SQL 文法的“表达式语义动作”实现。crate 边界由 `pkg/parser/Cargo.toml` 定义，库入口是 `pkg/parser/lib.rs`；本文件作为私有模块由 `pkg/parser/parser_actions/mod.rs` 声明，不直接向 crate 外导出 API。

它处在生成式解析器和 AST 之间：`pkg/parser/parser_runtime.rs` 在 LR 归约时取得稳定的 `RuleId`、右部语义值 `Rhs`、待写入的 `yySymType`、`Parser` 状态与 `yyLexer`，然后调用 `parser_actions::apply`。总分派器按 `mview → ddl → dml → expression → query → security → admin → misc` 的顺序检查所有权，表达式规则命中后才进入本文件的 `owns`/`apply`/`apply_rule`。

这里不是词法分析器，也不执行表达式。它只把 `pkg/parser/grammar/main.astergram`（Rust 当前文法）或同源的 Go `pkg/parser/parser.y` 中一次归约的语义值，构造成 `parser_ast::ExprNode`、`parser_types::types::FieldType` 或供后续归约消费的中间值。最终 AST 会被 planner、expression 等上层模块使用，但那些消费者不是本文件的直接调用者。

## 核心职责

- 以 `identify` 将生成器提供的稳定、带哈希的规则字符串映射到私有枚举 `ExpressionRule`，避免依赖易变化的 yacc 数字规则号。
- 以 `apply_rule` 实现表达式文法的 AST 构造：列名、字面量、变量、算术/逻辑/比较谓词、子查询、函数和聚合、CASE、CAST、日期时间单位、JSON 运算等。
- 维护归约间的中间语义值。AST 节点写入 `yySymType.expr`；列表、布尔开关、操作符、类型和枚举等写入类型擦除的 `yySymType.item`；标识符结果写入 `yySymType.ident`。
- 在构造阶段完成 Go 文法要求的局部验证和归一化，例如字符集/排序规则检查、CAST 精度检查、`LIKE ... ESCAPE` 长度检查、`SignedNum` 的 `i64` 边界检查，以及系统变量作用域解析。
- 保持当前 Rust action 清单完整且与其他 action 模块互斥。`pkg/parser/parser_actions/query_expression_aster_unit_test.rs::query_expression_rule_coverage` 从 `grammar/main.astergram` 计算出 288 个需要 action 的表达式产生式，并逐个要求 `expression::owns` 命中。

## 主要符号

- `ExpressionRule`：私有、可比较的枚举，共 291 个 Rust 变体。变体按非终结符和候选编号命名；部分变体可服务共享动作或兼容别名，因此它不应被当作“文法产生式数”的替代，权威覆盖数由测试从文法计算为 288。
- `identify(rule_id: RuleId) -> Option<ExpressionRule>`：规则所有权表。输入是生成器的稳定规则 ID，例如 `expression_expression_logor_expression_prec_pipe--...`；未知 ID 返回 `None`。
- `owns(rule_id: RuleId) -> bool`：供 `parser_actions::apply` 和 `has_semantic_action` 使用的轻量所有权检查，仅调用 `identify`。
- `apply(rule_id, rhs, context) -> Option<Result<bool, isize>>`：本模块的窄入口。非本模块规则返回 `None`；本模块规则调用 `apply_rule` 并保留三层结果语义。
- `apply_rule(rule, rhs, context) -> Result<bool, isize>`：3089 行文件的核心分派器。它解构 `Context { output, parser_state, lexer }`，按枚举分支读取 `Rhs` 并写出语义值。成功构造返回 `Ok(true)`，缺少/类型不符的必要语义值通常返回 `Ok(false)`，已向 lexer 追加明确错误时返回 `Err(1)`。
- 直接依赖的共享符号来自父模块/解析器根：`Context`、`Rhs`、`RuleId`、`yySymType`、`SubquerySemantic`、`VectorElementTypeSemantic`、`likeEscapeSpec`、`semantic_value_expr/text`、`semantic_numeric_isize`、`getInt64FromNUM`、`getUint64FromNUM`。它们不是本文件定义的类型。

公开性边界很窄：`owns` 与 `apply` 仅为 `pub(super)`，枚举、映射及具体动作均为私有；没有 trait、模块常量、`impl` 或条件编译项。

## 执行流程

1. `pkg/parser/parser_runtime.rs` 确认归约索引、稳定 `RuleId` 和栈右部长度，计算右部语义完整性，并创建默认 `reduced_value`。
2. 运行时以 `Rhs::new(&mut values[value_start..])` 和 `Context::new(&mut reduced_value, parser_state, yylex)` 调用 `parser_actions::apply`。
3. `parser_actions/mod.rs::apply` 调用 `expression::owns`；命中后调用本文件 `apply`，后者再次 `identify` 并进入 `apply_rule`。模块装配处的 `expect("owned expression rule has an action")` 表明 `owns` 与 `apply` 必须共用同一映射，不能漂移。
4. `apply_rule` 根据规则类别构造结果：
   - 原子与名称：`ColumnName*`/`SimpleIdent*` 建立一至三段限定名；`Literal*`、`StringLiteral*`、变量规则保留 parser 的 charset/collation 与作用域信息。
   - 运算与谓词：`Expression*`、`BitExpr*`、`BoolPri*`、`PredicateExpr*` 建立 unary/binary、`IsNull`、`IsTruth`、`InList`/`InSubquery`、`Between`、`Like`/`Regexp`、子查询比较等节点，并按文法优先级使用已经归约好的左右节点。
   - 函数与聚合：`BuiltinFunction*`、`FunctionCall*`、`SumExpr*` 规范函数名和参数顺序；日期时间、TRIM、GET_FORMAT 等专用语法把单位/方向编码为专用 `ExprKind` 参数。
   - 复合表达式：CASE 读取 `WhenClause` 列表；行构造读取表达式向量；EXISTS/子查询会设置多行或 exists 语义；JSON 箭头被规范成 `JSON_EXTRACT`，双箭头再包 `JSON_UNQUOTE`。
   - 类型转换：`CastType*` 生成 `FieldType`，`SimpleExprAlt23/24/25/27` 再组装 BINARY/CAST/JSON_SUM_CRC32/CONVERT 节点，并补默认 flen/decimal、array 与显式字符集信息。
5. 构造成功写入 `out` 后返回 `Ok(true)`。显式错误返回 `Err(1)`，运行时立即以该状态结束；`Ok(false)` 则由运行时进入默认语义值路径，但由于 `has_semantic_action` 仍为真，会把 `semantic_complete` 标为假，避免把缺失 action 当成有效完成值。

## 数据与状态

`Rhs<'_>` 是解析栈当前产生式右部的可变借用。本文件大量采用 `rhs[rhs_len - back]` 读取相对位置；仅在所有权必须转移时使用 `take()`，例如消费 `SubquerySemantic` 或转移 `SignedNum` 的 item。`parser_actions/mod.rs` 为 `Rhs` 实现的索引若越界会以 `expect("semantic RHS position")` 触发 panic，因此规则 ID、生成文法与偏移必须同步修改。

`yySymType` 同时承载多种通道：

- `expr: Option<ExprNode>` 是主要 AST 通道；
- `item: Option<Box<dyn Any>>` 保存列表、`FieldType`、`TimeUnitType`、布尔标志、操作符字符串等，消费端必须用精确类型 `downcast_ref/downcast`；
- `ident` 保存标识符、字符集或排序规则名；
- `semantic_complete` 由运行时维护，本文件通过返回值间接影响它。

可变共享状态只有 `Context` 中的三个借用。`output` 写当前归约结果；`lexer` 累积用户可见错误；`parser_state` 读取默认 `charset`/`collation`，并在 CAST/CONVERT 处理时读取后清除 `explicitCharset`。清除动作是一个跨产生式不变量：显式字符集只属于当前转换，不得泄漏到下一个表达式。

AST 多数节点拥有子节点（`Box`/`Vec`），因此归约完成后不再依赖栈元素生命周期。列表动作通常克隆已有向量再追加；这保持实现简单，但扩展超长表达式列表时应关注重复克隆的成本。

## 依赖与调用关系

上游直接调用链为：

`parser_runtime::yyParse` → `parser_actions::apply` → `expression::owns` → `expression::apply` → `identify` / `apply_rule`。

`parser_actions::has_semantic_action` 也直接调用 `expression::owns`，用于完整性标记和全量 action 注册测试。RustCodeGraph 能定位 `ExpressionRule`、`identify`、`apply_rule` 及目标文件被 36 个文件间接使用；但对重名的私有 `apply` 执行 `callers/callees` 没有返回可用边，因此上述直接边由 `pkg/parser/parser_actions/mod.rs` 与 `pkg/parser/parser_runtime.rs` 的实际调用语句交叉确认。

主要下游依赖如下：

- `parser-ast`：`ExprNode`/`ExprKind`、`ColumnName`、`WhenClause`、CAST/时间单位/聚合相关 AST 类型；
- `parser-types` 与 `parser-mysql`：`FieldType`、MySQL 类型码、flag、CAST 默认长度和精度；
- `parser-charset`：字符集、默认排序规则与名称校验；
- `parser-terror` 及解析器根导入的错误常量：构造 MySQL 兼容错误；
- `Parser`/`yyLexer`：会话级解析选项、SQL mode、字符集状态和错误汇聚。

这些都是 `pkg/parser/Cargo.toml` 声明的本地 parser 子 crate；本文件没有自行引入网络、存储、执行器或异步运行时依赖。

## 错误处理与边界

返回值必须区分三类情况：`None` 表示规则不归本模块；`Some(Ok(true))` 表示 action 已构造结果；`Some(Ok(false))` 表示规则属于本模块，但所需动态值缺失或类型不匹配；`Some(Err(1))` 表示已经记录语义错误，解析应失败。

明确的用户输入错误包括：

- `CharsetNameAlt01`/`CollationNameAlt01`：未知字符集或排序规则，通过 `ErrUnknownCharacterSet`/`ErrUnknownCollation` 写入 lexer；带 `_charset` 的 introduced literal 也校验默认 collation。
- `Int64NumAlt01` 与 `SignedNumAlt03`：拒绝越过有符号 64 位范围；`-9223372036854775808` 被单独映射为 `i64::MIN`。
- `CastTypeAlt12`：FLOAT CAST 精度过大时追加错误；25–53 的语义按 Go 版本提升为 DOUBLE。
- `CastTypeAlt14`/`StringTypeAlt17`：当前只支持 VECTOR FLOAT；代码会追加错误，但仍构造 vector field type，最终是否失败取决于 lexer 错误汇聚。
- `PredicateExprAlt04/05`：显式 ESCAPE 长度大于 1 时返回 `Err(1)`；空字符串是合法的“禁用 escape”表达，不能与缺省反斜杠混同。

大量 `Ok(false)` 是内部语义不完整保护，而不是用户语法错误。它覆盖缺失左右操作数、错误的 `Any` downcast、缺失子查询/表名/字段类型等情况。新增动作时不应无条件 `unwrap_or_default` 掩盖必需值；应依据 Go action 是容错缺省、内部不变量还是用户错误，选择克隆/转移、`Ok(false)` 或 `AppendError + Err(1)`。

另一个边界是规则映射：未知 `RuleId` 必须保持不拥有，让其他模块或总分派器处理；禁止恢复数字规则回退。`query_expression_modules_have_no_numeric_fallback` 明确断言本文件不含 `legacy_rule_number`、`apply_numeric`、`rhs_index`。

## 并发与资源生命周期

本文件没有线程、锁、channel、异步任务或外部资源。一次 action 通过独占 `&mut` 借用解析栈片段、输出值、`Parser` 与 lexer，Rust 借用规则保证同一次调用不会并发修改这些状态；`ExpressionRule` 和规则字符串表是只读的。

生命周期边界与一次 LR 归约一致：`Rhs`/`Context` 在 `apply_rule` 返回后释放借用；写入的 AST、字符串、向量和字段类型均由 `reduced_value` 所有。运行时随后截断已归约的栈元素，再把新的语义值压回栈。`take()` 的使用必须限于不再需要原值的分支，否则会破坏同一 action 后续读取。

资源风险主要是内存而非并发：深层表达式产生递归 AST，长参数/表达式列表会分配 `Vec` 并克隆已有内容。当前文件没有缓存或手工释放逻辑；解析器栈缓存由 `Parser`/runtime 管理，不属于本文件。

## 与 Go 版本的对应关系

Go 对照的权威文件是 `pkg/parser/parser.y`。Rust 的 `ExpressionRule` 候选与其中同名非终结符对应，`query_expression_rule_coverage` 又从 Rust 的 `grammar/main.astergram` 验证当前 action 清单，因此扩展时需要同时确认“当前 Rust 文法”与“Go 原语义动作”，不能只看枚举名。

已核对的等价语义包括：逻辑/算术/比较操作符；`IS [NOT] NULL/TRUE/FALSE`；`IN`、BETWEEN、LIKE/ILIKE/REGEXP；ANY/SOME/ALL 子查询；系统变量作用域；DATE_ADD/DATE_SUB；CAST 默认长度、array charset；字符集 introducer；FLOAT/VECTOR 限制；以及 SignedNum 最小值特例。Rust 将 Go 的大量具体 AST struct 统一表示为 `ExprNode`/`ExprKind`，并把 Go interface 语义值改为 `Box<dyn Any>`，这是表示方式差异而不是有意删减语法。

需要特别注意的可见差异/迁移状态：

- Rust 使用稳定哈希 `RuleId`，Go yacc action 直接绑定产生式；Rust 不允许退回 legacy 数字规则。
- Go `PatternLikeOrIlikeExpr.Escape` 最终保存单字节，而 Rust `ExprKind::Like` 保存 `String`；两边都拒绝长度大于 1、保留 empty-explicit 状态，但后续消费者必须继续维持兼容含义。
- Go AST 通过具体类型和 interface 断言，Rust 通过统一 enum 与动态中间 `item`；新增字段时必须核对 AST restore/visitor 与执行侧是否理解新 `ExprKind`。
- 当前模块已有 AsterSQL 处理标记（源文件顶部 `// Copyright 2026 AsterSQL.`），不是未接线桩；完整 action 注册测试也证明它已进入 runtime 主链。

Go 回归可参考 `pkg/parser/parser_test.go` 和 `pkg/parser/ast/expressions_test.go`；Rust 直接覆盖集中在 `pkg/parser/parser_actions/query_expression_aster_unit_test.rs`、`pkg/parser/yy_parser_test.rs`、`pkg/parser/parser_test.rs`、`pkg/parser/parser_3_aster_unit_test.rs` 与 `pkg/parser/parser_runtime_aster_unit_test.rs`。

## 扩展指南

新增或修改表达式语法时，建议按以下顺序保持映射、行为和验证一致：

1. 在 `pkg/parser/grammar/main.astergram` 确认产生式及其稳定 `RuleId`，并用 `pkg/parser/parser.y` 的同名 action 核对 Go 行为、错误码、优先级和默认值。
2. 在 `ExpressionRule` 增加候选，并在 `identify` 增加唯一映射；若产生式不需要自定义动作，不要仅为“看起来完整”而注册。
3. 在 `apply_rule` 选择准确的 RHS 相对位置和 `yySymType` 通道。必需动态类型应显式 downcast；需要转移所有权时才 `take()`。
4. 若引入新 AST 形态，先扩展 `parser-ast` 的节点、restore/visitor/flag 传播，再让 action 构造它；仅把新语法伪装成字符串函数名可能丢失专用语义。
5. 对 `Parser.explicitCharset` 等跨规则状态保持成对读取/复位；对 lexer 错误保持 Go 对应的 MySQL 错误及返回状态。
6. 至少更新 `query_expression_rule_coverage` 的清单/数量（若非终结符或 action 数变化），并在独立 Rust 测试文件中增加端到端 `ParseOneStmt` 断言。源文件与单元测试不得合并。

最常修改的符号是 `ExpressionRule`、`identify` 和 `apply_rule`，三者必须作为原子改动。高风险点是语法优先级、RHS 偏移、动态类型、NOT 布尔反转、字符集/SQL mode、AST flag 传播和 Go restore 文本兼容；性能风险主要来自在递归列表动作中额外克隆。若只变更 grammar 而漏掉映射，覆盖测试应报告未拥有的规则；若映射命中却 action 缺值，runtime 会把语义标记为不完整，不能把“解析返回节点”当作充分验证。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/parser/parser_actions` 确认本模块与独立测试位置。
- RustCodeGraph `node --file pkg/parser/parser_actions/expression.rs` 分段读取完整 1–3089 行；`query ExpressionRule/identify/apply_rule` 定位枚举和三层分派。`callers/callees` 对重名私有 `apply` 未给出有效结果，这一缺口由真实调用点补证，而未推测图中不存在调用。
- 源与装配：`pkg/parser/parser_actions/expression.rs`、`pkg/parser/parser_actions/mod.rs`、`pkg/parser/parser_runtime.rs`、`pkg/parser/Cargo.toml`、`pkg/parser/grammar/main.astergram`（由覆盖测试读取）。`pkg/parser` 下不存在 `doc.go`，因此无额外 package contract 可读。
- Go 对照：`pkg/parser/parser.y` 的 `Expression`、`BoolPri`、`PredicateExpr`、`Literal`、`BitExpr`、`SimpleExpr`、`CastType`、`SystemVariable`、`UserVariable`、`SignedNum` 等动作。
- Rust 测试：`pkg/parser/parser_actions/query_expression_aster_unit_test.rs`（288 个 expression RuleId、模块互斥、无数字回退）、`pkg/parser/parser_3_aster_unit_test.rs`（2153 个需要 action 的主文法规则全部注册）、`pkg/parser/parser_runtime_aster_unit_test.rs`（稳定 RuleId 调度）、`pkg/parser/yy_parser_test.rs`（表达式 flag、聚合/子查询/变量/默认值/谓词）以及 `pkg/parser/parser_test.rs` 的端到端解析集合。
- Go 测试：`pkg/parser/parser_test.go` 与 `pkg/parser/ast/expressions_test.go`，用于确认 Go parser/AST 的既有语义面；本次是纯文档分析，按任务约束未运行 Cargo 或测试二进制。

结构验收使用任务指定命令，要求文档存在且固定的十一个二级标题恰好各出现一次。人工复核重点是：文件为何存在、runtime 如何进入它、每种输出/错误如何传播、Go 对照何在，以及扩展时需同步哪些符号与独立测试。
