# `pkg/parser/ast/sql_restore.rs`

## 文件定位

本文件是 `astersql-parser-ast` crate 的结构化 SQL 还原适配层。模块由 [`lib.rs`](lib.rs) 的 `pub mod sql_restore` 公开，crate 边界和依赖声明位于 [`Cargo.toml`](Cargo.toml)：它只使用 AST crate 自身及已经进入该 crate 的 `parser-mysql`、`parser-types` 等类型，没有单独的 feature 开关。

它解决的是“手工构造或改写后的规范 AST 可能没有原始 SQL 文本，但调用方仍需得到 SQL”这一问题。公开入口 `restore_expr`、`restore_node` 和 `restore_select_stmt` 先对已支持结构进行递归序列化；对于多数尚未结构化覆盖的 `Node`，`restore_node` 回退到 `Node::Text()`。因此它是 Rust 迁移期间的规范 AST 恢复桥，而不是 Go `format.RestoreCtx` 全部能力的通用替代品。

直接生产使用包括：`dumpling/export/schema_projection_restore.rs` 在替换表达式字面量和生成建表查询时恢复 SQL；`pkg/session/runtime/mview_ddl.rs` 恢复物化视图日志调度表达式；`pkg/ddl/persistent_modify_column.rs` 在列重命名后重新生成 masking policy 表达式；`pkg/parser/ast/stats.rs` 恢复统计选项值；[`materialized.rs`](materialized.rs) 将两个公开入口复用为物化视图语句的内部恢复器。

## 核心职责

- `restore_expr` 对 `ExprKind` 做穷举匹配，恢复字面量、列名、运算、谓词、函数、窗口、子查询、类型转换和全文检索等表达式。递归子节点的错误通过 `?` 向上传播。
- `restore_node` 是动态 `Node` 分派入口：为少数规范语句提供结构化路径，为物化视图、`AnalyzeTableStmt` 等类型委托其专用 `restore()`，为普通节点使用已保存的 `Text()`。
- `restore_select_stmt` 和 `restore_setopr_stmt` 在没有原始文本时重建受支持的 `SELECT`、CTE、连接树和集合运算。
- 私有辅助函数统一处理标识符引用、值文本、时间单位、窗口 frame、索引提示、表名、授权层级、用户、查询字段和 `LIMIT`。
- 文件显式保护当前结构化实现无法无损表达的情况：复杂 SELECT 优先退回原文；既无结构化支持又无原文的节点返回错误，避免生成看似合法但语义被删减的 SQL。

## 主要符号

- `pub fn restore_expr(&ExprNode) -> Result<String, String>`：表达式总入口。主要分支与 `ExprKind` 一一对应；`Subquery` 经 `NodeRef::with_node(restore_node)` 跨回语句恢复，`Function` 对时间区间函数委托 `functions::FuncCallExpr::keyword(...).restore()`，`Cast`/`JSONSumCrc32` 委托 `FieldType::FormatAsCastType` 生成目标类型。
- `pub fn restore_node(&dyn Node) -> Result<String, String>`：按运行时类型下转。它直接处理 `SetRoleStmt`、特定 `GRANT`/`REVOKE OPERATE VIEW`、`SHOW STORAGE_CLASS TRANSITIONS`、`SelectStmt` 和 `SetOprStmt`；物化视图语句及 `AnalyzeTableStmt` 走各自 `restore()`；其他类型使用 `Node::Text()`。
- `pub fn restore_select_stmt(&SelectStmt) -> Result<String, String>`：按 `WITH`、括号、`SELECT` 选项、字段、`FROM`、`WHERE`、`GROUP BY`、`HAVING`、`WINDOW`、`ORDER BY`、`LIMIT` 的顺序输出。它先检测不支持的 kind、hint、priority、rollup、锁、VALUES 列表和 `INTO` 等特征。
- `restore_setopr_stmt`：恢复集合运算列表；首项不写操作符，后续项从 `operators[index]` 选择 `UNION [ALL]`、`EXCEPT [ALL]` 或 `INTERSECT [ALL]`，缺失操作符按 `UNION` 处理，最后追加语句级排序和限制。
- `restore_join`、`restore_table_source`、`restore_table_name[_core]`、`restore_index_hint[s]`：恢复表源和连接树，并为嵌套连接加括号。`TableSource::QuerySource` 必须持有可访问节点；普通表源输出 schema、table、partition、alias 和 index hint。
- `restore_window_spec`、`restore_frame_clause`、`restore_frame_bound`、`restore_by_item`：恢复窗口名/引用、分区、排序以及 `ROWS`/`RANGE`/`GROUPS BETWEEN ... AND ...`。
- `quote`、`restore_column_name`、`restore_value_text`、`time_unit_keyword`：最底层格式规则。标识符复用 `dml::quote_name`；字符串以 `_UTF8MB4'...'` 输出并把单引号加倍；`NULL`、参数标记和可解析为 `f64` 的文本保持裸值。

