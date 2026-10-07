# `pkg/parser/parser_semantic_support.rs`

## 文件定位

本文件是 `astersql-parser` crate 的解析器共享语义支持层。它不是独立模块：[`pkg/parser/parser.rs`](parser.rs) 先 `include!` `parser_value.rs`，再包含本文件，随后声明 `parser_actions` 并包含 `parser_runtime.rs`。因此这里的私有类型和函数与 LR 解析器语义值 `yySymType`、各类 `parser_actions::*::apply_rule` 处在同一父模块作用域，作用是把词法器产生的动态值和规约栈项目转换成 `parser_ast` 节点，并集中执行若干必须在构造 AST 时完成的兼容性校验。

crate 边界由 [`pkg/parser/Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-parser`，库入口是 `lib.rs`；本文件直接使用同 crate 已接入的 `parser-ast`、`parser-auth`、`parser-mysql`、`parser-terror`、`parser-test-driver` 与 `parser-types` 等路径依赖。文件顶部虽保留 goyacc 生成文件声明与原始许可证，但当前文件实际承担的是从大型生成解析器中拆出的共享语义类型和手写辅助逻辑，真正的动作分发和 LR 状态机分别在 `parser_actions/` 与 `parser_runtime.rs`。

## 核心职责

1. 为语法规约提供有类型的中间载体：例如建表列/约束、字段类型选项、子查询、集合运算、角色或权限、文本字面量等，避免每个动作重复拼装松散的 `Box<dyn Any>`。
2. 规范化嵌套 `SELECT`/集合运算和子查询语义值，使集合运算动作能保留或移动 `WITH`、`ORDER BY`、`LIMIT` 等字段。
3. 在 AST 进入上层前复刻 Go 行为校验：Go duration 的语法与 `int64` 纳秒范围、生成列非法选项、分区方法/定义/列数/子分区数量一致性。
4. 将词法值的运行时类型转换为文本、精确 AST 字面量或 `isize`，并将 FROM 项限定为 `TableSource` 或 `Join`。
5. 提供唯一公开函数 `getMaskingPolicyRestrictOp`，把大小写不敏感的脱敏策略限制名映射为 AST 枚举。

这些职责只发生在 SQL 解析和 AST 构造阶段；本文件不负责词法扫描、LR 状态推进、语法规则选择、执行计划或 SQL 执行。

## 主要符号

- `likeEscapeSpec { escape, explicit }`：LIKE/ESCAPE 规约的中间值，记录转义串以及是否显式给出；由 `parser_actions/expression.rs` 构造和消费。
- `insertRowAlias { rowAlias, columnAliases }`：`INSERT ... AS alias(columns...)` 的行别名及列别名；由 `parser_actions/dml.rs` 使用。
- `CreateTableElementsSemantic` 与 `CreateTableSemantic`：分别累计建表列/约束，以及携带整条 `CreateTableStmt`；`complete` 标记规约产物是否完整，使用点位于 `parser_actions/ddl.rs`。
- `TypeOptSemantic`、`FloatOptSemantic`、`OptBinarySemantic`、`VectorElementTypeSemantic`、`IndexNameAndTypeSemantic`：承载字段类型的 `UNSIGNED`/`ZEROFILL`、浮点长度与小数位、binary/字符集、向量元素类型、索引名和索引类型。
- `SubquerySemantic { query }`：持有可被取得所有权的 `parser_ast::NodeRef` 子查询节点；`take_subquery_statement` 从 `yySymType.item` 中消费它。
- `GroupBySemantic { items, rollup }`：同时保存 `GROUP BY` 项和尾随 `WITH ROLLUP`。
- `RoleOrPrivSemantic::{Priv, Role, Dynamic}`：区分静态权限元素、角色身份和动态权限名，由 security/admin 动作分派。
- `TextStringSemantic { value, binary }`：保留文本字面量及 binary 属性。
- `SetOprSemantic { nodes, operators }`：累计 UNION/INTERSECT/EXCEPT 的节点序列和与节点对齐的可选操作符。
- `ShowLikeSemantic(ExprNode)`：SHOW LIKE 过滤表达式的新类型包装，避免与其他动态表达式语义混淆。
- `nested_set_op_list(statement, retain_select_with)`：把 `SelectStmt` 或 `SetOprStmt` 规范化为 `SetOprSelectList`；其他节点返回 `None`。
- `validate_go_duration`：复刻 Go `time.ParseDuration` 所需的字面量解析、单位识别、溢出检查与错误文本。
- `validate_column_def`、`validate_partition_clause`、`validate_partition_options`：返回 `errors::Error` 的 DDL 校验器，错误码来自 `mysql`，错误实例由 `terror::ClassDDL` 构造。
- `getMaskingPolicyRestrictOp`：公开的脱敏限制名解析器；未知名称返回 `(MaskingPolicyRestrictOpNone, false)`。
- `semantic_value_text`、`semantic_value_expr`、`semantic_numeric_isize`：分别执行动态值到文本、保留类型的 AST 表达式、整数的转换。
- `result_set_from_item`：仅接受 `TableSource` 或 `Join`，克隆后包装为 `ResultSetNode`。

## 执行流程

典型调用从 `Parser::ParseOneStmt` 进入 LR runtime。规约命中后，`parser_actions` 中对应的 `apply_rule` 从 RHS 的 `yySymType` 读取 `item`/`ident`/`expr`/`statement`，调用本文件的类型或助手，最后把产物写回输出语义值；助手失败时动作通常返回 `Ok(false)` 表示类型/形状不匹配，或向 lexer 追加错误并以 `Err(1)` 中止解析。

集合运算路径中，`parser_actions/query.rs::apply_rule` 先用 `take_subquery_statement` 取得子查询所有权，再调用 `nested_set_op_list`。若输入是 `SelectStmt`，函数创建单元素列表，并按 `retain_select_with` 决定是否把同一个 `With` 共享引用也放到列表层；若输入是 `SetOprStmt`，则拆出其 `select_list`、`With`、`OrderBy`、`Limit` 重建列表。初始括号操作数传 `false`，其他嵌套集合分支传 `true`，对应测试验证了 WITH 应保留在哪一层。

duration 路径中，`validate_go_duration` 先按 Go `time.quote` 的字节规则生成错误文本，再处理可选正负号；除裸 `0` 外，它循环解析“整数和/或小数 + 单位”的连续段。合法单位是 `ns`、`us`、`µs`、`μs`、`ms`、`s`、`m`、`h`。每段以 `2^63` 为边界检查整数、单位乘法、分数贡献及总和；正数必须严格小于该边界，负数允许恰好达到该边界，从而覆盖 `i64::MIN`。admin 动作把错误嵌入具体 SQL 选项消息后交给 lexer。

DDL 路径中，`validate_column_def` 先判断是否存在 `Generated`，再从后向前找 `AUTO_INCREMENT`、`ON UPDATE`、`DEFAULT` 中最后出现的非法选项并生成 MySQL 1221 错误。`validate_partition_options` 先核对显式分区数、各分区的子分区数及子分区声明，随后补齐 HASH/KEY 默认分区数、检查 RANGE/LIST 必须有定义和 SYSTEM_TIME 至少两个定义，最后逐一定义调用 `validate_partition_clause` 核对子句种类与表达式列数。

字面量路径中，`parser_actions/expression.rs::apply_rule` 对 lexer 的动态值调用 `semantic_value_expr`。函数按具体类型生成 Null、Bool、Int、Uint、Float、String、Decimal、Hex 或 Bit AST 节点；字符串、十六进制和位串同时携带 parser 当前字符集/排序规则。未支持的类型会立即 panic，表明词法器与语义动作契约被破坏，而不是把未知值静默降级。

## 数据与状态

本文件没有进程级可变业务状态。主要状态是单次解析栈中的所有权对象：`yySymType.item` 为 `Option<Box<dyn Any>>`，中间语义结构通过 downcast 恢复类型。`take_subquery_statement` 使用 `take()` 消费 item，再消费 `SubquerySemantic.query`，成功后原栈位不再持有该节点；其他读取型助手多用 `downcast_ref` 并在需要写入新 AST 时克隆。

`SetOprSemantic.nodes` 与 `operators` 维持位置对应关系，首节点通常对应 `None`，后续节点记录前置集合操作符。`nested_set_op_list` 对 `SelectStmt.With` 使用 `clone`；该字段是共享引用时，列表层和 SELECT 层仍指向同一 WITH 对象，独立测试用 `Rc::ptr_eq` 和一次可见于两处的修改验证该不变量。

唯一静态值是 `AST_ERR_WRONG_USAGE` 与 `AST_ERR_UNKNOWN_CHARACTER_SET` 两个 `LazyLock<Box<terror::Error>>`。前者由 `validate_column_def` 使用；后者由 `parser_actions/expression.rs::apply_rule` 在字符集 introducer 无法取得默认 collation 时生成“不支持的字符 introducer”错误。两者在首次访问时构造，之后只作为生成带参数错误的模板读取。

## 依赖与调用关系

上游接线为 `lib.rs` 暴露 parser crate，`parser.rs` 通过 `include!` 把本文件并入解析器父模块，`parser_runtime.rs` 在规约时调度 `parser_actions`。RustCodeGraph 的直接调用证据包括：

- `parser_actions/query.rs::apply_rule` 调用 `nested_set_op_list`、`take_subquery_statement`、`result_set_from_item`；
- `parser_actions/ddl.rs::apply_rule` 调用 `validate_column_def`、`validate_partition_options` 和 `take_subquery_statement`；
- `parser_actions/admin.rs::apply_rule` 调用 `validate_go_duration`、`semantic_value_text`、`semantic_numeric_isize`；
- `parser_actions/expression.rs::apply_rule` 调用 `semantic_value_expr`；
- `parser_actions/dml.rs`、`misc.rs`、`security.rs` 也消费子查询、整数转换或本文件定义的中间类型。

下游依赖主要是 `parser_ast` 的语句、表达式、DDL 和集合运算节点；`auth::RoleIdentity`；`mysql` 错误码与脱敏常量；`terror::ClassDDL` 错误工厂；`parser_test_driver::{MyDecimal, HexLiteral, BitLiteral}`；标准库的 `Any` 和 `LazyLock`。本文件不直接访问存储、网络、文件系统或 planner/executor。

## 错误处理与边界

`validate_go_duration` 返回字符串，以保持 Go 错误细节。空串、只有符号、缺单位、未知单位、单段或累计纳秒溢出均失败；`+0`、`-0`、小数无整数部分、整数后只有小数点、两种微秒 Unicode 写法均按 Go 接受。错误转义按 UTF-8 字节而非 Unicode 标量生成 `\xNN`，因此 `µ` 的错误详情包含两个转义字节。

DDL 校验使用 MySQL 兼容错误码。分区边界包括：RANGE/LIST 定义缺少 VALUES、方法与 LESS THAN/IN/HISTORY 不匹配、RANGE 非列分区出现多值、列分区元组宽度不一致、DEFAULT 元组例外、显式分区/子分区数量不一致，以及 SYSTEM_TIME 定义不足。校验过程中会规范化 `PartitionMethod.Num` 和 `Sub.Num`；调用方必须预期“成功校验也可能修改 options”。

动态 downcast 是明确的契约边界：`nested_set_op_list`、`take_subquery_statement`、`result_set_from_item` 在类型不符时返回 `None`；`semantic_numeric_isize` 最后尝试文本解析并返回 `None`；`semantic_value_text` 对未知类型给出占位文本 `"<value>"`；而 `semantic_value_expr` 对未知 lexer 字面量 panic。整数到 `isize` 的 `as` 转换沿用 Rust 截断语义，调用动作负责只传入语法允许范围内的值。

## 并发与资源生命周期

解析辅助逻辑是同步、单次调用的纯计算或局部 AST 变换，不创建线程、异步任务、锁、通道、事务或外部句柄。两个 `LazyLock` 由标准库保证并发首次初始化安全；初始化后没有写操作。

资源生命周期由 Rust 所有权管理：动态语义值随解析栈创建和释放，`take()`/`downcast()` 将所有权移入最终 AST，失败的 downcast 不产生悬垂引用；借用型转换只在调用期间读取。集合运算重建会移动节点向量，避免复制整棵子树；必要的 AST `clone` 和 `With` 共享引用应保持别名语义，扩展时不能改成深拷贝而破坏同一 CTE 节点的共享关系。

## 与 Go 版本的对应关系

直接一一对应的部分包括：`likeEscapeSpec`、`insertRowAlias` 和 `getMaskingPolicyRestrictOp` 对应 [`pkg/parser/parser.y`](parser.y) 及其生成的 `parser.go`；脱敏限制名先转大写，再映射四种合法操作，未知值返回 None/false。`validate_go_duration` 对应语义动作中的 `time.ParseDuration`，Rust 本地实现保留 Go 的单位、范围和错误文本。

`validate_column_def` 对应 [`pkg/parser/ast/ddl.go`](ast/ddl.go) 的 `ColumnDef.Validate`：仅当列含生成表达式时，`ON UPDATE`、`AUTO_INCREMENT`、`DEFAULT` 才非法。`validate_partition_clause` 和 `validate_partition_options` 对应同一 Go 文件中各 `PartitionDefinitionClause.Validate` 与 `PartitionOptions.Validate` 的检查和计数规范化；Rust 将这些校验放在 parser 共享层，由 DDL 动作构造节点后立即调用。

其余多数 `*Semantic` 类型、`semantic_value_*` 和所有权提取助手是 Rust 适配层，并非 Go 中都有同名结构。Go 的 yacc `%union` 使用 `interface{}` 和类型断言，动作中直接构造 AST；Rust 使用 `Box<dyn Any>`、显式 downcast 与具名中间结构表达相同规约意图。`semantic_value_expr` 对应 Go test driver 的 `newValueExpr`/`DefaultTypeForValue` 组合，关键要求是不能把 decimal、hex、bit、unsigned 等值先统一文本化而丢失 SQL datum 种类。

## 扩展指南

- 新增 lexer 字面量类型时，优先扩展 `semantic_value_expr`，必要时同步 `semantic_value_text` 和 `semantic_numeric_isize`；保持字符集/排序规则、signed/unsigned、decimal/hex/bit 类型信息，并在独立测试文件中覆盖未知类型策略。
- 新增或修改 DDL 约束时，在 `validate_column_def`、`validate_partition_clause` 或 `validate_partition_options` 中对齐 Go 对应增量和 MySQL 错误码；若校验同时规范化字段，要记录修改时机并覆盖输入 AST 的前后状态。
- 扩展集合运算时同时检查 `SetOprSemantic`、`nested_set_op_list` 和 `parser_actions/query.rs` 的操作符/节点对齐，以及 WITH、ORDER BY、LIMIT、`AfterSetOperator` 的归属。尤其要保留首个括号操作数与后续括号操作数不同的 `retain_select_with` 语义。
- 新增中间语义结构应放在本文件、消费逻辑放在相应 `parser_actions/<domain>.rs`；不要把 Rust 单元测试嵌入生产文件。优先扩展同目录独立的 `parser_semantic_support_test.rs`，跨规则动作可扩展相应 `parser_actions/*_test.rs`。
- 修改前后需核对 [`pkg/parser/parser.y`](parser.y)、Go AST 校验和 Rust 动作调用点；动态类型生产者与消费者必须同步修改，否则会走 `None`、占位文本或 panic 边界。性能风险主要来自不必要的 AST 深克隆、频繁字符串格式化和大分区定义列表的重复遍历。

## 验证依据

- RustCodeGraph 索引状态和文件查询确认目标文件已索引；`node --file pkg/parser/parser_semantic_support.rs` 读取了 1–587 行全部源码。
- RustCodeGraph `explore` 确认主要调用边：query 动作调用 `nested_set_op_list`/`take_subquery_statement`/`result_set_from_item`，DDL 动作调用列与分区校验，admin 动作调用 duration 与数值转换，expression 动作调用字面量转换。
- 已读接线与调用文件：[`pkg/parser/parser.rs`](parser.rs)、[`pkg/parser/lib.rs`](lib.rs)、`pkg/parser/parser_actions/query.rs`、`ddl.rs`、`admin.rs`、`expression.rs`；精确使用搜索还覆盖 `dml.rs`、`misc.rs`、`security.rs`。
- 已读 crate/Go 对照：[`pkg/parser/Cargo.toml`](Cargo.toml)、[`pkg/parser/parser.y`](parser.y)、[`pkg/parser/parser.go`](parser.go)、[`pkg/parser/ast/ddl.go`](ast/ddl.go)。
- 已读独立 Rust 测试 [`pkg/parser/parser_semantic_support_test.rs`](parser_semantic_support_test.rs)：覆盖 duration 的 `i64` 纳秒边界、分数与微秒单位、错误详情，分区 MySQL 1479 错误，以及嵌套集合运算 WITH 的保留位置和共享引用；`parser_3_aster_unit_test.rs` 另覆盖生成列 1221 错误文本。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以固定标题结构命令和 diff 人工复核验证文档完整性、链接与事实边界。
