# [`pkg/ttl/sqlbuilder/sql.rs`](./sql.rs)

## 文件定位

本文件是 Rust crate `astersql-ttl-sqlbuilder` 的实际实现文件；crate 根 `pkg/ttl/sqlbuilder/lib.rs` 通过 `pub mod sql` 和 `pub use sql::*` 将这里的公开项整体导出。根 workspace 又以 `facade_ttl_sqlbuilder` 引入该 crate，并在 `pkg/lib.rs` 的 facade 模块中再导出。`pkg/ttl/ttlworker/Cargo.toml` 与 `pkg/resourcegroup/runaway/Cargo.toml` 声明了依赖，但对生产 `.rs` 的符号搜索没有找到对本文件 API 的实际调用，因此当前可确认的是“可被 workspace 使用且已有独立单元测试”，不能据此声称它已接入 Rust TTL worker 的运行主链。

文件前 511 行是被逐行注释掉的早期 Go 机械翻译草稿，不参与编译；可执行实现从 `SqlError` 开始。当前实现自带 `Datum`、`FieldType`、`Column`、`PhysicalTable` 和 `IndexInfo` 等轻量数据模型，并不直接使用 Go 版本对应的 `types.Datum`、`model.ColumnInfo` 或 `cache.PhysicalTable`。`pkg/ttl/sqlbuilder/Cargo.toml` 在普通目标上唯一直接依赖是 `chrono`；多项 AsterSQL crate 依赖仅置于 `cfg(windows)` 条件下，当前可执行代码本身没有引用它们。

## 核心职责

1. `write_datum`/`FormatSQLDatum` 按 MySQL 列类别把 `Datum` 恢复成可嵌入 SQL 的字面量，处理二进制十六进制、字符串转义、NULL、数值、时间和布尔值。
2. `SQLBuilder` 以 `sqlBuilderState` 约束 `SELECT`/`DELETE`、条件、排序、限制的写入顺序，并用 `has_expire_condition` 阻止没有 TTL 过期条件的删除语句完成构建。
3. `ScanQueryGenerator` 保存分页游标前缀栈，生成主键扫描或次级 TTL 索引扫描 SQL；首次扫描包含范围起点，后续扫描排除上一页最后一行。
4. `BuildDeleteSQL` 为指定表键集合生成带过期时间保护和行数上限的批量 `DELETE`。
5. `NewIndexScanQueryGenerator` 建立索引扫描投影、排序列和表键位置映射，使扫描结果既可按索引物理顺序续页，也能由 `TableKey` 提取删除所需表键。

这些职责只构造 SQL 文本和保存生成器内存状态，不执行 SQL、访问存储或管理事务。

## 主要符号

- `SqlError` 与 `Result<T>`：模块内的字符串错误包装和统一返回类型。
- `Datum`：SQL 值枚举，覆盖 NULL、有/无符号整数、浮点、十进制定点字符串、字节、字符串、日期时间、枚举、集合、JSON 和布尔值。
- `FieldKind`、`FieldType`：决定字面量编码；`FieldType::binary` 使字符串类值按 hex 输出。
- `Column`、`IndexInfo`、`PhysicalTable`：构建 SQL 所需的最小元数据。`PhysicalTable::new` 拒绝空表键；`validate_key_prefix` 拒绝比表键更长的键前缀；`with_index` 只保存索引，生成器仍要求调用方显式传入要使用的 `IndexInfo`。
- `ExpireTime`：同时保存 Unix 秒和扫描 worker 捕获的时区偏移。`Timestamp` 使用绝对时刻，其他时间列通过 `wall_clock` 恢复偏移后的墙上时间。
- `write_datum`/`FormatSQLDatum`：底层 Rust 风格函数及其 Go 风格公开别名。
- `sqlBuilderState`：`writeBegin -> writeSelOrDel -> writeWhere -> writeOrderBy -> writeLimit -> writeDone` 的状态枚举。
- `SQLBuilder<'a>`：借用 `PhysicalTable` 的有状态 SQL 拼接器。Rust 风格方法与 `Build`、`WriteSelect` 等 Go 风格别名同时存在。
- `ScanQueryGenerator<'a>`：保存范围、当前页大小、前缀栈、首批/耗尽标志和可选 `IndexScanPlan`。
- `IndexScanPlan`：私有派生计划，包含索引、扫描投影列、排序列与表键在投影中的偏移。
- `NewSQLBuilder`、`NewScanQueryGenerator`、`NewIndexScanQueryGenerator`：Go 命名兼容入口。
- `BuildDeleteSQL`：批量删除 SQL 的高层入口。

