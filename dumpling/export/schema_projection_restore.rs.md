# `dumpling/export/schema_projection_restore.rs`

## 文件定位

本文件属于 `astersql-dumpling-export` library crate；crate 清单位于 [`Cargo.toml`](Cargo.toml)，其中 `schema-parser`、`schema-format`、`schema-mysql` 和 `regex` 是本链路的直接依赖。模块入口 [`schema_projection.rs`](schema_projection.rs) 通过 `#[path = "schema_projection_restore.rs"] mod restore;` 把它声明为私有子模块，并仅由 `restore_projected_schema(&ast::CreateTableStmt)` 包装其 `pub(super) fn restore`。因此它不是通用 SQL formatter，也不是 crate 的公开 API。

它位于 Dumpling 列投影的后半段：[`dump.rs`](dump.rs) 的 `prepareColumnProjection` 先取得 `SHOW CREATE TABLE`，由 [`schema_projection.rs`](schema_projection.rs) 解析并基于选中列修改 `CreateTableStmt`，只有表确实被过滤时才调用 `restore_projected_schema`。本文件把这棵已经裁剪和校验过的 AST 恢复成新的 `CREATE TABLE` 文本，随后文本缓存到 `columnProjection.schemaSQL`，供 `dumpTableMeta` 等元数据输出路径消费。未过滤表直接保留服务器返回的原始 SQL，不经过本文件。

文件顶部注释明确了边界：列选择、依赖判定和保留/删除决策由 AST 处理；恢复阶段不会通过删除原始 SQL 片段实现投影，也不会在失败时退回未修改的建表语句。这一约束避免被删列因 formatter 不支持而重新泄漏到输出 schema。

## 核心职责

1. 从 `ast::CreateTableStmt` 逐层恢复表名、列、列选项、约束、表选项、分区、预切分和 `CREATE TABLE ... SELECT`，形成一条完整 SQL。
2. 为列名、表名、字符串及 TiDB 特殊注释提供一致转义；对 TTL、AUTO_RANDOM、clustered index、placement 等扩展生成与 TiDB parser/Go restore 兼容的文本。
3. 对表达式复用 `ast::sql_restore::restore_expr` 的结构恢复，同时通过“唯一占位 literal -> 一次性替换”保留字符串、字节、bit、hex 和 introduced value 的类型及转义语义。
4. 对 parser AST 中缺少恢复所需子节点、非法枚举值或明确不支持的选项返回错误，禁止静默丢弃 AST 信息。
5. 与 Go 的 `CreateTableStmt.Restore` 保持输出行为，而不是复刻列投影算法；投影算法及外键父表校验属于相邻的 [`schema_projection.rs`](schema_projection.rs)。

本文件没有模块级常量、公开结构体、公开 trait、条件编译项或内嵌测试。除 `restore` 为父模块可见外，其余函数和函数内 visitor 都是私有实现细节。

## 主要符号

