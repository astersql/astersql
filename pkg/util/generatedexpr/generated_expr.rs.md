# [`pkg/util/generatedexpr/generated_expr.rs`](./generated_expr.rs)

## 文件定位

本文件是 `astersql-util-generatedexpr` crate 的业务实现，crate 入口 `pkg/util/generatedexpr/lib.rs` 通过 `pub mod generated_expr` 和 `pub use generated_expr::*` 导出这里的 API。它位于“持久化的 SQL 表达式字符串”与 parser AST 之间：将元数据中的生成列、表达式默认值或分区表达式文本还原为 `ast::ExprNode`，并可按 `model::TableInfo` 校验 AST 中的列引用。

`pkg/util/generatedexpr/Cargo.toml` 表明该 crate 直接依赖 `astersql-parser`、`astersql-meta-model` 和 `astersql-util-parser`；本文件分别通过 crate 根的 `ast`/`charset`/`parser`、`model` 和 `parserutil` 桥接这些依赖。当前 Rust 生产使用方包括 `pkg/table/tables/tables.rs`、`pkg/table/tables/canonical_partition_expr.rs` 和 `pkg/planner/core/planbuilder_runtime.rs`。

## 核心职责

1. `ParseExpression`：按默认 SQL mode 解析一段不带 `SELECT` 的表达式文本。
2. `ParseExpressionWithSQLMode`：按表达式写入元数据时的指定 SQL mode 解析，避免 `||`、反斜杠等语义随当前会话模式漂移。
3. `SimpleResolveName`：遍历表达式 AST，确认每个列名都能在给定 `TableInfo.Columns` 中按 `CIStr.L`（规范化小写名）找到；成功时原样返回 AST，不做绑定、类型推断或节点改写。
4. `syntax_error`：将普通 parser 诊断转换成 TiDB/MySQL 1064 风格的 `ErrParse`，同时保留已经具有 parser `TerrorError` 根因的错误。

它不负责把 AST 构造成可执行表达式。例如 `planbuilder_runtime.rs` 在解析、验名后继续调用 `expression::BuildSimpleExpr`；因此这里是语法和简单名称校验层，不是表达式执行层。

## 主要符号

- `SYNTAX_ERROR_PREFIX: &str`：TiDB SQL 语法错误的固定手册提示前缀，由 `syntax_error` 生成 `ErrParse` 时使用。
- `NameResolver<'a> { table_info: &'a model::TableInfo }`：只读借用表元数据的内部解析器，不对表或 AST 持久化任何状态。
- `NameResolver::resolve_column`：以 `candidate.Name.L == column.Name.L` 判断列存在性；失败返回 `can't find column <原始列名> in <原始表名>`。
- `resolve_exprs`、`resolve_items`、`resolve_window`、`resolve_node`：分别处理表达式切片、`ByItem`、窗口规格和子查询中的 `SelectStmt`，为递归遍历提供分层辅助函数。
- `NameResolver::resolve_expr`：按 `ast::ExprKind` 穷举递归；列节点校验名称，复合节点递归到全部相关子表达式，不含列引用的叶子节点直接成功。
- `syntax_error(errors::Error) -> errors::Error`：识别已有 `parser::TerrorError`，否则用 `parser::ErrParse.GenWithStackByArgs` 包装错误。
- `ParseExpression(&str)` 与 `ParseExpressionWithSQLMode(&str, mysql::SQLMode)`：公开入口，分别向内部 `parse_expression` 传入 `None` 或 `Some(sql_mode)`。
- `parse_expression`：唯一的解析实现，负责包装 SQL、配置 parser、释放 parser、检查结果并提取首个投影表达式。
- `SimpleResolveName(ast::ExprNode, &TableInfo)`：公开验名入口；取得 AST 所有权，校验成功后把同一个节点返回给调用方。

## 执行流程

解析路径如下：

1. 公开入口调用 `parse_expression`，把输入拼成 `select {expression}`，使普通 TiDB SQL parser 能解析孤立表达式。
2. `charset::GetDefaultCharsetAndCollate` 提供连接字符集和排序规则，随后包装为 `parser::CharsetConnection` 与 `parser::CollationConnection`。
3. `parserutil::GetParser` 取得 parser；若调用者提供 SQL mode，则先执行 `SetSQLMode`。
4. `ParseSQL` 解析包装后的 SQL；无论成功失败，紧接着都调用 `parserutil::DestroyParser` 归还/销毁 parser。
5. 解析错误经 `map_err(syntax_error)` 传播。成功路径取第一条 statement，将其向下转换为 `ast::SelectStmt`，再取第一个字段的 `Expr`。

名称校验路径从 `SimpleResolveName` 进入 `NameResolver::resolve_expr`。递归会覆盖函数参数、聚合参数及排序项、二元/一元表达式、列表与范围、LIKE/REGEXP、CASE、窗口函数及窗口 frame、子查询 SELECT 的投影/WHERE/GROUP BY/HAVING/ORDER BY/LIMIT/列表/窗口和其子节点。任何分支首次返回错误时，`?`/`try_for_each` 立即停止遍历。