本文件没有模块级常量、类型定义、trait、`impl` 或条件编译项；共有三个公开函数，其余二十一个函数均为模块私有实现细节。

## 执行流程

1. 调用方按输入粒度选择 `restore_expr` 或 `restore_node`。物化视图代码在专用 `restore()` 中分别把它们别名为 `expression` 和 `restore_select`。
2. 表达式入口匹配 `ExprKind`。叶子节点直接格式化；组合节点递归恢复子表达式，再按固定关键字和分隔符拼接。列表分支用 `collect::<Result<Vec<_>, _>>()?` 保证任一子项失败即终止。
3. 子查询通过 `NodeRef::with_node` 借用内部 `dyn Node`，再调用 `restore_node`；缺失节点时生成明确错误。CAST 类表达式先写入字节缓冲区，再验证 UTF-8。
4. 节点入口先匹配必须从结构恢复的特殊语句或已经有专用恢复器的语句。`SelectStmt` 字段为空但保存了文本时直接返回文本，否则进入 `restore_select_stmt`；集合运算进入 `restore_setopr_stmt`。
5. `restore_select_stmt` 若发现结构化子集之外的特征，就只在存在 `stmt.Text()` 时回退；否则报错。受支持路径按 SQL 子句顺序递归恢复，连接树由 `restore_join` 深度优先处理。
6. `restore_node` 未识别的节点最后读取 `Node::Text()`；非空则保留解析器记录的原 SQL，空文本则拒绝恢复。

例如 [`materialized_test.rs`](materialized_test.rs) 证明：无原文 AST 可恢复 `SELECT SQL_NO_CACHE 1`、带字段/表/过滤/分组/排序/限制的 SELECT、CTE、`UNION ALL`、子查询和 `BINARY` CAST；这正是结构化路径存在的主要理由。

## 数据与状态

所有公开入口只借用 AST 并返回新建的 `String`，不修改节点。中间状态是函数栈上的 `String`、`Vec<String>`、分隔符和临时字节缓冲区；没有全局或线程局部状态。

影响输出的状态完全来自 AST 字段：

- `CIStr.O` 用作标识符原始拼写；schema/table/name 的空值决定限定名层级。
- `ExprKind` 携带表达式树、否定标志、escape、distinct、全文检索位标志以及类型信息。
- `SelectStmt` 的 `WithBeforeBraces` 与 `IsInBraces` 决定 CTE 和括号相对位置；`SelectStmtOpts` 决定 `SQL_NO_CACHE` 与 `DISTINCT`。
- `Node::Text()` 是兼容回退所需的持久文本状态。结构化覆盖不足时，是否有该文本直接决定成功或报错。
- `NodeRef` 内部是 `Rc<RefCell<Option<Box<dyn Node>>>>`（见 [`lib.rs`](lib.rs) 的 `NodeRef`），本文件只通过 `with_node` 做短期不可变借用，不持有引用越过调用边界。

输出是规范化 SQL，而非原始空白的保真复制：关键字通常大写，列表分隔和空格由各恢复函数固定，名称加反引号，普通字符串带 `_UTF8MB4` introducer。

## 依赖与调用关系

上游直接调用边由仓库搜索确认：

- `dumpling/export/schema_projection_restore.rs -> restore_expr`：AST 字面量替换后重建表达式；同文件还以 `restore_node` 输出 `CREATE TABLE ... AS <query>` 的查询部分。
- `pkg/session/runtime/mview_ddl.rs -> restore_expr`：类型检查完成后恢复 `PURGE START WITH/NEXT` 调度表达式，并把错误加上子句上下文。
- `pkg/ddl/persistent_modify_column.rs -> restore_expr`：访问器重命名列引用后重新生成 masking policy 表达式。
- `pkg/parser/ast/stats.rs -> restore_expr`：`AnalyzeTableStmt::restore` 输出统计选项值。
- `pkg/parser/ast/materialized.rs -> restore_expr/restore_node`：物化视图 AST 的专用恢复实现复用表达式和 SELECT 恢复。

