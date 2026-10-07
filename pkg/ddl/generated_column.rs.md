# `pkg/ddl/generated_column.rs` 逻辑说明

## 文件定位

`pkg/ddl/generated_column.rs` 属于 `astersql-ddl` crate；crate 根由 `pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/ddl/lib.rs` 通过 `pub mod generated_column` 将本模块公开。它位于 DDL 元数据构造和校验层：在生成列或表达式索引的元数据进入后续 DDL 作业流程前，检查表达式形状、列依赖、列顺序、自动增量引用以及 `EMBED_TEXT` 的专有限制。本文件不创建或调度 DDL job，也不推进 schema state。

RustCodeGraph 的文件节点将直接生产使用方概括为 `pkg/ddl/create_table.rs` 与 `pkg/session/runtime/ddl.rs`；源码搜索还显示，模块内的简化数据模型被 `pkg/ddl/add_column.rs`、`pkg/ddl/modify_column.rs` 和 `pkg/ddl/constraint.rs` 分别用于新增列、修改列依赖保护和约束依赖提取。测试模块由 `pkg/ddl/lib.rs` 中的 `mod generated_column_test` 独立装配，测试逻辑没有内嵌在生产文件中。

## 核心职责

本文件有两组相互补充的职责。

1. 以 `GenerationAttribute`、`GenerationType`、`ExpressionNode` 和 `GeneratedColumnError` 为边界无关的简化模型，提供可复用的生成列校验：全表/单列依赖顺序、依赖存在性、表达式列名提取、非法表达式结构、自动增量引用和修改生成列限制。
2. 对真实的 `astersql_parser_ast::ExprNode` 执行 `EMBED_TEXT`/embedding 函数专项校验，以便 `CREATE TABLE`、表达式索引物化以及 session 的 `ALTER TABLE ADD COLUMN` 路径直接使用 parser AST，而不先转换为简化 `ExpressionNode`。

它是“DDL 接受定义之前的纯校验组件”，不是生成列求值器。虚拟列读取时计算、存储列写入时计算的实际执行不在本文件；这里仅决定定义是否允许进入元数据/DDL 主链。

## 主要符号

- `GenerationAttribute { position, generated, dependencies }`：全表依赖校验所需的列属性快照。位置是列偏移，依赖集合使用规范化后的列名。
- `GenerationType::{Column, Index}`：区分普通生成列和表达式索引。两者的函数稳定性与数组 `CAST` 规则不同。
- `ExpressionNode`：只保留校验需要的 AST 形状，包括列、函数、聚合、行值、窗口函数、数组转换、子查询、变量和字面量；它不是 parser 的完整 AST。
- `GeneratedColumnError`：简化校验器的结构化错误集合，覆盖未知列、非前序生成列、非法函数/结构、表达式索引开关、存储属性变化、索引保护和自增引用。
- `verify_column_generation`：在属性表中校验指定列；普通列直接成功，生成列的每个依赖必须存在，且被依赖的生成列位置必须更小。
- `find_position_relative_column`：把 `ColumnPosition::{None, First, After}` 转换成新列偏移；`After` 的目标不存在时返回 `UnknownColumn`。
- `check_depended_columns_exist`：从传入的可变依赖集合中移除所有可见列名；剩余任意元素作为 `UnknownColumn` 返回。隐藏列有意不算可引用列，并且函数会修改输入集合。
- `verify_column_generation_single`：用于 ADD COLUMN；先求目标位置，再拒绝位于该位置或其后的被依赖生成列。
- `find_column_names_in_expr`：递归遍历简化 AST，收集列引用并转为 ASCII 小写；集合自然去重。
- `check_illegal_function_for_generated`：递归检查简化 AST；特殊处理 `GROUPING`、表达式索引函数稳定性，以及只有根节点表达式索引允许的 `CastArray`。
- `has_dependent_generated_column`：扫描 `TableInfo.columns`，返回第一个依赖指定列的列名及其 `hidden` 标志。调用方借此区分普通生成列和隐藏表达式索引。
- `check_auto_increment_reference`：当任一 `auto_increment` 列名出现在依赖集合中时，返回携带当前生成列名的错误。
- `check_modify_generated_column`：比较旧/新列的存储属性、表达式、类型和索引状态，必要时以新列替换旧列构造全表快照并重跑依赖校验。当前代码搜索没有找到生产调用者；修改列主链 `pkg/ddl/modify_column.rs::advance_modify_column` 只直接调用了 `has_dependent_generated_column`。
- `check_embed_text_generated_column`：在真实 parser AST 上要求部署允许 `EMBED_TEXT`、表达式根节点本身就是 `EMBED_TEXT` 调用、列为 STORED，并且 `ExtractEmbedTextInfo` 能解析参数。
- `embed_text_dependency_error`：统一构造“生成列依赖使用 EMBED_TEXT 的生成列”的 `[ddl:3106]` 错误文本。
- `check_embedding_function_usage`：以局部 `Checker` 实现 `ExprNodeVisitor`。只有表达式含 `EMBED_TEXT` 时才启用严格的 embedding 参数检查；表达式索引始终阻止 `EMBED_TEXT` 和 `vec_embed_*`。文件没有模块级常量、trait、条件编译项或持久状态。