生产链路的三个直接例子是：

- `table_from_meta_for_validation`（`pkg/table/tables/tables.rs`）解析生成列并验名后存入 `generated_expressions`；表达式默认值只解析、不验名，存入 `default_expressions`。
- `build_insert` 附近的生成列逻辑（`pkg/planner/core/planbuilder_runtime.rs`）解析并验名，再交给 `BuildSimpleExpr` 构造 INSERT 所需的可执行表达式。
- `canonical_partition_expr.rs` 在 HASH 分区路径解析原始表达式并保存到 `PartitionExpr.OrigExpr`。

## 数据与状态

本文件没有全局可变状态。`SYNTAX_ERROR_PREFIX` 是只读常量；`NameResolver` 仅在一次调用期间借用 `TableInfo`。`SimpleResolveName` 消费并返回 `ExprNode`，其不变量是：成功返回值与输入节点相等，且遍历范围内的每个 `ExprKind::Column` 都在 `TableInfo.Columns` 中存在。

列匹配只比较 `CIStr.L`，所以大小写不敏感，同时错误文案保留 `ColumnName.Name.O` 和 `TableInfo.Name.O` 的原始拼写。它不使用列 offset、ID、schema/table 限定名或类型信息，也不向 AST 附加已解析的 `ColumnInfo`。

解析结果依赖 parser AST 定义以及指定 SQL mode。`gen_expr_test.rs` 证明 `1 || 2` 在 `ModePipesAsConcat` 下成为 `concat` 函数，在默认 mode 下成为逻辑 OR；`ModeNoBackslashEscapes` 也会改变字符串字面量内容。

## 依赖与调用关系

下游依赖：

- `parserutil::{GetParser, DestroyParser}` 管理 parser 实例。
- `parser::{CharsetConnection, CollationConnection, ErrParse, TerrorError}` 提供解析选项和错误分类。
- `charset::GetDefaultCharsetAndCollate` 提供默认连接配置。
- `ast::{ExprNode, ExprKind, SelectStmt, WindowSpec, ByItem, Node}` 提供解析结果和递归结构。
- `model::TableInfo` 提供表名与列清单；`errors` 提供构造、cause/stack 检查和统一错误类型。

上游调用：RustCodeGraph 对本文件给出的直接使用文件为 `planbuilder_runtime.rs`、`canonical_partition_expr.rs`、`tables.rs` 和 `gen_expr_test.rs`。补充的精确引用搜索还确认 `migration_aster_unit_test.rs` 直接覆盖公开 API。当前仓库搜索未发现 Rust 生产代码调用 `ParseExpressionWithSQLMode`，该入口有 Rust/Go 对齐测试，并与 Go API 保持一致。

crate 边界由 `lib.rs` 再导出公开函数，因此调用方使用 `generatedexpr::ParseExpression`/`SimpleResolveName`，不需要依赖内部 `NameResolver` 或 `parse_expression`。

## 错误处理与边界

- parser 返回错误后，`syntax_error` 若识别到已有带栈且根因为 `parser::TerrorError` 的错误，就原样返回；否则包装为带固定前缀的 `parser::ErrParse`。`migration_aster_unit_test.rs` 验证残缺输入 `1 +` 同时具有 TiDB 语法错误提示和 `mysql::ErrParse` 分类。
- 未知列是可恢复的 `errors::Error`，精确文案由 `resolve_column` 生成；测试覆盖 `can't find column missing in widgets`。
- 遍历采用首次错误即返回的策略，不汇总多个未知列。
- `parse_expression` 在成功解析后对“至少一条 statement”“statement 必为 SELECT”“至少一个字段且字段含表达式”使用 `expect`。这些条件由内部固定的 `select <expr>` 包装与 parser 成功契约保证；若 parser 改变成功结果形态，会触发 panic 而不是返回错误。
- 空输入、只含语法残片的输入应由 parser 错误路径处理。本文件不做长度限制、SQL 注入过滤或表达式合法性策略校验；输入被当作 SQL 语法片段解析。
- `resolve_node` 只对可向下转换为 `SelectStmt` 的节点展开 SELECT 相关字段；当前实现以显式 `ExprKind` 枚举为覆盖边界。新增带子表达式的 AST 变体时必须同步递归逻辑，否则可能漏验列引用。

## 并发与资源生命周期

公开函数没有共享可变状态，`NameResolver` 只持有不可变借用；本文件本身不创建线程、任务、锁、通道或事务。能否跨线程调用取决于传入/返回 AST、元数据和 parser 基础设施的 trait/实现，本文件没有额外声明并发保证。

唯一显式资源生命周期是 parser：`GetParser` 后执行解析，随即在检查 `Result` 之前调用 `DestroyParser`，因此正常的成功和错误返回路径都会释放/归还 parser。需要注意，若 `ParseSQL` 自身 panic，当前代码没有 RAII guard，`DestroyParser` 不会执行；这与普通 `Result` 错误路径不同。

