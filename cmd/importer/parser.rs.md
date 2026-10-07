# `cmd/importer/parser.rs`

## 文件定位

本文件属于 `astersql-cmd-importer` crate；crate 根在 [`cmd/importer/lib.rs`](lib.rs) 以 `pub mod parser` 暴露它，二进制入口 [`cmd/importer/bin_main.rs`](bin_main.rs) 经 crate 根转入 [`cmd/importer/main.rs`](main.rs)。主流程 `run_with_args` 依次调用 `newTable`、`parseTableSQL`、`parseIndexSQL`，先把配置中的建表/建索引 SQL 转成 importer 专用的轻量表视图，再加载统计信息、执行真实 DDL 并启动造数任务。因此本文件位于“命令配置输入”与“统计加载/INSERT 生成”之间，不是通用 SQL parser。

[`cmd/importer/Cargo.toml`](Cargo.toml) 将该目录声明为同时具有 library 和 `astersql-cmd-importer` binary 的独立 crate，移植元数据指向 Go 包 `cmd/importer`，并明确采用本地 stubs 避免引入完整 TiDB 重依赖。源码直接使用 `regex::Regex`，但当前清单的显式依赖仅列出 `toml`、`serde_json`；本任务按要求未运行 Cargo，故这里只记录清单事实，不声称该依赖配置已通过独立编译验证。

## 核心职责

本文件有三项职责，证据分别是 `column`、`table` 以及四个解析入口：

1. 将 `CREATE TABLE`/`CREATE INDEX` 的本地 AST（定义于 `stubs.rs`）压缩为 importer 实际消费的 `table`/`column`，而不是保留完整语法树。
2. 从列注释的 `[[key=value;...]]` 片段提取造数规则：`range`、`step`、`set`、`incremental`、`repeats`、`probability`。
3. 同步构造 `TableInfo`、有序列列表以及普通/唯一索引映射，供 [`cmd/importer/stats.rs`](stats.rs) 查直方图、[`cmd/importer/db.rs`](db.rs) 生成列值和 INSERT SQL。

支持面刻意有限：`stubs::parse_one_stmt` 只识别 importer 使用的 `CREATE TABLE` 与 `CREATE [UNIQUE] INDEX` 形式；`parseTableSQL` 和 `parseIndexSQL` 再做语句种类检查。它不是完整 TiDB parser 的替代品。

## 主要符号

- `pub struct column`：单列运行期状态。`idx` 从 1 开始；`name`、`tp` 来自 AST；`comment/min/max/incremental/set` 保存注释规则；`data: Arc<datum>` 保存带内部锁的步长、重复次数和概率状态；`hist` 可在主流程加载统计后挂接直方图。
- `column::parseRule`：解释一条恰好含两个元素的键值规则。未知键、元素数不为 2、`range` 分段数不为 1/2 时静默忽略；数值/布尔格式错误及非法概率通过 `stubs::fatal` 终止当前流程。
- `column::parseColumnComment`：只取首次 `[[` 与首次 `]]` 之间且起点早于终点的内容，先按分号、再按等号拆分并逐条交给 `parseRule`。
- `column::parseColumnOptions`：识别 `PrimaryKey`、`UniqKey`、`AutoIncrement` 为唯一性标记，提取最后遇到的 `Comment`；其他选项忽略。
- `pub struct table`：保存表名、声明顺序列向量、逗号连接的 `columnList`、普通索引 `indices`、唯一索引 `uniqIndices` 和最小 `TableInfo`。两个索引映射的值是 `Option<usize>`，因此能保留“索引点名了不存在列”的 Go `nil` 语义。
- `table::parseTableConstraint`：把表级约束折叠到两张索引映射。与 Go 版本一致，`PrimaryKey`、`Key`、`Uniq`、`UniqKey`、`UniqIndex` 进入 `uniqIndices`，只有 `Index` 进入 `indices`。
- `newTable`：构造全空表视图以及默认 `TableInfo`。
- `parseTable`：从 `CreateTableStmt` 构造 `TableInfo` 和全部列，随后处理表级约束并生成 `columnList`。
- `parse_column_into`：单列私有装配函数；先读取列选项和唯一性，再解析注释规则，最后把列推入表并登记稳定下标。
- `parseTableSQL` / `parseIndexSQL`：字符串入口；调用 `stubs::parse_one_stmt` 并验证 AST 种类。后者将空字符串视为“没有附加索引”。
- `parseIndex`：验证索引语句表名与当前表完全一致，再按 `IndexKeyType` 写入普通或唯一索引映射。
- `parse_bool_go`：只接受 Go `strconv.ParseBool` 的十二种大小写/数字形式，其他输入返回兼容风格错误文本。
- `String` / `printColumns`：提供接近 Go `fmt` 输出的调试文本；`HashMap` 遍历顺序不构成稳定输出契约。

