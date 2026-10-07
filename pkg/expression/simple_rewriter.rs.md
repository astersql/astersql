# `pkg/expression/simple_rewriter.rs`

源文件：[`simple_rewriter.rs`](./simple_rewriter.rs)

## 文件定位

本文件属于 `astersql-expression` crate，是 SQL AST 与内部 `Expression` 之间的轻量适配层，同时提供字段名解析工具。`pkg/expression/lib.rs` 以 `expression_simple_rewriter` 私有模块装入该文件，并通过 `pub use expression_simple_rewriter::*` 将四个函数作为 crate 公共 API 再导出。crate 边界由 `pkg/expression/Cargo.toml` 的 `[package] name = "astersql-expression"` 与 `[lib] path = "lib.rs"` 确认；该清单还用 `package.metadata.porting.go-package = "pkg/expression"` 标明 Go 对照包。

它不实现完整表达式重写器：`ParseSimpleExpr` 只负责把一段表达式文本变成 `SELECT` 的首个投影 AST，再将该节点交给 `BuildSimpleExpr`；真正从 AST 构造内部表达式的逻辑由安装在 `pkg/expression/expression.rs` 中的 `BUILD_SIMPLE_EXPR_FACTORY` 完成。`FindFieldName` 和 `FindFieldNameIdxByColName` 则是规划、DDL、表元数据等调用方可复用的名称查找函数。

## 核心职责

1. `ParseSimpleExprWithTableInfo` 保留面向旧外部仓库的兼容入口，把单个 `TableInfo` 转成 `WithTableInfo("", table_info)` 构建选项后转交 `ParseSimpleExpr`。
2. `ParseSimpleExpr` 拒绝空输入，把表达式包装为 `select <expression>`，优先使用 `BuildContext::ParseSQL` 提供的会话解析器，否则创建默认 `parser::New()`；解析告警追加到当前 `EvalContext`，首条语句的首个投影表达式交给 `BuildSimpleExpr`。
3. `FindFieldName` 在 `types::NameSlice` 中按列名及可选的库名、表名解析唯一位置，跳过空槽位和 `NotExplicitUsable` 项，并处理 `Redundant` 字段与真正歧义。
4. `FindFieldNameIdxByColName` 为只需要列名的简单场景返回首个匹配下标；未找到以 `None` 表示。

这些函数只做入口规范化、选择和委派，不负责执行表达式、维护 schema，或决定具体标量函数语义。

## 主要符号

- `ParseSimpleExprWithTableInfo(ctx, expression_text, table_info) -> Result<Box<dyn Expression>, Error>`：兼容门面。它不复制构建逻辑，唯一的语义增量是生成 `WithTableInfo` 选项。
- `ParseSimpleExpr<'a>(ctx, expression_text, options) -> Result<Box<dyn Expression>, Error>`：文本解析主入口。`options: Vec<BuildOption<'a>>` 的生命周期允许构建选项借用表等外部元数据；成功值是动态分派的表达式对象。
- `FindFieldName(names, ast_column) -> Result<Option<usize>, Error>`：限定名查找。`Some(i)` 表示唯一选中的槽位，`None` 表示无匹配，`Err` 表示存在两个非冗余候选。
- `FindFieldNameIdxByColName(names, column_name) -> Option<usize>`：不检查库、表、可用性或冗余标志，只按 `ColName.L` 返回首项。
- 间接关键符号 `BuildSimpleExpr`（`pkg/expression/expression.rs`）：从 `BUILD_SIMPLE_EXPR_FACTORY` 取得已安装的构建函数；未安装时返回 `BuildSimpleExpr factory is not installed`。因此本文件的解析成功不等于表达式构建一定成功。

本文件没有模块级可变状态、常量、类型、trait、条件编译项或私有辅助函数；四个定义均为公开函数。

## 执行流程

`ParseSimpleExpr` 的正常路径如下：

1. 若 `expression_text` 为空，触发 `intest::Assert(false, &[])` 的开发期断言语义，并立即返回清晰错误，避免访问空 AST。
2. 用 `format!("select {}", expression_text)` 生成可由 SQL parser 接受的最小查询。
3. 调用 `ctx.ParseSQL(&sql)`；返回 `Some` 时沿用上下文解析器及其 SQL 模式/扩展语法，返回 `None` 时使用默认 parser。
4. 解析器错误通过 `?` 原样进入本函数的错误通道。若解析成功，逐个调用 `ctx.GetEvalCtx().AppendWarning(warning)` 保存语法告警；告警本身不阻断构建。
5. 从第一条语句向下检查 `ast::SelectStmt -> Fields -> first field -> Expr`。形状不符分别返回“未得到 SELECT”或“字段列表为空”的错误。
6. 调用 `BuildSimpleExpr(ctx, expression, options)`，把构建结果或错误直接返回。

