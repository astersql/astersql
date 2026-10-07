# `dumpling/export/schema_projection.rs`

## 文件定位

本文件属于 `astersql-dumpling-export` library crate；crate 根在 `dumpling/export/lib.rs`，通过 `#[path = "schema_projection.rs"] mod schema_projection;` 将它注册为私有模块。它位于 Dumpling 列过滤的“表元数据重写”阶段：`dumpling/export/dump.rs::prepareColumnProjection` 先从服务器取得每张基础表的 `SHOW CREATE TABLE`，再调用本文件解析、裁剪和校验 AST，最终把恢复出的 SQL 写入 `columnProjection.schemaSQL`。因此它不会筛选数据行，也不负责发现可写列；它保证数据列被过滤后，随导出产物写出的 `CREATE TABLE` 仍只引用保留列，并保持可解析及关键 DDL 约束。

直接配套的 SQL 恢复实现位于 `dumpling/export/schema_projection_restore.rs`，由本文件以私有子模块 `restore` 引入。独立 Rust 测试是 `dumpling/export/schema_projection_test.rs`；Go 语义基线是同目录 `schema_projection.go` 和 `schema_projection_test.go`。

## 核心职责

1. `parse_table_schema` 将一条 SQL 严格解析为 `CreateTableStmt`，并以原始全部列初始化保留列集合。
2. `build_projected_table_schema` / `project_table_schema` 按用户选中的列裁剪列定义与约束，同时闭包式保留可由当前保留列推导的生成列。
3. 投影后重新验证不能被静默破坏的 DDL 不变量：默认值与 `ON UPDATE` 表达式、分区/子分区键、TTL 列、`AUTO_INCREMENT` 的索引要求、`AUTO_RANDOM` 的聚簇主键要求。
4. `validate_foreign_key_parents` 在全部表 AST 已建好后，验证导出范围内父表的被引用列仍存在且仍有合格的前缀索引；导出范围外的父表不在这里重写，因而不作断言。
5. `restore_projected_schema` 将裁剪后的 AST 交给专用恢复器生成 SQL；`new_schema_parser` 将会话 `sql_mode` 应用到解析器，保证 ANSI 等语法按源会话解释。

该模块的原则是“可整体删除的元数据可过滤，不可整体删除且会改变列语义的表达式必须报错”：例如依赖已删除列的 `CHECK` 或索引约束会被删掉，而保留列上的 `DEFAULT`/`ON UPDATE` 若依赖已删除列则返回错误。

## 主要符号

- `ProjectedTableSchema { create_table, retained_columns }`：单表投影状态。`create_table: Box<ast::CreateTableStmt>` 是会被就地裁剪并最终恢复的 AST；`retained_columns: HashSet<String>` 保存规范化小写列名，供依赖和跨表外键检查使用。
- `ProjectedTableSchemas = HashMap<(String, String), ProjectedTableSchema>`：本次导出内所有基础表的投影结果，键保留数据库名和表名原始大小写；模糊查找由 `lookup_schema` 单独实现。
- `parse_table_schema(parser, sql)`：公开到 crate 内的严格解析入口。解析失败会加上投影上下文；结果不是 `CREATE TABLE` 时拒绝；成功后把 `Cols[*].Name.Name.L` 收集为初始保留集合。
- `expression_columns`：用 parser AST 的 `Visitor::enter_column_name` 收集表达式内列引用，避免把函数名、字符串或限定名误当成依赖。`uses_only_retained_columns` 在此集合上做子集判断。
- `keep_constraint`：只有约束的列式索引项、表达式索引项、约束表达式和部分索引条件全部仅依赖保留列时才保留整个约束。
- `index_constraint` / `index_covers_columns` / `has_parent_index`：识别普通主键、索引、唯一索引，并要求被引用列是索引的完整左前缀、每项为列而非表达式且没有前缀长度；列级主键/唯一键也可满足单列父键。
- `has_clustered_primary_key`：检查目标列是否属于列级或表级主键，并排除显式 `NONCLUSTERED`，供 `AUTO_RANDOM` 验证。
- `build_projected_table_schema(parser, sql, selected)`：组合入口，依次调用 `parse_table_schema` 与 `project_table_schema`。
- `project_table_schema(schema, selected)`：本文件的核心变换函数；消费并返回 `ProjectedTableSchema`，保证错误时不会暴露半成品。
- `lookup_schema(schemas, database, table)`：先做完全匹配，再借助大小写不敏感正则实现接近 Go `strings.EqualFold` 的 Unicode simple folding；折叠后多匹配会报歧义。
- `validate_foreign_key_parent` / `validate_foreign_key_parents`：分别验证一个引用和一张子表中列级、表级的所有引用。
- `restore_projected_schema(table)`：调用 `restore::restore`，并把底层恢复错误包装为投影上下文错误。
- `new_schema_parser(params)`：创建 parser；存在 `sql_mode` 参数时先 `FormatSQLModeStr`，再 `GetSQLMode` 并设置到 parser。

