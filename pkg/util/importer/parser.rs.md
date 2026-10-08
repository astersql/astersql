# `pkg/util/importer/parser.rs`

## 文件定位

本文件属于 `astersql-util-importer` crate，负责把配置中的 `CREATE TABLE` 和可选的 `CREATE [UNIQUE] INDEX` 文本转换为导入器使用的轻量表元数据。crate 入口 `pkg/util/importer/lib.rs` 将 `parser` 声明为公开模块并重导出其公开项；生产入口 `pkg/util/importer/importer.rs::process` 依次调用 `parse_table_sql`、`parse_index_sql`，随后才执行 DDL 和并发数据生成。因此这里位于“读取导入配置”与“生成 INSERT 数据”之间，不负责向数据库执行 SQL，也不是通用 SQL 解析器。

`pkg/util/importer/Cargo.toml` 的真实 `[dependencies]` 为空。完整 TiDB parser 相关 crate 只列在 `target.'cfg(any())'.dependencies` 下，该条件恒假；当前实现实际使用字符串扫描，而非构造 SQL AST。这一边界决定了本文件只应覆盖导入工具所需的 DDL 子集。

## 核心职责

- `parse_table_sql` 验证建表语句的基本头部和外层括号，解析表名、列定义及表级约束，并生成 `Table::columns`、`unique_indices`、`indices`、`unsigned_columns` 与 `column_list`。
- `parse_index_sql` 处理独立的建索引语句，校验其目标表与已解析表一致，然后把索引列登记为普通或唯一索引。空字符串表示没有独立索引 SQL，直接成功。
- `field_type` 把 MySQL 类型文本压缩成导入数据生成所需的 `FieldKind`、无符号标志和长度；`pkg/util/importer/db.rs::generate_column_data` 据此选择数值、字符串或日期时间生成器。
- `Column::parse_comment_rules` 从列注释的首个 `[[...]]` 块中读取 `range`、`step`、`set` 生成规则；这些字段随后控制随机范围或唯一序列。
- `split_top_level`、`identifier`、`constraint_columns` 和 `extract_comment` 提供轻量词法辅助，避免在括号或引号内部错误地按逗号切分，并统一标识符与注释内容。

## 主要符号

- `DEFAULT_STEP: i64 = 1`：未显式给出 `step` 时的唯一值递增步长。
- `FieldKind`：导入器支持的类型族，涵盖四类整数、字符串/二进制/LOB、浮点/定点数以及日期时间类型。它不是完整 MySQL 类型枚举，而是 `db.rs` 生成策略的分派键。
- `FieldType { kind, unsigned, length }`：保存规范化类型族、`UNSIGNED` 标志及字符串类长度。显式类型长度优先，否则 `field_type` 使用按类型族定义的默认值。
- `Column`：一列的解析结果。`data: Arc<Datum>` 保存跨行唯一值状态；`minimum`、`maximum`、`set` 和 `step` 来自注释规则；`index` 从 1 开始；`comment` 保留提取后的注释正文。自定义 `Debug` 不输出共享的 `Datum`。
- `Table`：表名、列、索引集合及 INSERT 列清单的聚合。`indices` 的值是 `Option<Arc<Column>>`，所以语法上合法但找不到的索引列仍可用 `None` 留痕；唯一索引和无符号列只保存规范化列名集合。
- `Table::new` / `Default`：构造空元数据；`Table::find_column` 按规范化名称线性查找并克隆 `Arc`；`Table::build_column_list` 生成以逗号连接、反引号转义的列清单。
- `identifier`：去除外围反引号、对限定名取最后一段、还原双反引号并转 ASCII 小写。
- `split_top_level`：记录括号深度与当前引号，在顶层分隔符处切分；它只把反斜杠视为引号转义，不是完整 MySQL lexer。
- `field_type`：识别支持的类型名、首个长度参数和独立的 `unsigned` token；未知类型返回 `ImporterError::UnsupportedColumn`。
- `constraint_columns`：读取约束最外层括号中的列项，移除前缀长度等列后缀后规范化名称。
- `parse_table_sql` / `parse_index_sql`：本文件两个公开解析入口；其余解析函数均为文件内部实现。

## 执行流程

`pkg/util/importer/importer.rs::process` 先创建 `Table::new()`，再把 `Config::table_sql` 交给 `parse_table_sql`。后者按以下顺序工作：

1. 将 SQL 的修剪版本转为小写，仅接受 `CREATE TABLE` 或 `CREATE TEMPORARY TABLE` 头部；不匹配时返回解析错误。
2. 以首个 `(` 和最后一个 `)` 确定表体，以头部最后一个空白分隔 token 作为表名并经 `identifier` 规范化。
3. 清空旧的 `columns`，用 `split_top_level(..., ',')` 拆分列定义和约束定义。与 Go 实现一致，此处不清空构造时分配的索引集合，因此复用同一 `Table` 解析第二张表时只保证列切片被替换。
4. 对表级约束：忽略外键、检查约束及非唯一的命名 `CONSTRAINT`；对其余主键、唯一键、`KEY`、`INDEX` 提取列名。唯一类登记到 `unique_indices`，普通索引登记到 `indices`，后者同时尝试关联已解析列。
5. 对列定义：解析首个 token 为列名，其余文本供 `field_type` 判定；初始化 `Datum`、1 基序号和默认步长，提取 `COMMENT`，登记无符号列及列内主键/自增/唯一属性，然后解析注释规则并追加 `Arc<Column>`。
6. 所有定义完成后，`build_column_list` 生成后续 INSERT 使用的列名清单。