## 执行流程

字面量格式化从 `write_datum` 开始：Bit/Blob 类以及带 binary 标志的字符串类先由 `datum_bytes` 转成字节，再由 `write_hex` 输出 `x'...'`；普通字符串类要求字符串、枚举、集合或可解码 UTF-8 的字节，并经 `escape_sql_string` 转义；其他类型按枚举值恢复，非有限浮点被拒绝。

基础 SQL 构建由 `SQLBuilder` 驱动。`write_select` 或 `write_select_columns` 写投影、表名和可选分区并标记只读；`write_delete` 写删除头但不标记只读。条件入口先调用 `expect_condition_state`，首次条件写 `WHERE`，后续条件写 `AND`。`write_common_condition` 写比较式，`write_in_condition` 写行值列表，`write_expire_condition` 写时间边界。之后可追加 `ORDER BY` 和 `LIMIT`，最终 `build` 锁定为 `writeDone` 并检查删除安全条件。

主键扫描由 `NewScanQueryGenerator` 创建并校验起止键不长于表键。第一次 `next_sql` 没有上一页结果时，`set_stack(None)` 使用 `key_range_start`；满页续扫时从最后一行取得排序键并重建各级前缀；短页时弹出一层前缀，栈空即标记 `exhausted`。`build_sql` 对前缀的非末列写 `=`，末列首批写 `>=`、续页写 `>`；NULL 固定前缀写 `IS NULL`，跨过 NULL 边界写 `IS NOT NULL`，首批以 NULL 为包含式下界时不追加末列条件。随后依次写范围上界、TTL 条件、升序排序和 LIMIT。

索引扫描由 `NewIndexScanQueryGenerator` 建立 `IndexScanPlan`：索引第一列必须等于 TTL 时间列；Float/Set 索引列被拒绝；扫描投影先放索引列，再补充尚未出现的表键；非唯一索引的排序列还会补齐表键以保证稳定续页。`ScanColumnTypes` 返回实际投影类型，`TableKey` 按 `key_offsets` 从结果行提取表键。索引模式的范围上界只比较 TTL 时间列。

删除流程由 `BuildDeleteSQL` 负责：拒绝空行集，依次调用 `write_delete`、对全部表键写 `IN`、写过期条件、用行数写 LIMIT，最后调用 `build`。行数向 `i32` 转换失败时不会生成无界删除。

## 数据与状态

`SQLBuilder` 的核心不变量是子句状态只能单向推进；`build` 后状态为 `writeDone`，任何继续写条件、排序或限制的调用都会失败。`is_read_only` 仅由 SELECT 路径置位；所有非只读构建必须令 `has_expire_condition` 为真。构建结果通过克隆内部 `String` 返回，因此构建器仍持有原文本，但状态禁止继续修改。

`ScanQueryGenerator.stack` 的第 `i` 层保存当前排序键的 `0..=i` 前缀。短页会逐层回退，因此复合键可依次从 `(a,b,c)` 的末列边界退到 `(a,b)`、再退到 `(a)`；栈空表示整个范围耗尽。`first_build` 无论本次 `next_sql` 成功或失败都会在闭包执行后变为 false，与 Go 的 `defer` 语义对齐。`limit` 保存上一轮请求大小，用来判断调用方返回的结果是否为满页。

`ExpireTime.unix_seconds` 表示绝对时刻，`utc_offset_seconds` 只用于 DATE/DATETIME 的墙上时间恢复。偏移加法或 chrono 时间转换溢出时返回错误。所有表、列、索引和 Datum 都是拥有所有权的普通值；生成器和 builder 只以不可变引用借用 `PhysicalTable`。

## 依赖与调用关系

向下依赖方面，运行时代码只直接使用标准库格式化能力和 `chrono::DateTime::from_timestamp`。内部调用主链为：

`NewScanQueryGenerator/NewIndexScanQueryGenerator -> ScanQueryGenerator::next_sql -> set_stack + build_sql -> SQLBuilder::{write_select_columns, write_force_index, write_common_condition, write_expire_condition, write_order_by, write_limit, build} -> write_datum/escape_sql_string/write_hex`。

删除链为：

`BuildDeleteSQL -> SQLBuilder::{write_delete, write_in_condition, write_expire_condition, write_limit, build}`。