## 执行流程

完整应用主链由 `dumpling/export/dump.rs::prepareColumnProjection` 驱动：

1. 先调用 `buildColumnProjection` 得到源列和选中列。若 `NoSchemas` 为真或没有任何列被过滤，直接返回，不解析 schema；存在视图且需要重写 schema 时则提前拒绝。
2. 通过 `new_schema_parser(&conf.SessionParams)` 创建复用的 parser。对每张基础表执行 `ShowCreateTable`；列数发生变化时调用 `build_projected_table_schema`，未过滤表只调用 `parse_table_schema`，从而保留原始 SQL 输出。
3. `project_table_schema` 首先收集主分区和子分区的显式列及表达式列；空列列表的 `PARTITION BY KEY()` 因真实依赖无法从 AST 明确得出而立即拒绝。
4. 将 `selected` 转为小写集合；随后按声明顺序扫描生成列。某生成列表达式只依赖当前集合时，将该生成列加入集合。这一顺序允许后面的生成列依赖前面已经自动保留的生成列，但不会反向保留声明在依赖者之后的列。
5. 消费原列数组，只留下保留列。对留下的列，删除依赖已移除列的 `CHECK`；若 `DEFAULT` 或 `ON UPDATE` 依赖已移除列则报错。然后用 `keep_constraint` 整体过滤不再成立的表级约束及索引。
6. 在裁剪后的 AST 上依次验证 `AUTO_RANDOM`、`AUTO_INCREMENT`、分区列集合和 TTL 列。成功后用最终集合覆盖 `schema.retained_columns`。
7. 过滤表通过 `restore_projected_schema` 生成新的 `CREATE TABLE`；未过滤表继续使用服务器原始字符串。所有表结果先放进 `ProjectedTableSchemas`。
8. 第二遍遍历所有基础表，调用 `validate_foreign_key_parents`。先建全集、后验外键是必要顺序：父表不能因迭代次序尚未进入 map 而被误判为导出范围外。

## 数据与状态

状态仅存在于调用栈和调用者持有的 map 中。`project_table_schema` 取得 `ProjectedTableSchema` 的所有权，使用 `std::mem::take` 搬出列及列选项后重建数组；这避免克隆大型 AST，也保证返回对象中的 AST 与 `retained_columns` 同步。列匹配统一使用 parser 提供的 `.L` 小写规范名，错误信息和恢复输出使用 `.O` 原始拼写。

`retained_columns` 不等同于用户直接选中的列：它还包含按声明顺序推导出的生成列。相反，索引、检查约束等不会向集合反向增加依赖列；它们只能在全部依赖已保留时留下。`ProjectedTableSchemas` 的精确键区分大小写，以便同时容纳 `Orders` 与 `orders`；只有外键引用查找失败时才进入 Unicode 不区分大小写匹配。

本文件不持有数据库连接、文件句柄或全局缓存。传入的 `Parser` 可由调用者在多个表间顺序复用；`new_schema_parser` 的唯一配置状态是由 `SessionParams["sql_mode"]` 派生的 SQL mode。

## 依赖与调用关系

上游直接调用者是 `dumpling/export/dump.rs::prepareColumnProjection`：它调用 `new_schema_parser`、`build_projected_table_schema`、`parse_table_schema`、`restore_projected_schema` 和 `validate_foreign_key_parents`。`dumpling/export/lib.rs` 只声明私有模块，不对 crate 外暴露这些 API。测试模块通过同一 crate 的私有可见性直接调用各入口。

