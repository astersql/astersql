# `pkg/parser/ast/expressions.rs`

## 文件定位

`expressions.rs` 是 `astersql-parser-ast` crate 的表达式 AST 兼容层。crate 入口通过 `pkg/parser/ast/lib.rs:4973-4975` 以公开模块 `expressions` 装入本文件；crate 本身由 `pkg/parser/ast/Cargo.toml` 定义，未为本模块声明单独 feature。文件头说明其来源是同目录的 Go 实现 `pkg/parser/ast/expressions.go`。

它位于“语法树已经形成、需要保存/遍历/重新输出表达式”的边界，而不是表达式求值器：本文件不执行 SQL 语义，不访问存储，也不负责完整 SQL 解析。当前 Rust 代码把表达式表示为 `Expr` 枚举，提供 SQL 还原、结构遍历、名称匹配和结构相等比较。RustCodeGraph 将本文件识别为 1246 行、199 个符号，并显示被 52 个文件使用；可确认的直接生产接线包括 `pkg/types/parser_driver/value_expr.rs` 对 `ValueExpr`/`ParamMarkerExpr` 的实现，以及 `pkg/expression/util.rs` 的 `ParamMarkerInPrepareChecker`。

## 核心职责

1. **定义表达式节点集合。** `Expr` 覆盖字面量、原始 SQL、二元/一元运算、`BETWEEN`、`CASE`、子查询、列名、`IN`、`LIKE/ILIKE`、正则、行构造、变量、全文检索和 `COLLATE` 等 25 种变体（`expressions.rs:703-731`）。各复合节点用 `Box<Expr>`、`Vec<Expr>` 或 `Option` 明确所有权和可选分支。
2. **把 AST 还原成规范 SQL。** `Expr::restore` 是统一分派入口；各节点的私有 `restore` 负责关键字、标识符引用、列表分隔和标志位处理。`RestoreCtx` 保存输出与父运算上下文，`Op::precedence`/`is_associative_with` 决定显式括号能否安全省略。
3. **提供可变深度优先遍历。** `Expr::accept` 遵循 `enter -> children -> leave`，`enter` 返回 `true` 时跳过子树；列名、表名和参数占位符有专门回调。
4. **维持 Go 迁移接口。** `ValueExpr`、`ParamMarkerExpr`、`ExpressionDeepEqual` 等名字保留 Go 侧概念；`pkg/types/parser_driver/value_expr.rs:448-497` 将真实参数标记接到这里的 trait。

本文件不是完整 Go AST 的等价替换。尤其 `SubqueryExpr::query` 仅保存 `String`，`Expr::Raw` 可直接输出未验证文本，且若干 Go 节点元数据和格式化接口没有进入这套 Rust 类型。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `RestoreResult<T>` | 公开别名，当前错误载荷是 `String`。 |
| `RestoreFlags` | 公开位集合；控制二元运算空格/括号、`BETWEEN` 括号、冗余括号省略以及列名 schema/table 前缀。位运算仅实现合并，没有公开清位操作。 |
| `RestoreCtx` | 公开构造与结束接口；内部持有输出、父运算符/左右侧、一元上下文，公开 `flags` 与小写 CTE 名集合 `cte_names`。 |
| `Op` | 逻辑、比较、算术、位、一元和 `COLLATE` 运算符；私有 `sql`、`precedence`、`is_keyword`、`is_associative_with` 驱动还原。 |
| `CiString` / `ColumnName` / `TableName` | 保存标识符原文和 ASCII 小写副本；`ColumnName::restore` 处理限定名前缀，`matches` 允许空 schema/table 作通配。 |
| `Value` | `NULL`、布尔、整数、无符号整数、浮点文本、字符串；浮点保留文本，字符串固定输出 `_UTF8MB4` 前缀。 |
| `ValueExpr` / `ParamMarkerExpr` | 对运行期值、投影偏移和预处理参数顺序/偏移/执行状态的对象安全接口；`clone_box` 支持 trait object 克隆。 |
| 节点结构体 | `BetweenExpr`、`BinaryOperationExpr`、`CaseExpr`、`SubqueryExpr`、`CompareSubqueryExpr`、`PatternInExpr`、`PatternLikeOrIlikeExpr`、`PositionExpr`、`VariableExpr`、`MatchAgainst` 等保存各语法形态的数据。 |
| `Expr` | 本文件的中心代数类型；`try_to_sql*`、`to_sql*`、`restore`、`accept` 是主要入口。 |
| `Visitor` | 可变访问器协议；默认列名/表名/参数标记回调保持节点不变并继续成功。 |
| `expression_deep_equal` / `ExpressionDeepEqual` | 当前都直接调用派生的 `Expr::eq`；后者只是 Go 风格命名别名。 |

