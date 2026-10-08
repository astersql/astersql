# `pkg/session/runtime/ddl_index_validation.rs`

## 文件定位

本文件属于 `astersql-session` crate 的具体会话运行时，由 `pkg/session/runtime.rs` 以私有模块 `mod ddl_index_validation;` 接入。它只负责 CREATE TABLE 已构造索引元数据的累计键长校验，不负责解析 SQL、构造 `IndexInfo`、执行 DDL job 或校验索引列类型是否合法。直接入口是 `ConcreteSession::validate_create_table_index_lengths`；`pkg/session/runtime/ddl.rs` 在完成表选项、枚举/集合长度和外键父表检查之后、规范化表名及写入临时表或提交 DDL 之前调用它。

`pkg/session/Cargo.toml` 声明该 crate 的库入口为 `lib.rs`，并直接依赖本文件使用的 `astersql-config`、`astersql-meta-model`、`astersql-parser-charset`、`astersql-parser-mysql` 与 `astersql-parser-types`。目标文件没有 feature 条件、模块级状态、公开类型或公开 crate API；入口可见性为 `pub(super)`，只供 `runtime` 父模块内部使用。

## 核心职责

- `validate_create_table_index_lengths` 对 `TableInfo.Indices` 中每个索引逐列计算占用字节并累加，确保普通行存索引不超过全局配置 `max_index_length`。
- `index_column_length` 把元数据中的字符数、位数或十进制精度换算为索引字节数；字符串族乘字符集最大字节宽度，BIT 向上取整为字节，定长数值/日期时间类型使用 MySQL 默认长度。
- 在非严格 SQL mode 下，仅对单列、非唯一、非主键、且列本身没有唯一键标志的超长索引执行前缀截断，并记录 1071 warning；其他超长情况立即返回错误。
- 列存索引统一按 1 字节参与计算。这与 Go `getIndexColumnLength` 的兼容约定一致：列存索引不创建行存 KV 索引，但返回非零最小值以避免后续计算问题。

本文件不是 Go `buildIndexColumns` 的完整复刻。Go 函数还承担列存在性之外的类型合法性、数组/多值索引、前缀规范化和 `IndexColumn` 构造；Rust 入口接收已经构造好的 `TableInfo`，这里只实现 CREATE TABLE 路径所需的累计长度与截断阶段。

## 主要符号

- `default_mysql_type_length(column_type: u8) -> Option<usize>`：在 `astersql_parser_mysql::const::DefaultLengthOfMysqlTypes` 中按类型码查表。返回 `Option`，让调用者把缺失映射转换为 `SessionError`，而不是索引表时 panic。
- `decimal_index_length(precision: usize) -> usize`：按每 9 位十进制数字占 4 字节、余数按两位约 1 字节的公式 `(precision / 9) * 4 + ((precision % 9) + 1) / 2` 计算 DECIMAL 索引长度，对应 Go `calcBytesLengthForDecimal` 在本调用场景中的精度部分。
- `index_column_length(column, index_length, columnar) -> SessionResult<usize>`：私有纯计算辅助函数。长度前缀不是 `UnspecifiedLength` 时优先使用 `IndexColumn.Length`，否则使用 `ColumnInfo.GetFlen()`；随后按字段类型换算字节数。
- `ConcreteSession::validate_create_table_index_lengths(&self, table: &mut TableInfo) -> SessionResult<()>`：父模块可见的唯一入口。它读取全局最大长度和会话 SQL mode，遍历并可能原地修改 `IndexColumn.Length`，也可能向当前语句上下文追加 warning。

## 执行流程

1. `pkg/session/runtime/ddl.rs` 构造好 `TableInfo` 后调用 `validate_create_table_index_lengths(&mut table)`；错误会通过 `?` 中止 CREATE TABLE，因而校验发生在表元数据落地之前。
2. 入口将 `astersql_config::get_global_config().max_index_length` 转成 `usize`。负值转换失败时返回 `max-index-length must not be negative`。
3. 它从 `self.state.borrow().sql_mode` 解析 SQL mode，并缓存 `HasStrictMode()` 的结果。解析失败直接包装为 `SessionError`。
4. 对每个 `IndexInfo`，先取得 `GetColumnarIndexType()` 和键列数量，再把累计值 `total` 置零。每个 `IndexColumn` 通过大小写折叠名 `Name.L` 在 `table.Columns` 中定位 `ColumnInfo`；找不到即返回 `column does not exist`。
5. `index_column_length` 先处理列存索引；普通索引选择显式前缀或字段 `flen`，拒绝无法转成非负 `usize` 的长度。BIT 使用 `div_ceil(8)`；字符/二进制/BLOB 族按字符集 `Maxlen` 放大；整数、浮点、日期时间及 DECIMAL 使用各自规则；其余类型直接使用逻辑长度。
6. 每列结果通过 `checked_add` 加到 `total`。未超过 `maximum` 时继续；一旦超过，形成兼容 MySQL/TiDB 的 `[ddl:1071]Specified key was too long ...` 消息。
7. 严格模式、唯一索引、主键索引、带 `UniKeyFlag` 的列或多列索引任一成立时立即报错。否则这是非严格模式下的单列普通索引：再次用逻辑长度 1 计算单位字节数，以 `maximum / bytes_per_unit` 得到可容纳的前缀长度，写回当前 `IndexColumn.Length`，并调用 `set_warning_with_code(1071, message)`。
8. 所有索引均通过后返回 `Ok(())`，调用方才继续规范化表名并进入临时表保存或普通 DDL 提交流程。