`FindFieldName` 对 `NameSlice` 做一次顺序扫描。候选必须非空、可显式引用、列名相等；AST 中非空的 schema/table 还必须与候选匹配。首个候选先记录；后续若任一候选为 `Redundant`，保留非冗余者（两个都冗余则保留较早项）；若已有项与新项都非冗余，则立即构造包含限定列名的 ambiguous 错误。扫描结束返回记录的位置或 `None`。其时间复杂度为 O(n)，除歧义错误路径拼接名称外不分配与候选数成比例的额外状态。

## 数据与状态

- 解析输入是借用的 UTF-8 `&str`，临时生成的 `sql: String` 仅活到调用结束。
- parser 返回的 `statements` 和 `warnings` 归当前调用所有；只读取第一条语句和第一个投影字段，额外语句或字段不会参与构建。
- 唯一可观察副作用是把 parser 告警追加到 `ctx.GetEvalCtx()`。表达式构建器也可能依据 `BuildContext` 和 `BuildOption` 读取上下文或元数据，但其状态约束属于 `BuildSimpleExpr` 的实现边界。
- 名称比较使用 `ast::CIStr` 的小写字段 `.L`，所以依赖上游已经建立的规范化值，而不是在此处重新进行大小写折叠。
- `FindFieldName` 的局部 `found: Option<usize>` 是扫描状态；它不修改 `NameSlice`。Rust 的 `NameSlice` 可含 `None`，本实现将其跳过，这是 Go 指针切片在正常调用中没有显式表达的安全边界。

## 依赖与调用关系

直接下游关系：

- `ParseSimpleExprWithTableInfo -> WithTableInfo -> ParseSimpleExpr`。
- `ParseSimpleExpr -> BuildContext::ParseSQL` 或 `parser::New().ParseSQL`，随后调用 `EvalContext::AppendWarning` 与 `BuildSimpleExpr`。
- `BuildSimpleExpr -> BUILD_SIMPLE_EXPR_FACTORY`（`pkg/expression/expression.rs`）；该工厂把 AST 构建实现与本入口解耦。
- `FindFieldName` 依赖 `types::NameSlice`、`types::FieldName` 和 `ast::ColumnName` 的命名/可用性标志。

RustCodeGraph 显示 `ParseSimpleExpr` 的生产调用者跨越表与 DDL/规划路径，例如 `pkg/table/constraint.rs::buildConstraintExpression`、`pkg/table/column.rs::getColDefaultExprValue`、`pkg/table/tables/canonical_partition_expr.rs::build`、`pkg/ddl/storage_class.rs::{compare_range,is_unsigned,range_value}`、`pkg/ddl/split_region.rs::policy_bounds` 与 planner 的 `partial_index_always_meets_constraints`。兼容入口还由 `pkg/table/tables/tables.rs::table_from_meta_for_validation` 使用。

`FindFieldName` 的生产调用点包括 `pkg/planner/core/expression_rewriter.rs::toColumn` 和 `pkg/planner/core/planbuilder_runtime.rs::buildUpdate`，说明它处在 AST 列引用绑定到 schema 位置的路径上。`FindFieldNameIdxByColName` 的 Rust 直接证据主要来自本文件及独立测试；Go 对照另有 infoschema reader 调用，因此新增 Rust 调用时不能把它误当成完整限定名解析器。

## 错误处理与边界

- 空表达式在解析前返回 `expression should not be an empty string`；这既避免非法 SQL，也避免随后对 AST 首项取值。
- 上下文 parser 和默认 parser 的错误均通过 `?` 传播。上下文 parser 优先级由 `simple_rewriter_test.rs::parse_simple_expr_prefers_context_parser_like_go` 验证。
- Rust 会验证第一条节点确为 `SelectStmt` 且首个字段含表达式，分别返回显式错误；Go 对照直接做类型断言和下标访问，因此 Rust 在异常 parser 合约下更安全，但正常语义不变。
- parser 告警是非致命信息，解析成功后全部追加到评估上下文。
- `BuildSimpleExpr` 的工厂未安装、构建选项无效或 AST 不能构造成内部表达式时，错误由下游直接返回。
- 名称未找到不是错误，而是 `Ok(None)`；只有两个有效的非冗余同名候选才返回 `Column '<qualified-name>' in field list is ambiguous`。AST 未提供 schema/table 时，错误名只包含实际提供的部分。
- `FindFieldNameIdxByColName` 有意忽略限定符、`NotExplicitUsable` 和 `Redundant`，且多个匹配时静默返回首项；需要歧义检测的调用方必须使用 `FindFieldName`。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。四个函数均只有调用栈局部状态；名称查找是只读的，可由多个线程并发调用，前提是传入类型本身满足调用环境的共享约束。