向上关系方面，`lib.rs` 和 workspace facade 公开再导出这些符号；`pkg/ttl/sqlbuilder/sql_test.rs` 直接调用 builder、两类扫描生成器和删除入口。RustCodeGraph 将本文件列为若干 TTL session/worker 文件的“used by”对象，但针对生产 Rust 源的精确名称搜索没有发现这些 API 的调用；`pkg/ttl/ttlworker/scan.rs` 和 `del.rs` 当前使用其自身 `crate::session::{Datum, PhysicalTable, ...}` 并自行构造参数化 SQL。因此，本文件与现有 Rust worker 的真实桥接仍未由代码证据确认。

## 错误处理与边界

- `PhysicalTable::new` 拒绝没有键列的 TTL 表；键范围和分页键超过可排序列数也会失败。
- builder 在初始状态直接 `build`、重复写 SELECT/DELETE、在 ORDER BY/LIMIT 后写条件、或 build 后继续写入，均返回含当前状态的 `SqlError`。
- 删除未写过期条件时 `build` 固定返回 `expire condition not write`；`BuildDeleteSQL` 还拒绝空行集并检查 LIMIT 的 `usize -> i32` 转换。
- `write_data_point` 要求列数和值数完全相同。字符串列接收不兼容 Datum、二进制类型无法转换成字节、非有限浮点、时间越界都会返回错误。
- 标识符由 `write_name` 加反引号且将内部反引号加倍；字符串处理 NUL、退格、换行、回车、Ctrl-Z、单双引号和反斜线。比较运算符是调用方提供的原始字符串，函数本身不验证白名单，因此新增外部调用点不能把不可信输入直接作为 operator。
- `ScanColumnTypes`、`TableKey` 和 `order_key` 使用切片索引，默认调用方提供与生成器投影一致的结果行；过短行会 panic，而不是返回 `SqlError`。
- `write_limit` 本身不拒绝零或负数；高层 `next_sql` 会拒绝非正 limit，但直接使用 `SQLBuilder` 的调用方需要自行保证有效值。
- 与 Go 当前实现相比，Rust `write_select_columns` 没有显式拒绝空投影；主键扫描也没有写 Go 的 `USE INDEX ()`。这些是已核实的当前差异，不应在文档中当作等价行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、连接或文件句柄。`SQLBuilder` 和 `ScanQueryGenerator` 都通过 `&mut self` 串行推进内部状态；其生命周期参数把对象限制在所借用 `PhysicalTable` 的生命周期内。类型没有声明进程级全局状态，多个实例之间没有共享游标或 SQL 缓冲区。