## 执行流程

新增列的简化主链位于 `pkg/ddl/add_column.rs::create_new_column`：若定义含生成表达式，先拒绝当前路径不支持的 STORED 列，再调用 `check_illegal_function_for_generated`；复制依赖集合交给 `check_depended_columns_exist`，保留原集合继续做 `verify_column_generation_single`；若会话没有允许生成列引用自增列，再调用 `check_auto_increment_reference`；全部成功后才把表达式文本、存储标志和依赖写入 `ColumnInfo`。

CREATE TABLE 的真实 AST 主链位于 `pkg/ddl/create_table.rs`。构建单列元数据时，Generated 选项依次调用 `check_embedding_function_usage(..., false)` 和 `check_embed_text_generated_column`，随后收集依赖并写入 `model::ColumnInfo`。表达式索引由 `materialize_expression_index_columns` 调用 `check_embedding_function_usage(..., true)` 后物化为隐藏虚拟列。表级 `check_table_info_valid_with_stmt` 独立核验限定名、未知列、生成列只能依赖更早的生成列，并在普通依赖顺序之后用 `embed_text_dependency_error` 拒绝依赖 embedding 生成列，以保持 Go 的错误优先级。

ALTER TABLE ADD COLUMN 的 session 路径在 `pkg/session/runtime/ddl.rs` 中先调用 `check_embed_text_generated_column`；即使根表达式和 STORED 属性本身合法，该路径仍明确拒绝通过 ALTER 新增 `EMBED_TEXT` 生成列。`ConcreteSession::check_embedding_column_dependencies` 又在生成列元数据上解析被依赖列的表达式，并复用 `embed_text_dependency_error`。

非法表达式检查的内部顺序是深度优先遍历并尽早返回：`GROUPING` 映射成聚合错误；不支持函数映射成 `IllegalFunction`；表达式索引中不保证跨版本可用的函数在开关关闭时返回 `UnsupportedExpressionIndex`；子查询/变量、聚合、行值、窗口函数分别映射到固定错误。数组 `CAST` 只有在 `GenerationType::Index` 且位于整个表达式根部时允许，一旦嵌套进函数参数或另一个数组转换，递归参数 `allow_array_cast = false` 会将其拒绝。

## 数据与状态

所有校验都以借用的 `ColumnInfo`、`TableInfo`、表达式节点或临时 `HashMap`/`HashSet` 为输入，没有全局可变状态。`verify_column_generation` 使用列名到 `GenerationAttribute` 的映射表达依赖图，但只检查“一跳依赖存在且生成列位于当前列之前”；严格递增的位置关系同时排除了生成列依赖环。

需要特别注意两个可观察的数据约定。其一，`find_column_names_in_expr` 将名称转为 ASCII 小写，而其他依赖检查直接比较字符串，因此调用方构造 `ColumnInfo.name` 和依赖集合时必须维持相同的规范化约定。其二，`check_depended_columns_exist` 会消费式地移除已匹配依赖，调用方若仍需完整集合必须像 `create_new_column` 一样先克隆。

真实 AST 的 `check_embedding_function_usage` 在局部 `Checker` 中累积布尔标志和首个参数校验文本；访问结束后按 `blocked`、`aggregate`、`row`、`window`、`other_error`、非索引数组 `CAST` 的顺序选择错误。这一优先级是行为的一部分。

## 依赖与调用关系

向上调用关系：