`ParseSimpleExpr` 对共享状态的唯一写入是 `EvalContext::AppendWarning`，其同步与生命周期由具体 `BuildContext`/`EvalContext` 实现负责。本函数不会缓存解析器或表达式，也不会延长 `table_info`、AST 或构建选项借用超过返回过程。返回的 `Box<dyn Expression>` 拥有表达式对象；临时 SQL、AST 容器和告警向量在函数结束时释放。

## 与 Go 版本的对应关系

对照文件为 `pkg/expression/simple_rewriter.go`，测试为 `pkg/expression/simple_rewriter_test.go`。四个公开函数逐一对应：

- Go `ParseSimpleExprWithTableInfo` 同样是外部兼容入口并标记 deprecated；Rust 注释建议新调用直接使用 `ParseSimpleExpr` 与 `WithTableInfo`。
- 两版 `ParseSimpleExpr` 都拒绝空串、包装成 `select ...`、优先采用上下文 SQL parser、追加 warnings，最后调用 `BuildSimpleExpr`。Rust 用 `Option<Result<...>>` 表达上下文是否提供 parser，并对 SELECT/字段 AST 形状增加显式检查。
- 两版 `FindFieldName` 都使用 `.L` 比较名称，空 schema/table 充当通配限定，跳过不可显式引用列，优先非冗余候选，并在两个非冗余候选冲突时报告 field-list 歧义。Rust 还跳过 `NameSlice` 中的 `None`；返回的 `Option<usize>` 对应 Go 的索引与 `-1` 哨兵。
- `FindFieldNameIdxByColName` 的 Rust `Some(index)/None` 对应 Go 的 `index/-1`，均返回首个同列名项。

Go 测试把优化后的 `FindFieldName` 与保留的原实现逐例比较，并含 10 至 10000 项的基准；Rust 独立测试复刻同一组语义案例，另外覆盖 `None` 槽、`NotExplicitUsable`、首匹配、空输入和上下文 parser 优先级。Rust 测试没有复刻 Go benchmark，因此本文不宣称两版具有实测等价性能。

## 扩展指南

- 新增解析前处理或 parser 选择策略时，修改 `ParseSimpleExpr`，并在独立的 `pkg/expression/simple_rewriter_test.rs` 增加成功、错误和 warning 顺序测试；不要把测试嵌入生产源文件。
- 新增表/schema 构建信息时，优先增加或组合 `BuildOption`，让 `ParseSimpleExpr` 继续只负责解析与委派；兼容入口应保持为薄封装，避免两条构建路径漂移。
- 改变字段匹配规则时，以 `FindFieldName` 为唯一的限定名/歧义规则入口，并同步 Rust 与 Go 的表驱动案例。尤其要保护三项不变量：不可显式引用列不参与匹配、非冗余列优先于冗余列、两个非冗余候选必须报错。
- 仅需首个裸列名位置时才扩展 `FindFieldNameIdxByColName`；若需要库表限定、可用性或歧义语义，应改用前者而非不断给这个简化函数叠加规则。
- 兼容风险集中在 parser 上下文优先级、warning 的写入时机、错误文本和歧义选择；性能风险主要是名称查找的线性扫描与错误路径分配。若要建立索引加速，必须证明候选顺序和 redundant 优先级仍与 Go 一致。
- 表达式构建失败应在 `BuildSimpleExpr`/工厂及其独立测试中处理，本文件不应复制构建器逻辑。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11467 个文件，其中 Rust 7032 个；目标文件可完整读取。
- RustCodeGraph `node --file pkg/expression/simple_rewriter.rs`：核对 124 行源文件及四个公开函数的完整实现。
- RustCodeGraph `callers`/`callees` 与精确 `explore`：确认 `ParseSimpleExpr -> BuildSimpleExpr`、`ParseSimpleExprWithTableInfo -> ParseSimpleExpr`，以及表、DDL、planner 和测试调用点；确认 `FindFieldName` 的 planner 调用点。
- RustCodeGraph `node BuildSimpleExpr`：确认 Rust 构建函数从 `BUILD_SIMPLE_EXPR_FACTORY` 取工厂，未安装时返回错误。
- `pkg/expression/lib.rs`：确认私有模块挂载、公共再导出和独立 `#[cfg(test)]` 测试模块。
- `pkg/expression/Cargo.toml`：确认 crate 名、库入口、parser/model/types/intest 等本地依赖，以及 Go 包迁移元数据。
- `pkg/expression/simple_rewriter.go` 与 `pkg/expression/simple_rewriter_test.go`：核对 Go 主流程、错误/冗余列语义、表驱动案例和 benchmark。
- `pkg/expression/simple_rewriter_test.rs`：核对 Rust 的 Go 等价案例，以及 Rust 特有的空槽、不可用列、空输入与自定义 parser 边界。
- 人工复核结果：文档区分了解析适配与实际表达式构建，列明了正常路径、错误边界、状态副作用、直接调用关系和安全扩展位置；没有把未运行的 Cargo 测试或未测性能写成已验证事实。