调用方应把一个 `ScanQueryGenerator` 视为单个扫描范围的有状态会话：按页顺序调用 `next_sql`，把上一条 SQL 的完整结果传回，并在 `is_exhausted` 为真后停止。耗尽后再次请求会得到错误；短页触发的前缀回退可能先返回空 SQL并标记耗尽。SQL 的执行、结果行类型校验、事务边界和重试均属于上层职责，本文件没有资源清理动作。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ttl/sqlbuilder/sql.go`，回归测试为 `pkg/ttl/sqlbuilder/sql_test.go`。Rust 保留了 Go 的主要结构：Datum 恢复、builder 状态机、DELETE 过期条件防护、`[start,end)` 扫描、首批 `>=`/续页 `>`、复合键前缀退栈、NULL 排序边界、索引投影与表键提取、批量删除 IN 条件。Rust 独立测试 `sql_test.rs` 覆盖了状态与删除安全、字符串转义、复合键分页、空 continuation 与 nil continuation 的区别、时间类型/时区偏移、索引物理顺序和表键投影。

当前差异包括：

- Go 使用完整 TiDB `types.Datum`、`model.ColumnInfo`、`cache.PhysicalTable` 和 `TTLIndexScanPlan`；Rust 使用本地简化模型，索引合法性只在本文件内检查首列、范围长度和部分不支持类型。
- Go 的 PK 扫描强制写 `USE INDEX ()`，Rust PK 路径未写该 hint；Go `writeSelectColumns` 拒绝空列，Rust 没有对应检查。
- Go 的索引计划由 `PhysicalTable.BuildTTLIndexScanPlan` 统一派生；Rust 从调用方传入的 `IndexInfo` 现场组装，`PhysicalTable.indexes` 不参与查找或一致性验证。
- Go 的通用 AST restore 负责非字符串类型；Rust 逐个匹配本地 Datum，Decimal/JSON/日期时间等值主要依赖调用方传入合法文本。
- Rust 用 `ExpireTime` 显式携带固定 UTC offset，以复现 worker 捕获的墙上时间；Go 直接接收已带 Location 的 `time.Time`。
- Rust API 同时保留 snake_case 与 Go 风格别名，但并非所有私有 Go helper 都有同名兼容方法。

因此，“移植语义接近”不能替代集成等价性结论；尤其查询 hint、真实 TiDB 类型转换和 cache 索引计划仍需由未来接线及集成测试证明。

## 扩展指南

新增 Datum/列类型时，应同步修改 `Datum`、`FieldKind`、`datum_bytes` 和 `write_datum`，并在独立的 `pkg/ttl/sqlbuilder/sql_test.rs` 增加普通、NULL、非法值、binary 与转义用例；不要把测试嵌入本生产文件。若类型会用于索引分页，还要评估全序关系和 NULL 顺序，并更新 `NewIndexScanQueryGenerator` 的类型限制。

新增 SQL 子句时，应先明确它位于 WHERE、ORDER BY、LIMIT 的哪一侧，再扩展 `sqlBuilderState` 与对应合法转移；DELETE 的 `has_expire_condition` 防线必须保留。任何接受原始 SQL 片段的新 API 都应优先改为枚举或受控值，避免扩大 operator 直写带来的注入面。

调整扫描分页时，应同时覆盖：空起点、显式空 continuation、满页、短页、复合键逐层回退、NULL frontier、唯一/非唯一索引、表键已在/未在索引投影、范围上界和耗尽后调用。对应 Rust 测试仍放在 `sql_test.rs`，并用 `sql_test.go` 的 `TestScanQueryGenerator`、`TestIndexScanQueryGenerator` 作为语义基准。

若要接入现有 Rust TTL worker，最关键的不是复制 SQL 文本，而是先设计 `crate::session::Datum/PhysicalTable` 与本 crate 本地模型的单一权威边界，统一参数化 SQL 与字面量 SQL 的安全策略，并补生产调用边和 worker 集成测试。还应决定是否补齐 Go 的 `USE INDEX ()`、空投影校验及 cache 统一索引计划；这些兼容性/性能选择不应由本文件静默推断。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/ttl/sqlbuilder`：确认 crate 下 `lib.rs`、`sql.rs`、独立 `sql_test.rs` 及 Go 对照文件。
- RustCodeGraph `node --file pkg/ttl/sqlbuilder/sql.rs --offset 1 --limit 500` 与 `--offset 492 --limit 858`：核对完整 1,350 行源文件、注释草稿边界、全部可执行类型/函数和文件级引用摘要。
- RustCodeGraph `query NewIndexScanQueryGenerator`、`query BuildDeleteSQL`、`query ScanQueryGenerator`：区分 Go/Rust 同名符号；对 Rust 函数执行 `callers`/`callees` 未返回静态调用边，因此又以生产 `.rs` 精确名称搜索核验接线现状。
- RustCodeGraph `node --file pkg/ttl/sqlbuilder/sql.go --offset 1 --limit 700`：核对 Go builder、扫描生成器、索引路径、NULL 边界和删除逻辑。
- 已读路径：`pkg/ttl/sqlbuilder/Cargo.toml`、`pkg/ttl/sqlbuilder/lib.rs`、`pkg/ttl/sqlbuilder/sql_test.rs`、`pkg/ttl/sqlbuilder/sql_test.go`，并搜索了 `pkg/ttl/ttlworker/scan.rs`、`pkg/ttl/ttlworker/del.rs`、`pkg/ttl/session/session.rs` 的调用关系；目标目录没有 `doc.go`。
- 独立 Rust 测试的直接证据：`sql_builder_matches_go_state_and_delete_safety_contracts`、`composite_scan_generator_matches_go_prefix_stack_pagination`、`build_delete_sql_matches_go_composite_key_contract`、`expiration_predicate_distinguishes_timestamp_from_wall_clock_types`、`captured_offset_is_shared_by_select_and_delete_expiration`、`index_scan_generator_uses_physical_order_and_projects_table_key`。
- Go 测试基准：`TestFormatSQLDatum`、`TestSQLBuilder`、`TestExpireConditionPreservesTemporalSemanticsInUTCSession`、`TestScanQueryGenerator`、`TestBuildDeleteSQL`、`TestIndexScanQueryGenerator`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构验证要求文档存在且恰有十一个固定二级标题。