文件没有条件编译项。测试由 `pkg/parser/ast/lib.rs:5033-5039` 在 crate 测试配置下从独立文件装入，没有把测试逻辑放进生产源文件。

## 执行流程

SQL 还原主流程如下：

1. 调用者通过 `Expr::try_to_sql_with_flags` 创建 `RestoreCtx`，或复用已有上下文直接调用 `Expr::restore`。
2. `Expr::restore` 按枚举变体分派。叶节点直接写入：`Value` 规范化字面量，`Raw` 原样追加，名称通过反引号转义；复合节点递归还原子表达式。
3. 二元类节点通过 `restore_binary_child` 临时记录父 `Op` 和子节点处于左侧还是右侧。`ParenthesesExpr::restore` 在启用 `SKIP_REDUNDANT_PARENTHESES` 时调用 `can_restore_without_parentheses`：子优先级更高可省略，优先级更低必须保留，同级仅左子或可结合运算可省略。
4. 显式括号和子查询通过 `with_reset_parent_context` 隔离父运算/一元状态，避免外层优先级错误影响内部文本。
5. 成功后 `RestoreCtx::finish` 交出字符串；`try_to_sql*` 返回 `Result`，`to_sql*` 则在错误时 panic。

Visitor 流程从 `Expr::accept` 开始。它先调用 `visitor.enter(self)`；若要求跳过子节点，立即执行 `leave`。否则按语法顺序递归，例如 `BetweenExpr` 依次访问被比较表达式、下界、上界，`CaseExpr` 依次访问可选比较值、每个 WHEN 条件和结果、ELSE。任一子节点或 `leave` 返回 `false` 都会短路。`PositionExpr` 中的动态参数标记调用其对象安全 `accept`，最终转入 `enter_param_marker`/`leave_param_marker`；`pkg/expression/util.rs:1271-1292` 借此检测 Prepare 中参数状态。

## 数据与状态

AST 自身全部由值类型、`Box`、`Vec`、`Option` 和 `String` 拥有，没有借用外部缓冲。`Expr` 及大多数节点派生 `Clone + Eq + PartialEq`，因此克隆是深克隆；`PositionExpr` 因含 trait object，手工克隆参数标记，并以 `offset/order/in_execute` 三元组实现相等与调试输出。

`RestoreCtx` 是一次还原过程中的可变状态：

- `output` 单调追加 SQL 文本；`finish(self)` 消耗上下文。
- `parent_binary_op`、`parent_binary_side` 和 `in_unary_operation` 是栈式临时状态。辅助函数在递归后恢复旧值，包括错误返回路径。
- `flags` 通常由调用者设定；`BinaryOperationExpr::restore` 在强制二元括号时临时加入 `BRACKET_AROUND_BETWEEN`，结束后恢复原标志。
- `cte_names` 由调用者填入小写名称；当列名的 table 命中 CTE 时，`ColumnName::restore` 不输出 schema。

部分字段是后续阶段或缓存状态而非纯语法：`SubqueryExpr` 带 `evaluated/correlated/multi_rows/exists`，`PatternLikeOrIlikeExpr` 带编译辅助数组，`PatternRegexpExpr` 带编译模式与原表达式文本，`PositionExpr` 可持有运行期参数对象。当前派生相等会把这些字段纳入比较（参数对象除外，见上文三元组规则）。

## 依赖与调用关系

crate 边界由 `pkg/parser/ast/Cargo.toml` 给出：本 crate 直接依赖相邻的 parser auth/charset/mysql/types crate 以及 `serde`、`serde_json`、`url`；但本文件源码只使用 Rust 标准库的 `Any`、`HashSet`、格式化和位运算 trait，没有直接调用这些外部 crate。

上游与接线证据：