- `name(&str) -> String`：用反引号包围标识符，并把内部反引号加倍；`column_name` 与 `table_name` 在此基础上恢复限定名。
- `string(&str) -> String`：用单引号包围文本，并先转义反斜杠、再把单引号加倍。列注释、表字符串选项、TTL interval 等共用该规则。
- `special(feature, body)`：生成 `/*T! body */` 或 `/*T![feature] body */`，用于 parser 可识别的 TiDB 特殊语法。
- `literal(&ast::ExprNode) -> Option<String>`：只处理完整 literal AST 节点。字符串保留 charset introducer，bytes 按有损 UTF-8 转成 quoted string，bit/hex 按字节恢复，其他 `ValueDatum` 使用 `text()`；`IntroducedValue` 显式输出大写 charset 前缀。
- `expression(&ast::ExprNode) -> Result<String>`：表达式恢复适配器。局部 `Source` visitor 收集节点文本与 literal 文本以避开占位符冲突；局部 `Literals` visitor 克隆表达式并把 literal 替换成唯一 UTF8MB4 字符串；调用 parser renderer 后再用一条正则同时还原原 literal。
- `optional_expression`、`expressions`：分别要求可选表达式必须存在，以及批量恢复表达式列表；前者是多个 AST 完整性错误的统一入口。
- `index_part`：恢复表达式索引项或列索引项，支持 prefix length 与 `DESC`；两者都缺失时报错。
- `reference`：恢复外键目标表/索引项、`MATCH`、`ON DELETE` 和 `ON UPDATE` 行为。
- `column_option`：覆盖 `ColumnOptionType` 各分支，包括主键、默认值、生成列、inline reference、check、auto-random 和属性；`Fulltext` 明确报 parser 当前忽略该类型。
- `split_option`、`index_option`、`constraint`：恢复 split range/value lists、索引属性及表级约束。`index_option` 先克隆并清空需自定义恢复的字段，再调用 parser 自带 `restore_with_special_comments(true)`，防止字段重复输出。
- `time_unit`、`table_option`：枚举时间单位与表选项。`table_option` 覆盖数值、字符串、DEFAULT、特殊注释、TTL、placement、storage 等不同编码，并对未覆盖 placement 变体返回错误。
- `partition_method`、`partition_definition`、`partition_options`：分层恢复分区/子分区方法、定义、interval、options 与 update indexes。
- `pub(super) fn restore(table: &ast::CreateTableStmt) -> Result<String>`：唯一父模块入口，按 SQL 语法顺序组装整条语句，并把字段类型 formatter、UTF-8 转换和子恢复器错误统一传播。

## 执行流程

完整应用链路如下：

1. [`dump.rs`](dump.rs) 的 `prepareColumnProjection` 只在 `NoSchemas == false` 且至少一张表实际过滤列时分析 schema；包含 view 时在恢复前直接拒绝。
2. `schema_projection::new_schema_parser` 按 session `sql_mode` 配置 parser。每张基本表的 `SHOW CREATE TABLE` 先经 `parse_table_schema`，过滤表再经 `build_projected_table_schema/project_table_schema` 删除未保留列和依赖它们的可整体删除约束，并验证 auto column、partition、TTL 等不变量。
3. 过滤表调用 `restore_projected_schema`，后者进入本文件 `restore`；包装层给任意底层错误增加 `failed to restore projected CREATE TABLE` 上下文，`dump.rs` 再增加库表名上下文。
4. `restore` 先输出 temporary 类型、`IF NOT EXISTS`、目标表及可选 `LIKE`。列类型由 `schema_format::RestoreCtx` 恢复，flags 为 `DefaultRestoreFlags | RestoreTiDBSpecialComment | RestoreStringEscapeBackslash`；随后逐项追加 `column_option`。
5. 表级约束通过 `constraint` 追加到同一个 definitions 列表；definitions 以逗号连接。再顺序恢复 table options、partition options、split index、可选 SELECT 及 global temporary table 的 `ON COMMIT` 行为。
6. 表达式路径先调用 `literal` 识别需精确保真的 AST literal。`Source` 汇总原始节点信息，`Literals::Enter` 生成不与原 AST 文本冲突且按序唯一的 token，并以 `_UTF8MB4'token'` 形式记录 renderer 预期输出。
7. `ast::sql_restore::restore_expr` 恢复已替换 literal 的表达式结构；所有 token 经 `regex::escape` 合并为一个正则并在一次 `replace_all` 中还原。一次性替换保证某个原 literal 即便包含另一 token 的文本，也不会被后续级联替换。
8. 成功字符串写入 `projection.schemaSQL`。所有表 AST 建好后，`dump.rs` 才执行跨表外键父表验证；该顺序防止 `HashMap` 迭代顺序把尚未建好的父表误判为 dump 外部表。

## 数据与状态

本文件的权威输入是借用的 `&ast::CreateTableStmt`。它不修改输入 AST；除 `index_option` 克隆一个 `IndexOption` 作为 parser formatter 的临时基值、`expression` 克隆/改写临时表达式外，所有函数只读取节点并构造局部 `String`/`Vec`。输出是新分配的单条 SQL 字符串，没有模块级缓存或隐式全局状态。

`restore` 中的 `definitions` 按 AST 中 `Cols` 后接 `Constraints` 的顺序生成；列内 options、table options、partition definitions 和 split indexes 同样保留各自向量顺序。它不重新执行列依赖计算，也不重新排序 schema 元素。`Vec::with_capacity(table.Cols.len() + table.Constraints.len())` 只减少 definitions 扩容，不改变语义。