主要下游关系为：`quote -> dml::quote_name`；`restore_expr -> functions::FuncCallExpr::keyword/restore`、`FieldType::FormatAsCastType`、`NodeRef::with_node` 及自身递归；`restore_node -> materialized.rs` 中各语句的 `restore()`、`AnalyzeTableStmt::restore`、`restore_select_stmt`、`restore_setopr_stmt`；SELECT 恢复再调用 CTE、join、窗口、字段和 limit 辅助函数。

RustCodeGraph 将本文件识别为 27 个符号，并报告被 15 个文件使用；精确 `query` 能定位三个公开入口。其 `callers`/`callees` 命令在本次环境中超时且未返回边，因此上述具体调用边用 `rg` 和对应源码补齐，未把超时结果推断成“无调用者”。

## 错误处理与边界

错误统一为 `Result<String, String>`。重要失败边界如下：

- `MATCH ... AGAINST` 同时设置 BOOLEAN MODE 与 QUERY EXPANSION（位标志 `0x01` 和 `0x10`）时返回 `BOOLEAN MODE doesn't support QUERY EXPANSION`；Rust 与 Go 测试均覆盖该不变量。
- 空 `NodeRef` 子查询和空派生表查询分别报 `subquery is missing query`、`missing query source`；缺少 join 左侧时报 `join is missing Left`。
- `FormatAsCastType` 的错误原样字符串化，生成的类型字节不是 UTF-8 时也返回转换错误。
- 不支持的 SELECT 特征只有在保存了原始文本时才安全回退；无文本时返回 `restore_select_stmt: unsupported SELECT feature without SQL text`。
- 未识别节点且无原始文本时返回 `restore_node: unsupported node without SQL text`。

需要特别注意的兼容边界：Rust 结构化实现没有 Go `RestoreCtx` 的 flags、CTE 名字记录和完整优先级括号策略；例如 Go `BetweenExpr.Restore` 会根据 restore flag 和操作符优先级添加括号，而本文件直接拼接子表达式。因此扩展结构化覆盖时不能仅以“能生成可解析字符串”为标准，必须对照 Go 的关键字、空格、括号、字符集、hint 和错误语义。

`restore_value_text` 以 `parse::<f64>()` 判断裸数值，这是一项格式规则而非完整 SQL 字面量解析器；新增 datum 类型时应避免让非数值文本意外绕过 SQL 字符串转义。`time_unit_keyword(Invalid)` 返回空串，调用方必须像 `restore_frame_bound` 一样先判断是否有合法单位。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。函数的可重入性来自纯借用输入和局部输出缓冲区；不同线程能否共享具体 AST 仍由 AST 类型自身的 `Rc<RefCell<...>>` 约束，本文件没有宣称或增加 `Send`/`Sync`。

递归生命周期与 AST 深度一致：表达式、连接树、CTE/子查询和集合运算都递归调用恢复函数。临时 `NodeRef::with_node` 借用在闭包返回时释放；`WithClause` 的 `RefCell` 借用也只覆盖单次恢复调用。当前实现没有显式递归深度限制，因此不可信的极深 AST 可能带来栈占用风险，但没有证据表明本文件接受未经解析器约束的远程对象。

内存开销主要来自每层新建字符串、列表 `collect` 后再 `join` 以及递归拼接。对超长字段/表达式列表扩展时，应关注重复分配，但不能以减少分配为由改变 Go 兼容输出。

## 与 Go 版本的对应关系

Go 没有同名 `sql_restore.go`；对应逻辑分散在各 AST 类型的 `Restore(*format.RestoreCtx)`：表达式位于 `pkg/parser/ast/expressions.go` 和 `functions.go`，SELECT/CTE/join/window/set operation 位于 `dml.go`，角色及授权位于 `misc.go`，统计语句位于 `stats.go`，物化视图扩展在相应 AST 实现中。

已核对的对应关系包括：