- `pkg/parser/ast/lib.rs` 公开 `expressions` 模块，并从独立测试文件装入还原测试。
- `pkg/types/parser_driver/value_expr.rs:448-497` 为真实 `ParamMarkerExpr` 实现本模块的两个 trait，其 `accept` 调用专用参数标记回调。
- `pkg/expression/util.rs:1271-1292` 实现本模块的 `Visitor`，用 `in_execute()` 判断 Prepare 参数状态；同文件的 `ConstructPositionExpr` 把驱动参数装进 AST 的位置表达式。
- RustCodeGraph 的文件级关系还列出 `dumpling/export/schema_projection.rs`、`pkg/bindinfo/binding.rs`、`pkg/ddl/create_table.rs`、`pkg/expression/builtin.rs`、`pkg/expression/scalar_function.rs` 等使用者，但图查询没有为通用 impl 方法稳定解析出逐方法 callers，因此这里不把文件级关系夸大为已验证的具体调用路径。

下游全部位于本文件：`Expr::restore` 调用节点还原函数，节点函数调用 `restore_binary_child`、`restore_list`、`with_reset_parent_context` 和 `RestoreCtx::write_*`；`Expr::accept` 调用子 `Expr::accept` 或名称/参数回调。没有 I/O、网络、锁或后台任务依赖。

## 错误处理与边界

可恢复错误经 `RestoreResult` 向上传播。目前生产路径中明确构造的业务错误只有 `MatchAgainst::restore`：`BOOLEAN_MODE | QUERY_EXPANSION` 同时出现时返回 `BOOLEAN MODE doesn't support QUERY EXPANSION`。`try_to_sql*` 保留该错误；`to_sql*` 用 `expect` 转成 panic，故处理不可信或可能非法的 AST 时应优先使用 `try_to_sql*`。

重要边界包括：

- `Expr::Raw` 与 `SubqueryExpr::query` 不解析、不转义内容，调用者必须保证文本可信且语法完整。
- `Value::Float` 原样输出字符串，不验证是否为合法数值；`Value::String` 只把单引号加倍，并固定 `_UTF8MB4`。
- `RestoreFlags::contains` 实际判定“任一共享位”，在当前调用处传入的都是单一标志；未来若用组合标志查询，不能把它误解成“包含全部位”。
- `ColumnName::matches` 仅将接收者一侧的空 schema/table 当通配；调用顺序会影响结果。
- `VariableExpr` 在系统变量显式 scope 下按 global、instance、否则 session 的优先顺序输出；名称始终作为反引号标识符输出。
- `accept` 的布尔值同时承担短路信号；访问器改写节点时须维护枚举变体和子节点结构不变量。
- 当前 `expression_deep_equal` 是完整派生相等；它不复现 Go 版先清除文本位置并规范函数名大小写再比较的行为。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁、事务、文件句柄或网络资源。`RestoreCtx`、`Visitor` 和 AST 都通过 `&mut` 串行访问；类型没有显式承诺跨线程共享，动态参数对象的 trait 也没有 `Send`/`Sync` 约束。若调用者需要并行处理，应为每个任务使用独立 AST/上下文，或自行在外层同步，不能共享同一个可变 `RestoreCtx`。

生命周期由所有权决定：递归节点随根 `Expr` 一起释放；`finish` 消耗上下文并转移字符串；临时父运算状态在辅助函数返回前恢复。`PositionExpr` 克隆时由 `ParamMarkerExpr::clone_box` 创建独立对象，但具体参数值是否具有更深层共享语义取决于 `pkg/types/parser_driver/value_expr.rs` 的实现，不由本文件保证。

## 与 Go 版本的对应关系

Rust 节点名和主要还原文本直接对照 `pkg/parser/ast/expressions.go`。`pkg/parser/ast/expressions_test.rs:931-1088` 复刻了 Go 测试中的一元运算、列名、真假/空值、`BETWEEN`、`CASE`、二元运算、括号、`IN`、`LIKE`、正则、行、变量和全文检索用例；对应 Go 表格位于 `pkg/parser/ast/expressions_test.go:108-421`。`pkg/parser/ast/expressions_4_aster_unit_test.rs` 另验证优先级省略、CTE/schema 规则、全文修饰符冲突和 Visitor 顺序。