表达式恢复的关键短生命周期状态是 `Literals { source, replacements }`。`source` 只用于检测 `__astersql_projection_literal_<n>__` 是否已经出现在原 AST 可见文本中；碰撞时追加下划线直到唯一。`replacements` 保存 renderer 输出 token 与原 literal SQL 的一一对应关系。正则只在存在替换项时构造，因此不会生成空 pattern；查找后的 `unwrap` 依赖“pattern 完全由该 vector 的 escaped token 组成”这一局部不变量。

字段类型不是由本文件手写：每个 `column.Tp` 写入局部 `Vec<u8>`，再要求它是有效 UTF-8。ENUM/SET 元素等类型内部字符串因此由 `schema-format` 的 backslash flag 管理；表达式 literal 则走本文件的独立保真路径。

## 依赖与调用关系

- 直接上游：[`schema_projection.rs`](schema_projection.rs) 的 `restore_projected_schema` 是唯一源码调用点，调用 `restore::restore` 并补充恢复阶段上下文。
- 应用上游：[`dump.rs`](dump.rs) 的 `prepareColumnProjection` 在 AST 投影成功后调用 `restore_projected_schema`；恢复文本存入 `Config.columnProjection[(db, table)].schemaSQL`。同文件的元数据导出随后优先消费该缓存。
- 同级职责边界：[`schema_projection.rs`](schema_projection.rs) 负责 parser 配置、列/约束过滤、generated column 依赖、auto increment/random、partition/TTL 和外键父表验证；本文件只恢复该 AST，不决定哪些节点保留。
- AST/formatter 下游：`schema-parser` 提供 `ast::CreateTableStmt` 及所有枚举/visitor，`ast::sql_restore::{restore_expr, restore_node}` 恢复通用表达式和 SELECT；`schema-format` 恢复列类型；`schema-mysql` 只由相邻 parser 配置链使用，不由本文件直接调用。
- 其他下游：`regex` 用于对多个 literal token 进行一次性安全替换；父模块 `super::*` 提供 crate 的 `Result` 与 `errors_new`。
- crate 接线：[`lib.rs`](lib.rs) 声明私有 `mod schema_projection`，测试构建时另以独立 [`schema_projection_test.rs`](schema_projection_test.rs) 模块覆盖投影与恢复。`Cargo.toml` 没有为本文件设置 feature gate。

RustCodeGraph 已完整索引目标文件并报告它含 46 个符号；文件级关系显示它处于 `dumpling/export` 的配置、dump 与测试图中。精确 `callers/callees` 命令未返回符号边，因此上述函数级接线以 `schema_projection.rs` 的唯一 `restore::restore(table)` 调用和 `dump.rs` 的明确调用点补证，而非从文件“used by”列表推断。

## 错误处理与边界