## 执行流程

主链可由 `cmd/importer/main.rs::run_with_args` 和本文件入口复核：

1. `newTable` 创建空表。
2. `parseTableSQL` 调用 `stubs::parse_one_stmt`；仅 `StmtNode::CreateTable` 被接受，否则返回 `invalid statement`。
3. `parseTable` 复制规范化表名，按列数预分配 `columns`，调用 `stubs::build_table_info_from_ast` 并把返回的 `TableInfo.ID` 固定为 1。
4. 每个 `ColumnDef` 按声明顺序进入 `parse_column_into`：设置 1-based `idx`、列名和类型；读取列级唯一性及 comment；带着该唯一性执行 comment 规则校验；推入 `columns` 后，才把稳定的零基下标写入 `uniqIndices`。
5. 列全部存在后，`parseTableConstraint` 处理表级约束；`findCol` 找不到列时仍插入键并保存 `None`。
6. `buildColumnList` 按列声明顺序生成 `a,b,c`，这与 `db.rs::genRowData` 逐列生成 values 的顺序一一对应。
7. `parseIndexSQL` 对空字符串直接成功；非空输入必须解析成 `CreateIndexStmt`。`parseIndex` 先校验目标表名，再为每个索引列写普通或唯一映射；缺失列同样保留为 `None`。
8. 主流程随后用 `table.tblInfo` 加载统计信息、把 histogram 挂到 `table.columns`，最终将 `Arc<table>` 交给导入任务；`db.rs::genColumnData` 依据 `uniqIndices` 强制唯一列走递增路径。

## 数据与状态

`table.columns` 的物理顺序、`column.idx` 的 1-based 编号、`TableInfo.Columns[*].Offset` 的 0-based 编号和 `columnList` 的文本顺序必须保持同步。普通/唯一索引不拥有列对象，只保存 `columns` 下标；`None` 是有意保存的“已声明但未解析到列”状态，而非遗漏记录。`parseTable` 会重建 `columns`，但不会主动清空既有 `indices`/`uniqIndices`；正常主链总是传入新表，若复用同一 `table` 重复解析 SQL，调用者必须注意旧索引状态可能残留。

每个 `column.data` 是 `Arc<datum>`；`datum` 在 [`cmd/importer/data.rs`](data.rs) 内用 `Mutex<DatumInner>` 保存 `step`、`repeats/remains`、`probability` 等可变生成状态。默认值来自 `newDatum`：`step=1`、`repeats=remains=1`、`probability=100`。`parseRule(repeats=...)` 通过 `set_repeats` 同时更新 repeats 与 remains。`range` 和 `set` 在本文件保存文本，实际类型化取值由下游随机/列值生成逻辑完成。

`TableInfo` 由 `stubs::build_table_info_from_ast` 创建，含列 ID/offset 和来自列级、表级约束的索引信息；独立 `CREATE INDEX` 只更新本文件的 `indices`/`uniqIndices`，不会回写 `tblInfo.Indices`。这是当前实现边界，扩展统计加载到独立索引时需要特别处理。

## 依赖与调用关系

上游直接调用者：

- `cmd/importer/main.rs::run_with_args`：生产主链依次调用 `newTable`、`parseTableSQL`、`parseIndexSQL`。
- `cmd/importer/parser_test.rs`、`parity_test.rs`、`stats_test.rs`：独立测试和跨 Go/Rust 契约测试直接调用同一公开入口。

主要下游：