- Rust `restore_expr` 的 BETWEEN、LIKE/ILIKE、MATCH AGAINST 分支对应 Go `BetweenExpr.Restore`、`PatternLikeOrIlikeExpr.Restore`、`MatchAgainst.Restore`；BOOLEAN 与 QUERY EXPANSION 互斥错误文本一致。
- Rust `restore_join`、`restore_with_clause`、`restore_select_stmt`、`restore_setopr_stmt` 分别对应 Go `Join.Restore`、`WithClause/CommonTableExpression.Restore`、`SelectStmt.Restore`、`SetOprStmt/SetOprSelectList.Restore` 的主要输出顺序。
- Rust `restore_node` 对 `SHOW STORAGE_CLASS TRANSITIONS` 的专门分支对应 Go `ShowStmt.Restore`；委托 `AnalyzeTableStmt::restore()` 和物化视图 `restore()`，避免在总入口复制这些类型的完整逻辑。
- 两端并非完全等价：Go 以每种节点的 `RestoreCtx` 为主，支持 flags、错误上下文和更完整的语法；Rust 仅对当前调用链需要的规范 AST 子集做结构化恢复，其余依赖原始 `Text()`。Go 的 join 恢复还含逗号连接特例，而当前 Rust `restore_join` 总是输出显式 `JOIN`；这类差异在扩大无文本恢复范围前必须先补兼容测试。

Rust 直接回归证据在 [`materialized_test.rs`](materialized_test.rs)、[`go_merge_13_test.rs`](go_merge_13_test.rs) 和 [`go_merge_16_test.rs`](go_merge_16_test.rs)；Go 的广泛恢复基线在 `expressions_test.go`、`functions_test.go`、`dml_test.go` 及 `util_test.go` 的 `runNodeRestoreTest*` 工具中。测试逻辑应继续保留在这些独立测试文件，不应嵌入本生产文件。

## 扩展指南

新增表达式种类时，优先在 `restore_expr` 增加明确的 `ExprKind` 分支，并复用现有名称、值、窗口或类型辅助函数；同时在独立 Rust 测试文件构造“无 `Text()`”节点，证明结构本身足以恢复。若 Go 节点依赖 restore flags 或运算符优先级，先移植对应语义，不能用简单字符串拼接降级。

新增语句支持时，按以下选择接入点：语句已有可靠的专用 `restore()`，就在 `restore_node` 增加下转委托；属于 SELECT 子句或集合运算，则扩展 `restore_select_stmt`/`restore_setopr_stmt` 及对应私有辅助函数；尚不能完整覆盖时保留 `Text()` 回退并让无文本输入明确失败。不要为了让测试通过而删除 `unsupported_options` 守卫。

兼容性检查至少覆盖：Go 同类 `Restore` 的关键字与间距、标识符和字符串转义、括号及优先级、CTE 作用域、index/table hints、错误文本，以及解析—恢复—再解析的 AST 等价性。性能变更应特别审查深树递归和大型列表的 `collect/join` 分配。

建议同步测试位置：通用表达式放在 `expressions_test.rs`/`functions_test.rs` 或相应 Aster 单元测试文件；SELECT、join、窗口和 set operation 放在 `dml_test.rs`；本模块当前无同名测试文件，直接规范 AST 的集成恢复回归集中在 `materialized_test.rs` 与 `go_merge_*_test.rs`。遵循仓库约束，生产逻辑和测试逻辑保持分文件。

## 验证依据

- RustCodeGraph：`status` 确认索引含目标文件；`files --filter pkg/parser/ast/sql_restore.rs` 确认文件及 27 个符号；`node --file ... --offset 1/500` 读取全部 1034 行；`query restore_node/restore_expr/restore_select_stmt --kind function` 定位公开入口。`callers`/`callees` 两次在 30 秒内无输出，故调用边改由源码搜索验证。
- Rust 源码：[`sql_restore.rs`](sql_restore.rs) 全文；[`lib.rs`](lib.rs) 的模块导出、`Node` 和 `NodeRef` 契约；[`materialized.rs`](materialized.rs)、`stats.rs` 以及 Dumpling、session、DDL 的直接调用点。
- crate 配置：[`Cargo.toml`](Cargo.toml) 确认包名 `astersql-parser-ast`、`lib.rs` 入口、无 feature 表及 parser 子 crate 依赖。
- Rust 独立测试：[`materialized_test.rs`](materialized_test.rs) 覆盖无源文本 SELECT、表达式、CTE、集合运算、子查询、CAST 与全文检索错误；[`go_merge_13_test.rs`](go_merge_13_test.rs) 覆盖 storage-class SHOW；[`go_merge_16_test.rs`](go_merge_16_test.rs) 覆盖 `AnalyzeTableStmt` 委托一致性。
- Go 对照：`pkg/parser/ast/expressions.go`、`functions.go`、`dml.go`、`misc.go`、`stats.go`，以及 `expressions_test.go`、`functions_test.go`、`dml_test.go`、`util_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查，并人工复核文档覆盖定位、运行流程和安全扩展入口。