- AST 必需子节点缺失会返回可诊断错误：默认/注释/on-update/generated/check 表达式、inline foreign key reference、索引列、TTL column/value/unit 等均不会被空字符串替代。
- `column_option(CO::Fulltext)` 明确报 `TiDB Parser ignore the ColumnOptionFulltext type now`；`table_option` 的兜底分支对未支持 variant 返回 `invalid TableOption`。这保证新 AST variant 不会在投影 schema 中静默消失。
- `RowFormat` 把 `UintValue - 1` 映射到固定列表；0 或超范围值返回错误。相反，`time_unit(Invalid)` 返回空字符串，但要求单位的 TTL 路径先检查 `Option` 存在，partition interval 则有意识地允许 invalid 表示“不输出单位”。
- 字段类型 `Restore` 错误和其字节不是 UTF-8 都转成 crate error；通用 expression/SELECT renderer 的字符串错误也通过 `errors_new` 传播。不会回退到原始 CREATE TABLE。
- `expression` 中正则编译使用 `expect("escaped literal tokens")`：pattern 仅由 `regex::escape` 处理后的非空 token 构成，按当前构造不可失败。replacement 查询的 `unwrap` 也由同一 vector 生成 pattern 的不变量支撑；修改 token 生成或 pattern 构造时必须保持这一证明。
- `literal` 对 bytes 使用 `String::from_utf8_lossy`，非法 UTF-8 会变为替换字符。这是当前实现的明确边界；若需要字节级无损输出，应新增与 parser AST/Go restore 对齐的测试后调整，不能在文档中声称已经无损。
- `ColumnOptionType::None` 恢复为空字符串，但 `restore` 仍会先加入一个空格；这是当前 AST/formatter 假设的结果。正常 parser 产物不应依赖 `None` 输出有效语法。
- 分区 `In` 的空 rows，或唯一 `DefaultValue/NamedDefault`，恢复为 `DEFAULT`；多值行只有在行宽大于 1 时额外包一层括号。外键 action 的 `None` 不输出子句。
- 本文件不会验证删除列依赖、父表索引、view、SQL mode 或服务器兼容性；这些边界分别由 `schema_projection.rs`、`dump.rs` 和目标 TiDB/MySQL 解析器负责。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、连接、文件句柄或后台资源。所有恢复工作在调用 `prepareColumnProjection` 的同步控制流内完成。输入 AST 只读借用，局部 byte buffer、字符串、克隆 AST、visitor、正则与替换表在函数返回时释放。

函数本身没有共享可变状态，因此并发调用不同或相同 AST 时不会在本模块内相互影响；实际是否并行由上游 Dumpling 调度决定。每次非空 literal 集合都会编译一次正则，时间和内存随表达式节点及 literal 数量增长；大型默认表达式、复杂 partition 或大量 table options 的资源风险主要是递归 AST 遍历、字符串拼接和临时克隆，而不是锁竞争。

`schemaSQL` 的缓存生命周期归 [`dump.rs`](dump.rs) / `Config.columnProjection` 管理。本文件只返回值，不负责缓存一致性或失效。跨表外键验证在所有 schema 构建后执行，但那是上游顺序约束，并非本文件持有跨表状态。

## 与 Go 版本的对应关系

直接 Go 对照是 [`schema_projection.go`](schema_projection.go) 的 `restoreProjectedSchema`：Go 创建 `format.RestoreCtx`，使用 `DefaultRestoreFlags | RestoreTiDBSpecialComment`，然后调用 `createTable.Restore`。Rust 的父层函数保留相同的“输入投影后 CreateTable AST、输出 SQL、失败加上下文”契约；本文件则是在 Rust parser 尚不能完全以单一 `CreateTableStmt.Restore` 覆盖所需行为时，对 Go restore 语义的专用展开。

两者职责一致但实现形态不同。Go 委托 AST 节点各自的 `Restore`；Rust 对 create table 层级显式分派，对列类型继续委托 `schema-format`，对 expression/SELECT 继续委托 `ast::sql_restore`。Rust 额外加入 typed literal token 适配器，因为通用 `ValueExpr` renderer 会按文本重新分类字符串；这保证诸如字符串 `"123"`、`"NULL"` 不会被误写为数字或 NULL。Rust 也显式启用 `RestoreStringEscapeBackslash` 恢复字段类型中的 ENUM/SET 元素。

TiDB 特殊语法的目标与 Go `RestoreTiDBSpecialComment` 一致：TTL、AUTO_RANDOM、AUTO_ID_CACHE、clustered index、placement 等用 `/*T![feature] ... */` 或 `/*T! ... */` 保存。Rust 独立测试 [`schema_projection_test.rs`](schema_projection_test.rs) 的 `projected_schema_restores_original_go_positive_outputs` 核对 partition、TTL、function default、外键 action 和 inline reference；`projected_schema_restoration_preserves_typed_literals_and_special_comments`、`projected_schema_enum_and_set_elements_preserve_backslashes`、`projected_schema_placement_policy_preserves_identifier` 补充 Rust renderer 的保真回归。

Go 测试 [`schema_projection_test.go`](schema_projection_test.go) 通过 `generateProjectedSchemaForTest` 调用 Go `restoreProjectedSchema` 并重新 parse 输出，覆盖列/约束投影及外键路径。Rust `project()` helper 同样执行 parse -> project -> restore -> reparse，并比较恢复前后保留列名；[`dump_test.rs`](dump_test.rs) 进一步验证集成路径会缓存过滤后的 schema、排除 secret 列、保留 generated/index 依赖且不会再次查询 SHOW CREATE TABLE。