- `crate::stubs::parse_one_stmt`：产生有限的 `StmtNode` AST；`StmtNode::Text` 用于错误信息。
- `crate::stubs::build_table_info_from_ast`：从 `CreateTableStmt` 构造最小 `TableInfo`。
- `crate::data::newDatum`：为每列建立带锁的生成状态；`parseRule` 调用其 setters。
- `crate::stubs::fatal`：模拟 Go `log.Fatal`，当前 Rust stub 以 panic 实现，测试可捕获。
- `cmd/importer/db.rs::genRowData/genColumnData`：消费表名、`columnList`、列类型/规则和唯一索引映射。
- `cmd/importer/stats.rs::loadStats` 及 `main.rs` 的 histogram 绑定逻辑：消费 `tblInfo` 并更新 `column.hist`。

RustCodeGraph 的文件节点显示 `parser.rs` 被 `main.rs`、`parser_test.rs`、`parity_test.rs`、`rand.rs`、`rand_test.rs`、`stats_test.rs` 等使用；精确符号查询定位了 `parseTableSQL`、`parseIndexSQL`、`parseIndex`、`parse_one_stmt`、`build_table_info_from_ast`。图工具对这些局部 Rust 调用未输出完整 callers/callees 列表，因此入口和下游关系进一步由上述相邻源码直接核验。

## 错误处理与边界

- SQL tokenizer/parser 失败由 `Result` 原样向上传播；能解析但不是期望语句种类时返回 `Error("invalid statement - ...")`。
- `parseIndexSQL("")` 明确成功，是配置默认值依赖的兼容行为。
- `parseIndex` 在表名不一致时返回错误；`IndexKeyType::Other` 返回 unsupported 错误。
- 索引引用缺失列不是错误：映射保留键和值 `None`，调试输出显示 `<nil>`。`parser_test.rs::missing_index_column_is_retained_as_go_nil_entry` 固化了该契约。
- comment 缺少有效 `[[...]]` 时按空规则处理；未知键或格式不是恰好一个 `=` 的字段被忽略。由于实现使用 `split('=')`，值本身含 `=` 也会被忽略。
- `range` 只接受一个或两个逗号分段；更多分段静默无效。`set` 允许空元素并按 trim 后文本追加。
- `step` 必须是 `i64`，`repeats` 必须是 `u64`，`probability` 必须是 `u32` 且位于 `(0,100]`，`incremental` 必须属于 Go bool 集合；失败调用 `fatal`。唯一列的 `repeats>1` 同样 fatal，而不是返回普通 `Result`。
- `AutoIncrement` 在本文件按唯一列处理，但不会自动把 `incremental` 规则设为 true；其直接作用是登记唯一性，从而让 `db.rs::genColumnData` 强制递增生成。
- `parseTable` 的 `build_table_info_from_ast` 失败会在任何列状态写入前返回；但该函数不会对传入表做事务性回滚，未来若增加中途可失败步骤需决定是否保留部分状态。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务或外部资源。解析阶段预期在 `run_with_args` 启动 worker 前串行完成；完成后的 `table` 被包装为 `Arc<table>` 共享给导入任务。

共享表本身在 worker 阶段主要只读，但 `column.data: Arc<datum>` 具有内部 `Mutex`，生成数据时会更新 remains/当前值等状态。`column.hist` 在共享前由主流程写入。普通 `HashMap` 不提供确定遍历顺序，所以 `table::String` 中索引条目的次序不应被并发或测试代码当作稳定协议。解析函数接收 `&mut table`，Rust 借用规则阻止解析期间并发修改同一表。

## 与 Go 版本的对应关系

直接对照文件是 [`cmd/importer/parser.go`](parser.go)。类型和流程基本逐项映射：Go `column/table` 对应 Rust 同名结构；Go `parseRule`、`parseColumnComment`、`parseColumnOptions`、`parseTableConstraint`、`parseTable(SQL)`、`parseIndex(SQL)` 均有对应实现；规则键、错误文本意图、列顺序和空 index SQL 行为保持一致。

关键实现差异：