递归遍历深度与 AST 嵌套深度一致，列查找对每个列引用线性扫描 `TableInfo.Columns`。本文件没有缓存或索引；扩展时若为性能引入名称映射，需要保持 `CIStr.L` 语义和错误中的原始拼写。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/generatedexpr/generated_expr.go`：

- Rust 的三个公开函数与 Go 的 `ParseExpression`、`ParseExpressionWithSQLMode`、`SimpleResolveName` 一一对应；两端都用 `select <expr>` 包装、默认 charset/collation、可选 SQL mode，并提取首个 SELECT 字段表达式。
- Go 用 `defer DestroyParser(parse)`；Rust 在 `ParseSQL` 后、传播 `Result` 前显式调用 `DestroyParser`。普通成功/错误路径语义相同，但 Rust 路径不具备 defer 对 panic 的清理保证。
- Go 的 `nameResolver` 实现 `ast.InPlaceVisitor`，由 `ast.Walk` 统一遍历；Rust 因当前 AST 接口而在 `resolve_expr`/`resolve_node` 中显式递归。Rust 必须随新增 AST 复合变体维护遍历覆盖，这是两端最重要的维护差异。
- Go resolver 在结构体中保存首个错误并让 walk 返回 false；Rust 直接用 `Result` 和 `?` 短路。两端均按 `CIStr.L` 验名，错误文案相同，成功时返回原节点。
- Go 调用 `util.SyntaxError(err)`；Rust 的本地 `syntax_error` 复现其核心规则：保留 parser terror，否则生成 `ErrParse`。迁移回归测试验证了错误前缀和错误码。
- `gen_expr_test.go` 与 `gen_expr_test.rs` 均覆盖 `json_extract` 以及 SQL mode 对 `||` 和反斜杠的影响；Rust 另有 `migration_aster_unit_test.rs` 覆盖验名和语法错误分类。

## 扩展指南

- 新增公开解析入口时，应复用 `parse_expression`，避免 charset/collation、SQL mode、parser 回收和错误包装出现分叉；同时在 `lib.rs` 的公开边界和独立测试文件中确认导出与覆盖。
- parser AST 新增或调整 `ExprKind` 时，应审查 `resolve_expr` 的穷举分支；若变体包含表达式、窗口、排序项或查询节点，必须递归到每个可能包含列引用的子节点。相应测试放在独立的 `migration_aster_unit_test.rs` 或 `gen_expr_test.rs`，不要嵌入生产文件。
- 扩展子查询支持时，应先核对 `ast::Node` 的实际节点种类，再调整 `resolve_node`；尤其要用嵌套 SELECT 的投影、过滤、分组、排序、LIMIT/frame 等用例证明没有漏验。
- 修改列匹配策略时需同步 Go 语义，重点保持 `CIStr.L` 大小写规则、错误文案和“只校验、不改写 AST”的契约。若改用哈希索引，应评估构建开销与宽表/多列引用场景的收益。
- 修改错误包装时需验证错误字符串、`TerrorError` 根因和 MySQL 1064/`ErrParse` 分类，避免调用方只得到无分类字符串。
- 修改 parser 获取/释放顺序时应优先采用能覆盖提前返回和 panic 的资源守卫，但不得在没有核对 `parserutil` 池化契约与 Go 行为前改变生命周期。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标目录的 Rust/Go 实现与测试均已索引。
- RustCodeGraph `node --file pkg/util/generatedexpr/generated_expr.rs`：读取完整 272 行，并得到直接使用文件 `pkg/planner/core/planbuilder_runtime.rs`、`pkg/table/tables/canonical_partition_expr.rs`、`pkg/table/tables/tables.rs`、`pkg/util/generatedexpr/gen_expr_test.rs`。
- RustCodeGraph `query`：确认 Rust/Go 两侧 `ParseExpression`、`ParseExpressionWithSQLMode` 和 `SimpleResolveName` 的定义与签名。`callers`/`callees` 即使加 `--file` 仍在 30 秒内无输出，因此没有据此推断调用边，而是读取上述直接使用文件并用精确引用搜索逐项确认。
- 已读取实现与边界：`pkg/util/generatedexpr/generated_expr.rs`、`pkg/util/generatedexpr/lib.rs`、`pkg/util/generatedexpr/Cargo.toml`、`pkg/util/generatedexpr/generated_expr.go`。
- 已读取生产调用证据：`pkg/table/tables/tables.rs` 的 `table_from_meta_for_validation`，`pkg/table/tables/canonical_partition_expr.rs` 的 HASH 分区路径，`pkg/planner/core/planbuilder_runtime.rs` 的 INSERT 生成列构建路径。
- 已读取测试：`pkg/util/generatedexpr/gen_expr_test.rs`、`migration_aster_unit_test.rs`、`main_test.rs`，以及 Go 对照 `gen_expr_test.go`；目录中不存在 `doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构验证，并人工检查唯一生产物、事实范围和 `plan.md` 未修改。