## 扩展指南

- 新增 `ColumnOptionType`、`ConstraintType`、`TableOptionType`、partition clause 或 reference action 时，先更新相应 exhaustive match (`column_option`、`constraint`、`table_option`、`partition_*`、`reference`)，再与 Go 对应 AST `Restore` 输出核对。不能通过默认空字符串让新节点消失。
- 扩展表达式或 literal 类型时，优先保持结构由 `ast::sql_restore::restore_expr` 负责，只在 `literal` 中处理通用 renderer 无法保真的完整 literal 节点。必须验证 token 不与源文本碰撞、不会级联替换，并覆盖 restore 后重新 parse 的 AST 等价性。
- 修改标识符/字符串转义时，应同时审查 `name`、`string`、字段类型 restore flags、charset introducer 及 SQL mode 行为；重点风险是 backslash、单引号、反引号、非 UTF-8 bytes 和看似数字/NULL 的字符串。
- 新增 TiDB 特殊选项时，应使用正确 feature tag，并对照 Go `RestoreTiDBSpecialComment`；错误 tag 会让下游兼容 parser 把语法当普通注释忽略。placement 等标识符值与普通字符串值必须区分。
- 调整 definitions、options、partition 或 split 的顺序/空格时，要用 parser 重新解析输出，并检查 Go 正向样例而非只比字符串。性能上避免为每个小节点重复编译正则或对整条 SQL 做多轮全局替换。
- 错误必须继续沿 `Result` 传播并保留 `restore_projected_schema` 与 `dump.rs` 的两层上下文；不要在恢复失败时返回原始 schema，因为它含有被过滤列。
- Rust 测试必须继续放在独立 [`schema_projection_test.rs`](schema_projection_test.rs)，集成接线放在 [`dump_test.rs`](dump_test.rs)，不能内嵌到本生产文件。Go 对照变化同步检查 [`schema_projection.go`](schema_projection.go) 与 [`schema_projection_test.go`](schema_projection_test.go)。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter dumpling/export/schema_projection_restore.rs` 确认目标已索引；`explore`/`node --file ... --offset 1 --limit 1200` 返回完整 987 行和 46 个符号。对 `restore` 的精确 `callers/callees` 查询未产出边，故调用关系改由源码精确引用补证并明确该限制。
- 目标与 crate 接线：[`schema_projection_restore.rs`](schema_projection_restore.rs)、[`schema_projection.rs`](schema_projection.rs) 的 `mod restore` / `restore_projected_schema`、[`lib.rs`](lib.rs) 的模块声明、[`Cargo.toml`](Cargo.toml) 的 parser/format/regex 依赖。
- 应用链路：[`dump.rs`](dump.rs) 的 `prepareColumnProjection`，尤其是 `SHOW CREATE TABLE -> build/parse schema -> restore_projected_schema -> projection.schemaSQL` 及恢复后外键验证顺序。
- Rust 独立测试：[`schema_projection_test.rs`](schema_projection_test.rs) 的 `project()` 重解析检查、`projected_schema_restores_original_go_positive_outputs`、`projected_schema_restoration_preserves_typed_literals_and_special_comments`、`projected_schema_sql_mode_is_formatted_and_applied`、`projected_schema_enum_and_set_elements_preserve_backslashes`、`projected_schema_placement_policy_preserves_identifier`；[`dump_test.rs`](dump_test.rs) 的 column projection schema/cache/generated-index 集成测试。
- Go 对照：[`schema_projection.go`](schema_projection.go) 的 `restoreProjectedSchema`、[`schema_projection_test.go`](schema_projection_test.go) 的 `generateProjectedSchemaForTest` 与相关投影用例、[`dump.go`](dump.go) 的 `prepareColumnProjection`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付验证使用任务指定的结构命令确认文档存在且恰有 11 个固定二级标题，并人工检查唯一生产物、相对链接、真实符号、错误边界、无源码整段复制及独立测试落点。