下游依赖如下：

- `schema-parser`（Cargo 包 `astersql-parser`）提供 parser、DDL AST、visitor 和规范化标识符；`expression_columns` 的依赖判断直接建立在 AST visitor 语义上。
- `schema-mysql` 提供 SQL mode 字符串格式化、解析与 parser 设置所需的位标志。
- `regex` 用于 `lookup_schema` 的转义后锚定、大小写不敏感匹配；先精确查找可保留大小写不同表的确定性。
- crate 内 `errors_new` 统一构造 `Result` 错误，`escapeString` 负责错误消息中的数据库、表、列名转义。
- 私有子模块 `schema_projection_restore.rs::restore` 负责 AST 到 SQL；选择与依赖判定完全留在本文件，恢复器不回退到修改原 SQL 文本。

`Cargo.toml` 将该 crate 标记为 Go 包 `dumpling/export` 的 Rust library 移植，并以 workspace path 依赖连接 parser 相关 crates；没有控制本模块的 feature 或条件编译项。

## 错误处理与边界

所有可恢复失败使用 crate 的 `Result` 返回，并在上游 `prepareColumnProjection` 再补充具体数据库/表上下文。主要拒绝条件为：SQL 无法解析、语句不是 `CREATE TABLE`、`PARTITION BY KEY()` 无显式列、保留列的默认或更新表达式引用已删除列、自动列失去合法索引、分区或 TTL 引用已删除列、导出范围内外键父列被移除或不再有合格索引、外键索引项不是列、大小写折叠得到多个父表，以及 SQL mode 或 SQL 恢复失败。

几个有意边界需要保持：

- 空表达式视为不依赖列；函数调用和字符串字面量不是列依赖。
- 依赖删除列的 `CHECK`、表达式索引、部分索引条件或其他表约束会整体删除，不会改写表达式。
- 父索引可以比外键列更多，但外键列必须按顺序覆盖索引左前缀；前缀索引和表达式索引不合格。当前 Rust `index_constraint` 接受 parser 归一化后的 `PrimaryKey | Index | Unique` 三类，测试同时确认 `FULLTEXT` 不合格而 vector/表达式约束按依赖过滤。
- 找不到父表表示它不在本次导出范围内，函数返回成功；这不是对外部表有效性的证明。
- `lookup_schema` 中正则构建使用已转义输入，`expect("escaped table name")` 表达内部不变量，而不是用户可触发的常规错误路径。
- `restore_projected_schema` 的具体格式与支持范围由相邻恢复模块决定；本文件只包装其错误，不保证任意未来 AST 节点都已可恢复。

## 并发与资源生命周期

本模块没有线程、异步任务、锁、通道或原子变量。所有变换同步执行，局部 `HashSet`/`Vec`/正则在函数返回时释放；AST 由 `ProjectedTableSchema` 独占。共享只发生在不可变借用的 `ProjectedTableSchemas` 外键校验阶段，因此本文件自身不存在数据竞争或锁顺序问题。

生命周期上的关键约束是阶段顺序而非并发：调用者必须先构建全部表的 `ProjectedTableSchema`，再统一执行外键校验；在 map 构建过程中校验会把尚未插入的父表错误地当成导出范围外。parser 在表间复用，但调用为顺序的 `&mut Parser` 借用；若未来并行化表解析，应为每个工作单元提供独立 parser，并保持“全集构建完成”的屏障后再做跨表校验。

## 与 Go 版本的对应关系

`dumpling/export/schema_projection.go` 是逐函数语义基线：Rust 的 `ProjectedTableSchema`、`parse_table_schema`、`build_projected_table_schema`、约束/索引辅助函数、自动列校验、分区和 TTL 校验、外键校验及 schema lookup，分别对应 Go 的同名 camelCase 实现。两者都按声明顺序补入生成列，都在裁剪后验证自动列/分区/TTL，也都先精确匹配表名再使用不区分大小写匹配并拒绝歧义。