## 数据与状态

输入 `TableInfo` 同时提供 `Columns` 与 `Indices`。索引列通过 `CIStr.L` 与表列的小写规范名匹配；长度来源是 `IndexColumn.Length`，值为 `astersql_parser_types::UnspecifiedLength`（`-1`）时回退到 `ColumnInfo.GetFlen()`。函数只会在允许截断的分支原地改写索引列的 `Length`，不会改写列定义、索引唯一性或全局配置。

只读外部状态有两处：进程级 `get_global_config().max_index_length`，以及会话 `SessionState.sql_mode`。可观察副作用是 `set_warning_with_code` 同时向 `session_vars.StmtCtx` 和 `state.current_warnings` 追加 warning；错误路径在完成写回和 warning 之前返回。累计 `total` 每个索引重新置零，所以限制是“单个索引所有键列之和”，而不是整张表所有索引之和。

## 依赖与调用关系

上游直接调用边为 `pkg/session/runtime/ddl.rs` 的 CREATE TABLE 路径到 `ConcreteSession::validate_create_table_index_lengths`；模块接线来自 `pkg/session/runtime.rs::mod ddl_index_validation`。RustCodeGraph 的 `callees` 结果识别到该入口调用 `index_column_length` 和会话状态的 `borrow`；图未返回完整上游文本，因此用限定 `rg` 确认了 `ddl.rs` 中唯一生产调用点。

下游依赖如下：

- `astersql-config::get_global_config` 提供运行时上限；
- `astersql-meta-model::{TableInfo, ColumnInfo, ColumnarIndexType}` 提供表、列和索引元数据；
- `astersql-parser-types::UnspecifiedLength` 区分整列索引与显式前缀；
- `astersql-parser-mysql::{const,type}` 提供 SQL mode、字段类型码、唯一键 flag、默认物理长度与浮点精度阈值；
- `astersql-parser-charset::charset::GetCharsetInfo` 提供字符串类型的每字符最大字节数；
- `SessionError`/`SessionResult` 统一错误边界，`ConcreteSession::set_warning_with_code` 写入语句 warning。

## 错误处理与边界

- 配置最大索引长度为负、索引前缀/字段长度为负或截断结果超出 `isize` 时，分别返回明确的范围错误。
- 找不到索引引用列时使用原始列名 `Name.O` 报错；不尝试跳过损坏元数据。
- 未知字符集会报告 `Unsupported charset <charset>, collate <collate>`；字符串字节乘法和累计加法均使用 checked 运算，溢出分别报告 `index column length overflow` 与 `index length overflow`。
- 定长 MySQL 类型若缺少默认长度映射会返回错误；FLOAT 根据声明长度是否超过 `MaxFloatPrecisionLength` 选择 FLOAT 或 DOUBLE 的默认长度。
- 超限错误码通过消息文本携带 `[ddl:1071]`。允许截断时 warning 的 code 明确为 1071，并保留截断前的 `total` 与配置上限。
- 单位字节数为零时拒绝做除法。列存索引固定返回 1，因此不会命中该分支；对普通索引，此防线避免异常元数据造成除零。
- 本文件不执行 Go `checkIndexColumn` 的零长度、BLOB/TEXT 必须显式前缀、JSON/VECTOR 或前缀类型合法性检查；这些不能从本文件推断为已支持，相关保证必须由上游元数据构造路径或额外校验提供。

## 并发与资源生命周期

该校验同步执行，不创建线程、异步任务、通道、事务、文件或网络资源。全局配置只在入口开始时读取一次，因此一次调用内使用稳定的 `maximum` 快照。会话 SQL mode 通过 `RefCell` 不可变借用读取，借用在表达式结束后释放；写 warning 时再对 `state` 做可变借用。目标表由调用者以独占 `&mut TableInfo` 传入，Rust 借用规则保证校验/截断期间不会并发修改同一表元数据。

错误返回可能发生在此前索引已经被截断之后，因此函数本身不提供对传入 `TableInfo` 的回滚保证。不过当前 CREATE TABLE 调用点在尚未发布该局部表元数据时使用它，后续错误会放弃整条创建流程；扩展到可复用或已发布元数据时不能假定失败具有事务性原子回滚。