已经对齐的核心语义包括：关键字大写、标识符反引号转义、二元运算空格标志、`BETWEEN`/二元括号联动、优先级与结合律、`LIKE` 转义输出、变量 scope、全文检索修饰符互斥，以及 Visitor 的 enter/skip/leave 顺序。

仍需明确的差异：

- Go 各节点实现统一 `Node`/`ExprNode`，Visitor 可返回替换后的节点；Rust 使用闭合 `Expr` 枚举与 `&mut` 原地修改，不能由回调返回不同动态节点。
- Go `SubqueryExpr` 持有 `ResultSetNode`，Rust 仅持有查询字符串和状态布尔值。
- Go `Restore` 使用带节点路径注释的 error，Rust 多数写入操作不可失败，仅返回字符串错误。
- Go 还提供各节点 `Format`、动态 `NewValueExpr`/`NewParamMarkerExpr` 构造钩子和更多基类元数据；本文件没有等价接口。
- Go `ExpressionDeepEqual`（`expressions.go:1687-1735`）临时清除文本位置并规范函数名原文后比较，再恢复对象；Rust `expression_deep_equal` 直接 `a == b`。因此函数名大小写、瞬态元数据和本文件缓存字段的等价规则不能假设与 Go 相同。

## 扩展指南

新增表达式种类时，至少需要同步以下位置：

1. 新增节点结构及 `Expr` 变体，并在 `Expr::restore`、`restore_op`（若属于二元优先级体系）和 `Expr::accept` 中穷尽处理。
2. 若引入新运算符，更新 `Op::sql`、`is_keyword`、`precedence` 和必要的结合律规则；用左右侧相同优先级的案例验证括号，避免只验证“能输出”。
3. 若节点包含列、表或参数标记，应调用专用 Visitor 回调；子节点顺序应与 SQL 语法顺序一致，并保持任何失败立即短路。
4. 若增加还原状态，使用保存旧值—设置临时值—执行—恢复旧值的模式，确保错误路径也恢复；不要泄漏状态到兄弟节点。
5. 同步独立 Rust 测试 `pkg/parser/ast/expressions_test.rs` 或聚焦测试 `pkg/parser/ast/expressions_4_aster_unit_test.rs`，并核对 `pkg/parser/ast/expressions_test.go` 的原始意图。Rust 单元测试不应写入本生产文件。
6. 涉及参数标记时还要检查 `pkg/types/parser_driver/value_expr.rs` 与 `pkg/expression/util.rs`；涉及 crate 暴露方式时检查 `pkg/parser/ast/lib.rs`，但通常无需改 Cargo 依赖。

兼容风险集中在还原文本的精确格式、括号语义、Visitor 覆盖完整性和 Go 等价规则；性能风险主要来自深递归、字符串反复增长以及 `CiString`/AST 深克隆。若要把子查询从字符串升级为真实 AST，属于跨文件接口变更，不能只改本文件。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11467 文件、307296 节点、1848419 边；`files --filter pkg/parser/ast` 确认目标及独立测试均已索引；`node --file pkg/parser/ast/expressions.rs --offset 1/421/841` 完整读取 1246 行源码，并报告本文件被 52 个文件使用；`query expression_deep_equal` 定位到 `expressions.rs:1239`。对 `try_to_sql_with_flags` 的 method 查询无结果，故未以缺失的逐方法调用边作结论。
- crate/模块：`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/lib.rs:4963-5039`。
- Rust 直接接线：`pkg/types/parser_driver/value_expr.rs:448-497`、`pkg/expression/util.rs:1271-1300`。
- Rust 测试：`pkg/parser/ast/expressions_test.rs`、`pkg/parser/ast/expressions_4_aster_unit_test.rs`；补充的旧/新 AST 桥接覆盖见 `pkg/parser/ast/go_merge_25_test.rs:396-426`。
- Go 对照：`pkg/parser/ast/expressions.go`、`pkg/parser/ast/expressions_test.go`；深相等实现核对 `expressions.go:1687-1735`。
- 人工复核结论：本文件存在是为了在 Rust parser AST crate 中集中表达、还原和遍历表达式；运行时由 `Expr::restore`/`accept` 两条递归主链驱动；安全扩展必须同时维护枚举分派、运算优先级、Visitor 子树覆盖和独立对照测试，并显式记录尚未与 Go 等价的部分。