- `pkg/ddl/add_column.rs::create_new_column` 使用 `check_illegal_function_for_generated`、`check_depended_columns_exist`、`verify_column_generation_single`、`check_auto_increment_reference`。
- `pkg/ddl/modify_column.rs::advance_modify_column` 使用 `has_dependent_generated_column`，并依据返回的 `hidden` 选择 `DependentFunctionalIndex` 或 `DependentGeneratedColumn`。
- `pkg/ddl/constraint.rs::find_dependent_columns` 复用 `find_column_names_in_expr`，再转成有序 `BTreeSet`。
- `pkg/ddl/create_table.rs` 使用两个 embedding 校验函数和依赖错误构造器；`pkg/session/runtime/ddl.rs` 使用 `check_embed_text_generated_column` 与 `embed_text_dependency_error`。
- `verify_column_generation` 由本文件的 `check_modify_generated_column` 调用，但后者当前没有搜索到生产入口。`find_position_relative_column` 由 `verify_column_generation_single` 调用。

向下依赖关系：基础校验只依赖 `std::collections` 及 `crate::column::{ColumnInfo, ColumnPosition, TableInfo}`。embedding 路径直接依赖 `astersql-parser-ast` 的 `ExprNode`/visitor、`astersql-parser` 的错误构造，以及 `astersql-expression` 的 `ContainsEmbedTextFunc`、`CheckEmbedTextAllowed`、`IsEmbedTextFuncCall`、`ExtractEmbedTextInfo`、非法函数表和正式函数注册表；这些依赖均在 `pkg/ddl/Cargo.toml` 声明。模块不访问 KV、系统表或网络。

## 错误处理与边界

简化路径返回 `Result<(), GeneratedColumnError>`，让调用方映射成其领域错误。错误通常在发现第一个违规项时返回；由于 `HashSet` 遍历无序，当存在多个未知依赖时，`check_depended_columns_exist` 返回哪一个名称没有稳定顺序，调用方和测试不应依赖多错误集合中的具体首项。

重要边界包括：非生成列无需进行依赖顺序检查；普通列可以被后向引用，只有“依赖对象也是生成列”才要求位于之前；隐藏列不会满足依赖存在性；`ColumnPosition::None` 表示追加，`First` 为 0，`After` 为目标偏移加一；修改时 VIRTUAL/STORED 不可互换，STORED 生成列表达式不可改，被索引覆盖的生成列表达式不可改，表达式相同但索引列类型变化也不可改。

embedding 路径直接返回 `astersql_parser::errors::Error` 并保留与 Go 对齐的错误码文本，如 `[ddl:3106]`、`[ddl:3758]`、`[ddl:1111]`、`[ddl:3800]`、`[ddl:3764]` 和 `[ddl:3593]`。`check_embed_text_generated_column` 对不含 `EMBED_TEXT` 的表达式立即成功；因此一般生成列表达式的完整合法性仍由其他既有路径负责，不能把该函数单独当作通用校验器。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务、文件句柄或外部资源。每次调用只创建调用栈内的集合或 visitor；递归生命周期不超过当前表达式检查。其并发安全性来自无共享可变状态，但复杂表达式的递归深度受 AST 深度影响，依赖扫描则是线性扫描列集合。