## 与 Go 版本的对应关系

直接对照是 `pkg/ddl/index.go::buildIndexColumns` 与 `getIndexColumnLength`。两端都按索引逐列累加、按字符集最大宽度计算字符串、按位/数值/浮点/DECIMAL/日期时间类别换算，并对列存索引返回 1。两端也都只允许非严格模式的单列普通非唯一索引降级为前缀索引，并生成 1071 warning；多列或唯一语义必须报错。

Rust 实现基于已构造的 `TableInfo` 做后置校验并原地改写 `IndexColumn.Length`；Go `buildIndexColumns` 从 AST `IndexPartSpecification` 构造返回的 `IndexColumn`，同时处理类型合法性、多值索引与前缀规范化。Rust 还直接检查 `index.Unique`、`index.Primary` 和列 `UniKeyFlag`，使“不可因截断破坏唯一性”的意图在本地入口中显式化。Go 的相关回归证据位于 `pkg/ddl/db_test.go::TestBuildMaxLengthIndexWithNonRestrictedSqlMode`：它覆盖多字符集字节宽度、严格模式 1071、非严格模式截断及 warning、唯一索引和复合索引仍报错；`pkg/ddl/tests/serial/serial_test.go::TestChangeMaxIndexLength` 覆盖动态配置上限及精确错误文本。

检索未发现 Rust 中直接调用 `validate_create_table_index_lengths` 或专门断言上述截断/1071 行为的独立测试；现有 `pkg/session/runtime/*test.rs` 只有一般 CREATE TABLE/索引用例。因此 Go 测试是移植语义证据，不应表述成 Rust 回归已覆盖。

## 扩展指南

- 新增字段类型时，优先修改 `index_column_length` 的类型分派，并与 Go `getIndexColumnLength`、parser-mysql 默认长度表核对；至少测试显式前缀、`UnspecifiedLength`、边界上限和溢出。
- 修改超限降级策略时，应集中调整 `validate_create_table_index_lengths` 的 strict/unique/primary/复合条件，保持 1071 错误文本、warning code 与 Go 行为兼容。不要把唯一或复合索引静默截断，否则可能改变唯一性语义。
- 新增列存索引类型时，应先确认它是否真的绕过行存 KV 键长限制；若是，保持非零最小长度约定；若否，不能无条件沿用当前 `columnar != NA` 快路径。
- 建议在同目录新增独立测试文件（例如 `pkg/session/runtime/ddl_index_validation_test.rs`），并在 `pkg/session/runtime.rs` 以 `#[cfg(test)] mod ddl_index_validation_test;` 接入；不要把测试内嵌回生产文件。用公开会话 CREATE TABLE 路径覆盖 ASCII/多字节字符集、BIT/FLOAT/DECIMAL、严格与非严格模式、唯一/主键/复合索引、列存索引、未知字符集以及异常负长度。
- 若把入口复用于 ALTER/ADD INDEX，必须重新核对调用时点和失败原子性；当前实现只由 CREATE TABLE 路径调用，不能直接声称覆盖后续索引变更。
- 性能上当前对每个索引列线性扫描 `table.Columns`，复杂度约为索引键列数乘表列数。只有真实宽表热点证据出现时才值得引入名称到列的临时映射，同时必须保留大小写折叠名匹配规则。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore "pkg/session/runtime/ddl_index_validation.rs ..."` 返回目标文件全貌及 `default_mysql_type_length -> index_column_length`、`decimal_index_length -> index_column_length`、`index_column_length -> validate_create_table_index_lengths` 的文件内关系。
- RustCodeGraph `query validate_create_table_index_lengths` 找到目标入口和 `pkg/session/runtime/ddl.rs` 中同名调用表达式；`callees validate_create_table_index_lengths` 对目标定义确认 `index_column_length` 与会话 `borrow`。`callers` 未产出可用结果，随后以 `rg` 核验唯一生产引用为 `pkg/session/runtime/ddl.rs`。
- 已读 Rust/配置路径：`pkg/session/runtime/ddl_index_validation.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/admin.rs`、`pkg/session/runtime/session.rs`、`pkg/session/Cargo.toml`、`pkg/meta/model/index.rs`、`pkg/parser/types/field_type.rs`。
- 已读 Go 对照与测试：`pkg/ddl/index.go` 的 `checkIndexColumn`、`getIndexColumnLength`、`buildIndexColumns`；`pkg/ddl/db_test.go::TestBuildMaxLengthIndexWithNonRestrictedSqlMode`；`pkg/ddl/tests/serial/serial_test.go::TestChangeMaxIndexLength`。限定检索确认未找到 Rust 直接回归测试。
- 本任务只新增文档，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有十一个固定二级章节，并人工复核所有“已支持”陈述均能回指上述符号或路径。