随后 `process` 调用 `parse_index_sql`。非空输入必须以 `CREATE INDEX` 或 `CREATE UNIQUE INDEX` 开头，并包含 ` ON ` 与索引列括号；解析出的目标表名必须等于 `Table::name`。普通索引进入 `indices`，唯一索引进入 `unique_indices`。最后 `pkg/util/importer/db.rs::generate_row_data` 遍历 `Table::columns`，依据列类型、规则和唯一集合生成值，并使用 `table.name` 与 `table.column_list` 组装 INSERT。

## 数据与状态

解析本身通过 `&mut Table` 原地更新状态。`columns` 和普通索引中的已解析列由 `Arc<Column>` 共享，避免在解析结果传给并发 job 时复制列元数据；`process` 完成解析后再把整个 `Table` 包进 `Arc`。`Column::data` 也是 `Arc<Datum>`，其内部状态由 `data.rs` 管理，用于多个生成任务共享唯一序列。

关键不变量如下：

- `Column::index` 是列在本次解析后的 1 基位置；重新解析表时重新从 1 编号。
- `column_list` 与 `columns` 顺序一致，并把列名中的反引号写成双反引号。
- 标识符存储为 ASCII 小写；带 schema 的名字只保留最后一段。
- `unique_indices` 的成员决定 `db.rs::generate_column_data` 是否走唯一值生成路径；索引名字本身不参与判断。
- `range=x` 只设置下界，`range=x,y` 同时设置上下界，更多逗号项被忽略；重复 `set` 规则采用追加语义；格式中出现多个 `=` 的规则被忽略。
- 再次调用 `parse_table_sql` 会清空 `columns`，但不会清空 `indices`、`unique_indices` 或 `unsigned_columns`。这是与 Go `parseTable` 仅替换列切片相同的当前行为，调用方通常为每次导入创建新表对象。

## 依赖与调用关系

上游生产调用边为 `pkg/util/importer/importer.rs::process -> Table::new -> parse_table_sql -> parse_index_sql`。`pkg/util/importer/lib.rs` 公开重导出这些符号，测试因此可通过 `use super::*` 直接使用。仓库文本引用还显示 `parser_test.rs`、`tests.rs` 和 `db_test.rs` 直接构造表并调用解析入口。

内部主要调用边为：

- `parse_table_sql -> identifier / split_top_level / field_type / extract_comment / Column::parse_comment_rules / Table::find_column / Table::build_column_list`；
- `Column::parse_comment_rules -> Column::parse_rule`；
- `parse_index_sql -> identifier / constraint_columns / Table::find_column`；
- `constraint_columns -> split_top_level / identifier`。

下游消费边集中在 `pkg/util/importer/db.rs`：`generate_column_data` 读取 `FieldKind`、`FieldType::unsigned`、唯一集合及列规则，`generate_row_data` 读取表名、列清单与列序列。`job.rs` 通过共享的 `Arc<Table>` 间接使用这些元数据。外部标准库依赖仅有 `HashMap`、`HashSet`、格式化 trait 与 `Arc`；crate 内依赖为 `config::ImporterError` 和 `data::Datum`。

## 错误处理与边界

所有可恢复失败经 `Result<_, ImporterError>` 传播。建表入口会报告非法语句、缺失外层括号、不支持的列类型和非法 `step`；建索引入口会报告非法头部、缺失 `ON`、缺失列括号以及目标表不匹配。`process` 使用 `?` 立即停止后续导入准备，因此解析失败不会执行 DDL 或启动 jobs。

一些输入被有意宽容处理：未知注释规则、格式不是恰好一个 `=` 的规则、非法 `range` 项数、没有成对 `[[...]]` 标记的注释均静默忽略；普通索引引用未知列时成功并在 `indices` 中保存 `None`。`parse_index_sql("")` 成功，但只含空白的字符串会进入语法检查并失败，这一边界由独立 Rust 测试固定。

轻量解析器并不覆盖完整 MySQL 文法。尤其是复杂表达式、特殊注释/转义模式、非标准空白、嵌套语法或关键字出现在非预期位置时，字符串查找可能与 Go AST 解析不同。`split_top_level` 对不平衡右括号会令深度变为负数；当前代码没有单独验证括号平衡。安全扩展时不能把“现有样例可解析”推导为“任意合法 MySQL DDL 均受支持”。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务或数据库连接；所有解析都在调用线程同步完成，临时字符串和切分结果在函数返回时释放。解析结束后，`process` 将 `Table` 放入 `Arc`，由 `process_jobs` 的 worker 共享只读表结构。