实现层面的可核对差异：Go 使用 `ddl.FindColumnNamesInExpr`，Rust 用 parser `Visitor` 的 `enter_column_name` 达到相同 AST 列依赖语义；Go 直接用 `strings.EqualFold`，Rust 用转义、首尾锚定、case-insensitive 的 regex 来覆盖 Unicode simple folding；Go 用 `format.RestoreCtx`，Rust 把恢复细节拆到 `schema_projection_restore.rs`。Rust 还显式检查外键 index part 的 `Column` 非空并返回错误，避免对异常 AST 解引用。

`dumpling/export/schema_projection_test.go` 覆盖原始移植意图，包括生成列、分区、TTL、默认表达式、自动列、外键和大小写表名；`schema_projection_test.rs` 在保持这些行为的基础上还验证子分区、部分/表达式/vector 索引、Unicode simple folding、typed literal 与 TiDB special comment、SQL mode、枚举/集合反斜杠和 placement policy 恢复。修改 Rust 行为时应同时核对 Go 测试，而不能为通过 Rust 测试删减 Go 分支。

## 扩展指南

- 新增 AST 依赖来源时，优先扩展 `expression_columns` 或调用它的判定点，不能退回字符串搜索；为列级选项和表级约束分别增加 `schema_projection_test.rs` 回归。
- 新增可影响表合法性的 DDL 选项时，在 `project_table_schema` 裁剪完成后加入明确验证，并同步 `schema_projection_restore.rs` 的恢复能力；若 Go 已有对应逻辑，保持错误条件与消息意图一致。
- 调整生成列策略时必须保留声明顺序语义，并覆盖“生成列依赖更早生成列”及“依赖尚未保留列”的双向用例。
- 扩展可作为外键父索引的约束类型时，应同时审查 `index_constraint`、`index_covers_columns` 和 Go `hasParentIndex`，特别验证复合索引左前缀、前缀长度、表达式索引及列级 unique。
- 改动大小写查找时必须保持精确匹配优先、多折叠匹配报歧义以及 Go `EqualFold` 的 Unicode 类；现有 `projected_schema_lookup_*` 测试是最低回归集合。
- 若接入并行解析，只能并行单表构建；`prepareColumnProjection` 的第二阶段外键校验屏障不能移除。
- 测试逻辑应继续放在独立的 `dumpling/export/schema_projection_test.rs`，不要内嵌回生产文件；应用级缓存与两阶段行为还应同步更新 `dumpling/export/dump_test.rs`。本文件属于 Go 移植，修改时也应检查 `schema_projection.go` / `schema_projection_test.go` 的对应语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标源码由 `node --file dumpling/export/schema_projection.rs --offset 1 --limit 400` 完整读取，并显示被 `dumpling/export/dump.rs`、`dump_test.rs`、`schema_projection_test.rs` 等文件使用。
- RustCodeGraph `query`：确认 `project_table_schema` 位于第 152 行、`lookup_schema` 位于第 263 行、`has_clustered_primary_key` 位于第 115 行；`callees` 确认核心内部边 `build_projected_table_schema -> parse_table_schema/project_table_schema`、`project_table_schema -> expression_columns/uses_only_retained_columns/keep_constraint/has_parent_index/has_clustered_primary_key`、`validate_foreign_key_parent -> lookup_schema/has_parent_index`。限定名 `callers` 未返回结果且 `callees` 输出混有同名候选，因此上游边改由直接入口源码核对，不将图中的噪声候选作为结论。
- 已读生产路径：`dumpling/export/schema_projection.rs`、`dumpling/export/schema_projection_restore.rs`、`dumpling/export/dump.rs::prepareColumnProjection`、`dumpling/export/lib.rs`、`dumpling/export/Cargo.toml`。
- 已读对照与测试：`dumpling/export/schema_projection.go`、`dumpling/export/schema_projection_test.go`、`dumpling/export/schema_projection_test.rs`；另由调用搜索核对 `dumpling/export/dump_test.rs` 的投影准备、缓存、外键和生成列场景。
- 人工复核结论：该文件存在是为了让列过滤同时安全地重写 `CREATE TABLE`；运行时以单表 AST 投影和全表外键二阶段校验工作；安全扩展点集中在 AST 依赖收集、投影后不变量、父索引资格、恢复器和独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定的 `test -f` 与 11 个固定二级标题计数命令。