它也不拥有 DDL job、owner、schema version 或回填资源的生命周期。校验成功只代表定义可以继续进入调用方流程；是否持久化、回滚、状态推进和集群同步由 `pkg/ddl` 的其他执行组件负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/generated_column.go`。Rust 的 `GenerationAttribute` 对应 Go 的 `columnGenerationInDDL`；`verify_column_generation`、`verify_column_generation_single`、`check_depended_columns_exist`、`find_position_relative_column`、`find_column_names_in_expr`、`has_dependent_generated_column`、`check_auto_increment_reference` 分别对应同名驼峰 Go 函数或其等价逻辑。Rust 的 `check_illegal_function_for_generated` 对应 `illegalFunctionChecker` 与 `checkIllegalFn4Generated`，包括 `GROUPING` 归类、函数参数验证意图、表达式索引 GA 函数开关以及数组 `CAST` 根节点规则。

`check_modify_generated_column` 合并表达了 Go `checkModifyGeneratedColumn` 中的存储状态/全表依赖检查以及 `checkIndexOrStored` 的表达式、类型、STORED 和索引保护，但 Rust 签名只接收已算好的 `indexed`，也不包含 Go 函数中的 session 开关、parser 列定义和 EMBED_TEXT 检查；更关键的是，它当前没有生产调用者。因此不能声称 Rust 修改列主链已经通过该函数完整复刻 Go 流程。

Rust 的 `check_embed_text_generated_column` 与 `embed_text_dependency_error` 分别对应 Go 的 `checkEmbedTextGeneratedColumn` 和 `embedTextDependencyErr`。Go 另有 `findEmbedTextDependency`；Rust 没有同名公共函数，而是在 `create_table.rs::check_table_info_valid_with_stmt` 和 `session/runtime/ddl.rs::check_embedding_column_dependencies` 的上下文中直接扫描依赖。Go 的 `checkExpressionIndexAutoIncrement` 在本文件也没有直接对应物。

Rust 还保留一套简化 `ExpressionNode`，同时 embedding 新增逻辑使用真实 parser AST。这是当前迁移状态，不应把两套遍历器视为自动同步；修改表达式规则时必须检查两条路径及 Go visitor 的优先级。

## 扩展指南

新增通用生成列表达式规则时，优先修改 `ExpressionNode`、`GeneratedColumnError` 和 `check_illegal_function_for_generated::inspect`，并同步 `pkg/ddl/generated_column_test.rs` 的独立测试；若规则影响真实 SQL AST 或 embedding 参数，还必须同步 `check_embedding_function_usage::Checker`、`pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs` 以及相关集成式测试。新增错误分支要保持 Go `pkg/ddl/generated_column.go` 的错误分类和优先级，尤其不要把 `GROUPING`、行值、窗口函数、数组 `CAST` 都折叠成普通非法函数。

修改依赖规则时，应同时审查 `verify_column_generation`（全表）、`verify_column_generation_single`（新增单列）、`check_depended_columns_exist`（可见性）、`has_dependent_generated_column`（删除/修改保护）以及 CREATE TABLE 自有的真实 AST 依赖检查，避免只修一条入口。若要让 `check_modify_generated_column` 成为生产主链的一部分，应先补齐与 Go `checkModifyGeneratedColumn` 的 session 开关、位置变化、非法函数、自增和 embedding 语义，而不是直接替换现有 `modify_column.rs` 检查。

兼容风险主要是 MySQL/TiDB 错误码、错误优先级、列名大小写和隐藏表达式索引语义；性能风险主要来自对所有列的重复线性扫描和深表达式递归。测试至少应覆盖 `pkg/ddl/generated_column_test.rs`，并视入口同步 `pkg/ddl/db_integration_test.rs`、`pkg/ddl/column_modify_test.rs`、`pkg/ddl/add_column_test.rs` 和 `pkg/ddl/create_table_validation_aster_unit_test.rs`。测试仍应放在独立文件，不要移入生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/ddl/generated_column.rs --offset 1 --limit 400` 与后续 400 行起的查询读取了完整 569 行，并报告直接使用文件 `pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs`；`query` 精确确认了关键函数定义。`callers` 对 `verify_column_generation` 和 `check_embed_text_generated_column` 的精确查询均在约 60 秒内无结果而超时，因此具体调用行改由源码引用搜索核验。
- 已读生产与配置路径：`pkg/ddl/generated_column.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、`pkg/ddl/add_column.rs`、`pkg/ddl/modify_column.rs`、`pkg/ddl/constraint.rs`、`pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs`。
- 已读契约与 Go 对照：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`、`pkg/ddl/generated_column.go`。DDL 文档只作为入口，本文行为结论均以源码和测试复核。
- 已读测试：完整的 `pkg/ddl/generated_column_test.rs`，以及 `pkg/ddl/db_integration_test.rs`、`pkg/ddl/column_modify_test.rs`、`pkg/ddl/create_table_validation_aster_unit_test.rs` 中的相关用例。证据覆盖 `GROUPING` 错误分类、表达式索引根/嵌套数组 `CAST`、未知依赖、前序生成列规则、非法/合法函数、列名小写化和 deployment 先于 EMBED_TEXT 形状校验的错误优先级。
- 人工复核结论：本文区分了纯校验与 DDL job 生命周期、简化 AST 与真实 parser AST、已接线函数与当前无生产调用者，并给出了新增功能的接入点、同步测试及兼容/性能风险；未把 Go 的完整实现或未来目标描述成 Rust 当前已支持能力。