- Go 保存 `*column`，缺失索引列用 `nil`；Rust 用 `Vec<column>` 加 `Option<usize>`，避免自引用结构，同时保留 nil 可观察语义。
- Go `column` 持有反向 `*table` 并在解析列选项时立即登记唯一性；Rust 的 `parse_column_into` 用 `marked_uniq` 在 comment 校验时即时生效，push 后才登记稳定下标，保持“唯一列禁止 repeats>1”的行为而避免悬垂引用。
- Go 使用完整 TiDB parser、DDL builder、model/types；Rust 当前通过 `stubs.rs` 的有限 parser、AST、`FieldType` 和 `TableInfo` 适配。支持语法范围因此由 stubs 限定，不能仅凭 Go 能解析就推断 Rust 已支持。
- Go 的 `datum` 是指针并带同步状态；Rust 使用 `Arc<datum>` 和内部 `Mutex`。Go `table.String` 可打印 nil；Rust显式把 `None` 渲染为 `<nil>`。
- Go 的 `log.Fatal` 终止进程；当前 Rust `stubs::fatal` 使用 panic 模拟，生产边界与测试可捕获性不同，但调用点保留 fatal 而非普通错误返回。
- Go `parseTable` 调用真实 `ddl.BuildTableInfoFromAST`；Rust stub 只构造 importer 当前需要的最小字段并分配简化 ID。文档不把它描述为完整 TiDB 元数据构造。

## 扩展指南

- 新增 comment 规则：修改 `column::parseRule`，先确认 Go `cmd/importer/parser.go::parseRule` 的语义；若规则改变生成状态，同步调整独立 [`cmd/importer/parser_test.rs`](parser_test.rs) 以及直接消费者（通常为 `data.rs`/`db.rs`）的测试。不要把测试嵌入 `parser.rs`。
- 扩展 SQL/类型/约束语法：解析能力实际位于 `stubs.rs::parse_one_stmt` 及其 helpers；本文件只做 AST 到运行时状态的二次校验和装配。需同时检查 `build_table_info_from_ast`，避免 `table` 映射与 `tblInfo` 分叉。
- 新增索引类别：同步修改 `parseTableConstraint`、`parseIndex`、stubs 的枚举/parser，并明确应进入普通还是唯一映射；测试缺失列、复合索引、表名不匹配和 unsupported 类型。
- 改动列顺序或下标：必须同时维护 `column.idx`、`columns`、`columnList`、`TableInfo.Offset`、索引映射以及 `db.rs::genRowData` 的对应关系，否则生成 SQL 的列和值会错位。
- 让独立 `CREATE INDEX` 参与统计元数据：当前 `parseIndex` 不更新 `tblInfo.Indices`，需要设计一致的 ID/offset 生成策略，并补 `stats_test.rs` 回归。
- 改善错误恢复：当前 comment 数值错误是 fatal/panic；若改为 `Result` 会改变 Go 可观察契约及多个签名，不能仅在局部吞错。
- 若复用同一 `table` 多次解析：先决定是否应清空索引映射并添加回归测试；当前只重置列，不重置索引。
- 修改后应优先扩展同目录独立测试文件 `parser_test.rs`，并保留 `parity_test.rs` 中正常、边界、错误主链对照；本仓库规则禁止把 Rust 单元测试放回生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter cmd/importer` 确认目标及 Go/Rust 对照文件；`node --file cmd/importer/parser.rs` 读取完整 412 行并显示使用方；`query` 定位 `parseTableSQL`、`parseIndexSQL`、`parseIndex`、`parse_one_stmt`、`build_table_info_from_ast`、`fatal`；相邻 Rust 文件通过 `node --file` 核验。
- 生产与配置：[`cmd/importer/parser.rs`](parser.rs)、[`cmd/importer/lib.rs`](lib.rs)、[`cmd/importer/main.rs`](main.rs)、[`cmd/importer/bin_main.rs`](bin_main.rs)、[`cmd/importer/Cargo.toml`](Cargo.toml)、[`cmd/importer/stubs.rs`](stubs.rs)、[`cmd/importer/data.rs`](data.rs)、[`cmd/importer/db.rs`](db.rs)。
- Go 对照：[`cmd/importer/parser.go`](parser.go)；逐项核验结构、规则、解析顺序、索引分类、错误与空输入行为。
- 独立测试：[`cmd/importer/parser_test.rs`](parser_test.rs) 验证 set 调试格式和缺失列 nil 语义；[`cmd/importer/parity_test.rs`](parity_test.rs) 验证类型、`IF NOT EXISTS`、comment 规则、列列表、唯一索引、空索引 SQL、非法语句及表名不匹配；[`cmd/importer/stats_test.rs`](stats_test.rs) 验证 `tblInfo` 可供统计恢复使用。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证命令及结果在任务交付时记录。