需要注意的可变资源是每列的 `Arc<Datum>`：解析器只负责创建它，唯一值生成时的并发安全与生命周期由 `pkg/util/importer/data.rs` 保证。表和列元数据在发布给 workers 前完成构建，公开流程中不存在边解析边生成数据的阶段。普通索引映射中的 `Arc<Column>` 与 `columns` 共享同一列对象；未知列则保存 `None`，不产生悬空引用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/importer/parser.go`。Rust 的 `Column`、`Table`、`parse_table_sql` 和 `parse_index_sql` 分别对应 Go 的 `column`、`table`、`parseTableSQL` 和 `parseIndexSQL`；注释规则、列序号、默认步长、第二次解析只替换列切片、空索引 SQL 成功、表名不匹配失败等语义均有对应实现或 Rust 回归测试。

主要实现差异是 Go 版本通过 `parser.New().ParseOneStmt` 获得 `ast.CreateTableStmt` / `ast.CreateIndexStmt`，字段类型由 `types.FieldType` 表示，并使用 `dbutil.ColumnName` 引用列名；Rust 版本没有运行时 parser 依赖，而是手写轻量扫描并用 `FieldKind` / `FieldType` 保存数据生成所需子集。Cargo 中的 parser/dbutil 依赖仅为 `cfg(any())` 下的移植元数据，不能作为当前运行时调用关系。

错误策略也不同：Go 的 `parseRule` 对非法 `step` 调用 `log.Fatal`，Rust 返回 `ImporterError::Parse` 给调用方；这使 Rust 可测试且不会在库函数中终止进程。Go 的索引 map 保存 `*column`（未知列为 `nil`），Rust 普通索引用 `Option<Arc<Column>>` 显式表达相同边界，而唯一索引只保留列名，因为下游只查询成员关系。Rust 额外维护 `FieldType::unsigned` 和 `unsigned_columns`；实际数据生成读取前者，后者目前没有生产消费方。

## 扩展指南

- 新增列类型时，应同时修改 `FieldKind`、`field_type` 的别名/默认长度映射，以及 `pkg/util/importer/db.rs::generate_column_data` 的生成分支；同步扩展 `pkg/util/importer/parser_test.rs` 和 `db_test.rs`，不要把测试嵌入生产文件。
- 新增注释规则时，入口是 `Column::parse_rule`；需明确重复规则、空值、额外 `=`、解析失败和与既有 `range`/`step`/`set` 的组合语义，并与 Go `column.parseRule`/`parseColumnComment` 对齐。
- 扩展 DDL 语法时，优先修改 `split_top_level`、`identifier`、`constraint_columns` 或两个公开入口中最窄的责任点。必须加入包含引号、反引号转义、括号、前缀索引和排序方向的独立测试，避免顶层逗号切分回归。
- 若要恢复完整 parser crate，应先改变 Cargo 的真实依赖边界，再以 Go AST 行为为准替换轻量扫描；不能仅启用当前 `cfg(any())` 条目并假定 API 已接线。
- 调整表复用行为前，应确认是否继续保留 Go 的“只清列、不清 map”语义；若决定清理旧索引状态，这属于可见行为变化，必须有调用方证据和回归测试。
- 性能上，`find_column` 是线性搜索，约束列多时会重复扫描；若改为名称索引，应保持 `Arc` 身份、未知列的 `None` 表达和列顺序不变。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/util/importer/parser.rs`；`files --filter pkg/util/importer` 列出该文件、`parser_test.rs`、`importer.rs`、`db.rs` 等直接相关文件；`node --file pkg/util/importer/parser.rs --offset 1 --limit 500` 返回完整 460 行源码及 41 个符号。精确 `query parse_table_sql --kind function` 定位到第 300 行。`callers parse_table_sql` 在本次环境中长时间无输出后被中断，故调用边由下述仓库文本引用交叉核验，没有据此臆造图边。
- 源码：`pkg/util/importer/parser.rs`（全部类型、辅助函数与两个公开入口）、`pkg/util/importer/importer.rs`（生产调用顺序）、`pkg/util/importer/db.rs`（解析结果的下游消费）、`pkg/util/importer/lib.rs`（模块声明与重导出）、`pkg/util/importer/config.rs`（错误枚举）、`pkg/util/importer/data.rs`（`Datum` 状态归属）。
- crate 边界：`pkg/util/importer/Cargo.toml`，确认 crate 名、`lib.rs` 入口、空的真实依赖表和 `cfg(any())` 下未启用的 Go parser 对应依赖。
- Go 对照：`pkg/util/importer/parser.go` 与 `pkg/util/importer/importer.go`，核对 AST 解析流程、规则语义、表复用、索引分类及主流程调用位置。
- Rust 测试：`pkg/util/importer/parser_test.rs` 覆盖重复/非法注释规则、第二次解析、转义标识符与注释、LOB 默认长度、空白索引 SQL及未知索引列；`pkg/util/importer/tests.rs` 覆盖外键/范围边界、索引唯一性和数据生成联动；`pkg/util/importer/db_test.rs` 覆盖解析类型到生成范围的消费关系。
- 文档交付只新增本文件，不修改 Rust、Go、Cargo 或总计划；按任务要求不运行 Cargo。结构验收命令及退出码在任务完成时单独记录。
