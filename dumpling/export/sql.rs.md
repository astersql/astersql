# `dumpling/export/sql.rs`

## 文件定位

[`sql.rs`](./sql.rs) 是 `astersql-dumpling-export` crate 的 SQL 辅助层。crate 入口 [`lib.rs`](./lib.rs) 通过 `include!("sql.rs")` 将它并入 crate 根，而不是建立独立的 `sql` 模块；因此文件里的 `pub` 符号位于 crate 根，并直接共享 `lib.rs` 引入的 `HashMap`、`Field`、`tcontext` 以及 [`stubs.rs`](./stubs.rs) 提供的 `DB`、`Conn`、`BaseConn`、`Rows`、`Config`、`TableMeta` 等迁移期类型。

它位于 Dumpling 导出流程的“数据库探测与 SQL 规划”层：向服务器查询库表、DDL、索引、分区、版本和集群信息，构造整表或分块导出的 `SELECT`，并生成锁表、快照、TiDB region 采样等语句。它不负责把数据写入文件；已接线的主要上游是 [`dump.rs`](./dump.rs)，例如 `prepareTableListToDumpInner -> ListAllDatabasesTables`、`dumpDatabases -> ShowCreateDatabase/ShowCreatePlacementPolicy`、`dumpWholeTableDirectly -> SelectAllFromTable` 和 `dumpSQL -> detectEstimateRows`。

[`Cargo.toml`](./Cargo.toml) 将本目录声明为库 crate（`[lib] path = "lib.rs"`，porting 元数据对应 Go 包 `dumpling/export`）。本文件没有条件编译项，也没有自定义 `struct`、`trait` 或 `impl`；其一个枚举、一个常量和函数集合都依赖 crate 级单包视图。

## 核心职责

1. **读取可导出对象与 DDL。** `ShowDatabases`、`ShowTables`、`ListAllDatabasesTables`、`ListAllPlacementPolicyNames`、`GetPartitionNames` 和 `CheckIfSeqExists` 枚举数据库对象；`ShowCreateDatabase`、`ShowCreateTable`、`ShowCreatePlacementPolicy`、`ShowCreateView`、`ShowCreateSequence` 取得或拼装恢复用 DDL。
2. **构造数据导出查询。** `SelectAllFromTable`、`buildSelectQuery`、`buildSelectField`、`buildOrderByClause`、`buildWhereCondition` 和 `buildWhereClauses` 决定列投影、分区、用户过滤条件、稳定排序及字典序 chunk 边界，最终产生延迟执行的 `TableDataIR`。
3. **选择切分依据与估算规模。** `SelectTiDBRowID`、`pickupPossibleField`、`getNumericIndex` 优先选择隐式 rowid、数值主键或数值索引；`estimateCount`/`detectEstimateRows` 解析不同数据库的 `EXPLAIN` 行数列；`GetSuitableRows` 以 128 MiB 为目标估算每文件行数。
4. **提供一致性与集群探测语句。** `FlushTableWithReadLock`、`UnlockTables`、`buildLockTablesSQL`、`createConnWithConsistency`、`getSnapshot`、`ShowMasterStatus`、`parseSnapshotToTSO` 处理锁与快照；`GetPdAddrs`、`GetTiDBDDLIDs`、`CheckTiDBWithTiKV`、`CheckTiDBEnableTableLock` 读取 TiDB 集群能力。
5. **生成 TiDB region/table-sample SQL。** `buildTiDBTableSampleQuery`、`buildTableSampleQueries`、`buildRegionQueriesWithoutPartition`、`buildRegionQueriesWithPartitions`、`buildVersion3RegionQueries` 只生成查询文本。RustCodeGraph 未发现它们连接到当前 Rust 生产主链；当前直接证据主要来自 [`sql_test.rs`](./sql_test.rs)，不能把这些 helper 描述为已经执行并发切分。

## 主要符号

### 类型与常量

- `listTableType`：与 Go 枚举值对齐的三种表枚举策略，固定为 information schema `0`、`SHOW FULL TABLES` `1`、`SHOW TABLE STATUS` `2`。顺序虽在 Rust 源码中不是按数值排列，但显式判别值维持兼容。
- `orderByTiDBRowID`：隐式 TiDB rowid 的稳定排序子句 `ORDER BY \`_tidb_rowid\``。

### 元数据与 DDL 函数

- `ShowDatabases`、`ShowTables`、`SelectVersion`：执行单一探测查询并把首列转为字符串。
- `ShowCreateDatabase`、`ShowCreateTable`、`ShowCreatePlacementPolicy`：通过 `BaseConn::QuerySQLWithColumns` 按结果列名取 DDL；结果为空时分别返回明确错误。
- `ShowCreateView`：先用 `SHOW FIELDS` 生成 MyISAM 占位表，再用 `SHOW CREATE VIEW` 生成先删表/视图、切换 charset、创建视图、恢复 charset 的脚本。
- `ShowCreateSequence`：读取 `CREATE SEQUENCE`，TiDB 再查 `NEXT_GLOBAL_ROW_ID`、MariaDB 再查 `NEXT_NOT_CACHED_VALUE`，追加 `SELECT SETVAL`。
- `ListAllDatabasesTables`：逐库执行所选枚举查询，构造 `DatabaseTables`，并按 `TableType` 过滤；information-schema 路径保留平均行宽，另外两条路径将其置零。
- `GetSpecifiedColumnValuesAndClose`/`GetSpecifiedColumnValueAndClose`：按大小写不敏感的列名投影 `Rows`，随后关闭结果集。
- `getWritableColumnNames`/`buildSelectField`：过滤虚拟生成列及默认不导出的存储生成列；发现生成列或 invisible 列时强制显式字段表，`complete_insert` 也会强制显式字段表。
- `GetPrimaryKeyColumns`、`GetPartitionNames`、`GetCharsetAndDefaultCollation`、`CheckIfSeqExists`：分别读取主键列、分区名、字符集默认 collation 和序列存在性。

### SELECT、边界与切分函数

- `SelectAllFromTable`：组合全局 `Config::Where` 与 `buildSelectQuery`，返回 `newTableData(..., is_sql=false)` 创建的 `Box<dyn TableDataIR>`。
- `buildSelectQuery`：按 `SELECT <fields> FROM <db>.<table> [PARTITION] [WHERE] [ORDER BY]` 拼接；空字段用 `''` 保持 SQL 合法。
- `buildWhereCondition`：把用户条件和 chunk 条件组合为 `WHERE (<user>) AND (<chunk>)`；只有一侧时不加双层括号，返回值保留尾随空格。
- `buildWhereClauses`：为有序 handle 边界生成 `(-∞, first)`、若干 `[low, up)` 和 `[last, +∞)`；内部由 `buildCompareClause`、`getCommonLength`、`buildBetweenClause` 展开多列字典序逻辑，相同相邻边界生成 `false`。
- `buildOrderByClauseString`/`buildOrderByClause`：反引号包裹列名；只有 `SortByPk` 开启时才使用隐式 rowid 或显式主键。
- `pickupPossibleField`/`getNumericIndex`：隐式 `_tidb_rowid` 最优先；否则只看索引首列和整数列，优先 `PRIMARY`、其次 unique、最后 cardinality 最大的普通索引。
- `estimateCount`/`detectEstimateRows`：执行 `EXPLAIN` 并从 `rows`、`estRows`、`count` 候选列中读取首个可解析浮点数，失败降级为零。

### 一致性、锁与 TiDB 辅助函数

- `SetCharset`/`RestoreCharset`：向字符串写入保存、切换和恢复会话 charset/collation 的 SQL。
- `buildLockTablesSQL`、`FlushTableWithReadLock`、`UnlockTables`：只锁未被 block-list 排除的基表；空集合返回空字符串，执行函数把驱动错误原样向上传播。
- `ShowMasterStatus`：MySQL 8.4 及以后改用 `SHOW BINARY LOG STATUS`，其他情况使用 `SHOW MASTER STATUS`。
- `createConnWithConsistency`：取得连接；要求 repeatable-read 时尝试设置隔离级别并启动一致性快照，但当前实现刻意忽略这两个 `ExecContext` 的错误。
- `parseSnapshotToTSO`：数字快照直接返回；时间字符串经 `unix_timestamp` 转换为 `(seconds << 18) * 1000`。
- `buildTiDBTableSampleQuery`、`buildPartitionClauses`、`buildTableSampleQueries` 和三个 `build*RegionQueries*`：构造 TiDB region 采样或 region-status 查询，不执行 SQL。

## 执行流程

### 枚举与元数据导出

1. [`dump.rs`](./dump.rs) 的 `prepareTableListToDumpInner` 先确定数据库集合和 `listTableType`，再调用 `ListAllDatabasesTables`。
2. `ListAllDatabasesTables` 为每个 schema 建空列表，根据策略执行 information schema、`SHOW FULL TABLES` 或 `SHOW TABLE STATUS`，解析 `TableType` 和可用的平均行宽，应用调用方传入的类型集合并关闭 `Rows`。
3. `Dumper::dumpDatabases` 遍历 `Config::Tables`：先生成库 DDL；TiDB 还尝试列举 placement policy；随后由 `dumpTableMeta` 取得表、视图或序列的恢复文本并发送元数据任务。
4. 视图路径先创建占位表以保持恢复顺序，再删除占位表/旧视图并在原 charset 环境下执行真实 `CREATE VIEW`。序列路径按服务端类型恢复下一值。

### 整表数据任务

1. `Dumper::dumpWholeTableDirectly` 调用 `SelectAllFromTable`。
2. `SelectAllFromTable` 从 `TableMeta` 读取库名、表名、已选字段和列数，把 `Config::Where` 交给 `buildWhereCondition`，再由 `buildSelectQuery` 拼出查询。
3. `newTableData` 将查询封装为 `TableDataIR`；`dump.rs` 再将其放入 `TaskTableData` 并发送到 writer 通道。SQL 的实际执行与文件写入发生在 IR/writer 层，不在本文件。

当前 Rust `dumpDatabases` 的普通表路径固定调用整表导出，传入空 partition、空 order-by 和单 chunk。虽然本文件已经移植 `pickupPossibleField`、`estimateCount`、`buildWhereClauses` 与 table-sample/region 构造器，RustCodeGraph 没有显示它们被 Rust `dump.rs` 的并发切分流程调用；与 Go 的 `concurrentDumpTable`、`sendConcurrentDumpTiDBTasks` 主链相比，这部分仍属于未完成接线。

### 多列 chunk 条件

给定列 `(a,b,c)` 和若干已按同一比较语义排序的边界，`buildWhereClauses` 先对列名加反引号，再生成首段严格小于、相邻边界半开区间、末段大于等于。`buildCompareClause` 用“前缀相等 + 当前列比较”的析取式表达字典序；`buildBetweenClause` 先消去共同前缀，再将单列区间或三分支多列区间展开。[`sql_test.rs`](./sql_test.rs) 的 `test_go_where_clause_format_and_ranges` 锁定了三列输出的精确空格、括号和等号位置，也验证重复单列边界会产生 `false` 中段。

## 数据与状态

- 本文件自身没有长期拥有的对象状态。多数函数读取 `Config`、`ServerInfo`、`TableMeta`，返回 `String`、向量、映射、布尔值或 `TableDataIR`。
- `DatabaseTables` 是 schema 到 `Vec<TableInfo>` 的映射；`ListAllDatabasesTables` 为每个请求 schema 都插入条目，即使查询没有结果。
- SQL 行数据先落入 `RawBytes`，再通过 `String::from_utf8_lossy` 转换。因此无效 UTF-8 会被替换字符容错，而不是报编码错误；数值解析多处以 `0` 兜底。
- `buildWhereClauses` 的边界值已经是 SQL 字面量字符串，本函数不会再次转义或绑定参数。正确的类型编码和排序是调用方不变量。
- `parseSnapshotToTSO` 除查询服务器外，会读取 mock/迁移层 `DB::query_row` 的 `Mutex<HashMap<...>>` 缓存；锁只在克隆缓存值期间持有。
- `createConnWithConsistency` 返回仍由调用方拥有和关闭的 `Conn`。`SelectAllFromTable` 返回 boxed trait object，把查询及列数状态交给后续任务。

## 依赖与调用关系

### 上游

- `dump.rs::prepareTableListToDumpInner -> ListAllDatabasesTables`：准备最终可导出的表清单。
- `dump.rs::Dumper::dumpDatabases -> ListAllPlacementPolicyNames/ShowCreatePlacementPolicy/ShowCreateDatabase`：生成策略与库级元数据任务。
- `dump.rs::Dumper::dumpWholeTableDirectly -> SelectAllFromTable -> buildWhereCondition/buildSelectQuery -> newTableData`：当前 Rust 数据导出主路径。
- `dump.rs::Dumper::dumpSQL -> detectEstimateRows`：自定义 SQL 模式估算总行数。
- `prepare_test.rs`、`dump_test.rs` 和 `sql_test.rs` 直接调用枚举、整表查询及各类 builder，提供独立测试证据。

### 下游

- 连接层：`Conn::QueryContext`/`ExecContext`、`DB::Query`/`Conn`、`BaseConn::QuerySQLWithColumns`/`ExecSQL`。这些类型来自同 crate 的 `stubs.rs`/`conn.rs` 单包视图。
- 模型层：`Config`、`ServerInfo`、`ServerType`、`DatabaseTables`、`TableInfo`、`TableType`、`TableMeta` 和 `TableDataIR`。
- 通用助手：`wrapBackTicks`、`escapeString`、`columnNamesToSelectFields`、`tableSourceColumnNames`、`tableSourceColumnTypes`、`string2Map`、`dataTypeIntContains`、`newTableData`。
- 错误与日志：`Result<...>`、`Error`、`errors_new`、`errors_errorf`、`errors_annotatef` 和 `tcontext::Context::L()`。

`Cargo.toml` 的外部依赖是整个 export crate 的边界，不等同于本文件逐一直接调用的依赖。本文件可见的绝大多数符号通过 `lib.rs` 的 `include!` 和 `pub use stubs::*` 获得；修改 `lib.rs` 的 include 顺序或共享导入会直接影响它的编译上下文。

## 错误处理与边界

- 查询、扫描和显式 `Rows::Close` 通常使用 `?` 传播错误；DDL 查询无行时返回 `no create ... sql`，指定列不存在时返回 `column ... not found`。
- `simpleQueryWithArgs` 在逐行回调失败或 `Rows::Err` 存在时尝试关闭结果集并返回错误；回调成功后返回最终 `Close` 结果。
- `detectEstimateRows` 将查询失败、无匹配列和不可解析值统一降级为 `0`，调用者必须把零理解为“无法估算或确为零”，不能当作可靠统计值。
- `SelectTiDBRowID` 仅把包含 `1054`、`unknown column` 或 `bad field` 的错误解释为无隐式 rowid；其他错误附带 SQL 上下文返回。
- `createConnWithConsistency` 忽略设置隔离级别和启动快照事务的错误，这是当前兼容旧 MySQL 的行为，也意味着仅凭 `Ok(Conn)` 不能证明快照已建立。
- `ShowCreateSequence` 对 `DBConn.as_ref().unwrap()` 有前置条件：`BaseConn` 必须持有底层连接；违反时会 panic。`getWritableColumnNames` 假定每个投影结果至少有两列并直接索引 `row[1]`。
- `buildCompareClause` 假定 `quota_cols` 与 `bound` 至少同长，`buildBetweenClause` 还假定 low/up 与列向量维数一致；不满足会索引越界。它们也不校验边界是否有序。
- `GetPdAddrs` 特意丢弃 `Scan` 错误（`let _ = ...`），可能返回部分结果；其他相近探测函数通常传播扫描错误。
- `GetSuitableRows(u64::MAX)` 的整数除法结果为零，测试明确保留此边界；函数不强制至少一行。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道，也不持有跨调用事务。并发执行发生在上层 `dump.rs`/writer；这里生成的纯字符串 builder 可由不同调用者独立使用。

数据库资源遵循“获取 `Rows`—遍历/扫描—显式 `Close`”模式。多数函数在正常和空结果路径都关闭 `Rows`；`simpleQueryWithArgs` 在回调或迭代错误时也尝试关闭。`createConnWithConsistency` 的返回连接和它启动的事务生命周期交给调用方，当前函数没有 rollback/commit。`FlushTableWithReadLock` 或 `buildLockTablesSQL` 获得的锁也必须由上层确保调用 `UnlockTables`；本文件没有 RAII guard，错误路径的解锁责任不能遗漏。

唯一显式同步原语出现在 `parseSnapshotToTSO` 访问 `DB::query_row.lock()` 的迁移层缓存。锁中毒会因 `unwrap()` panic；数据库查询和 `Rows` 本身的线程安全保证由 `stubs.rs`/实际连接实现决定，不由本文件扩展。

## 与 Go 版本的对应关系

主要 API、Go 风格命名和 SQL 文本来自 [`sql.go`](./sql.go)。已有单元测试特别覆盖枚举数值、空字段 `SELECT ''`、where/chunk 精确格式、隐式 rowid、DDL、序列、charset、分区和 region builder，说明 Rust 目标是行为对齐而非重新设计。

但当前不是完整等价移植，扩展前应先确认差异是否属于目标 commit：

- Go `ShowCreateDatabase` 对 MemSQL/SingleStore 不支持 `SHOW CREATE DATABASE` 有 `CREATE DATABASE` 回退；Rust 版本没有该回退。
- Go `ListAllDatabasesTables` 的 information-schema 和 `SHOW FULL TABLES` 查询在 SQL 侧加入类型条件，并对 parse/数值错误返回错误；Rust 查询后在内存过滤，未知类型和非法平均行宽分别回退为 base/0。Rust `SHOW TABLE STATUS` 路径也没有保留 Go 的平均行宽和“无 engine 且非 view comment”警告/跳过逻辑。
- Go `estimateCount` 会反引号包裹字段并附加 `Config::Where`；Rust 直接插入 `field`，忽略传入的 `_conf`。因此 Rust 的估算 SQL 可能与最终导出过滤范围不一致。
- Go `createConnWithConsistency` 会在非 RR 分支处理 `tidb_snapshot`/未知系统变量并确保失败时关闭连接；Rust 只有可选 RR 两条 best-effort 语句。
- Go `buildTiDBTableSampleQuery` 位于 `dump.go`，包含 `ORDER BY` 且使用 `PARTITION(...)`；Rust helper 当前不加 `ORDER BY`，输出 `PARTITION (...)`，而且尚未接入 Rust 导出主链。
- Go `sql.go` 还有 `updateSpecifiedTablesMeta`、`GetColumnTypes`、`GetPrimaryKeyAndColumnTypes`、`LockTables`、keyspace 查询、session 参数重建、`GetPartitionTableIDs`、`GetDBInfo`、`GetRegionInfos` 等实现；不能假设 Rust 本文件已提供这些行为。部分能力可能位于 Rust 同 crate 其他文件或仍未移植，需逐项搜索验证。
- 反向地，Rust 文件末尾集中放置的 table-sample/region 查询 builder 在 Go 中部分属于 `dump.go`，所以“同路径对照”之外还要检查 Go 的直接调用文件。

## 扩展指南

- 新增元数据查询时，优先复用 `BaseConn::QuerySQLWithColumns` 或完善 `simpleQueryWithArgs`，确保列名兼容、迭代错误与 `Close` 错误都有明确策略；对应测试放在独立的 [`sql_test.rs`](./sql_test.rs)，不要内嵌进生产文件。
- 修改 SQL builder 时同步覆盖标识符反引号转义、字符串字面量转义、空字段/空条件、复合键共同前缀、重复边界、分区名和尾随空格。`buildWhereClauses` 的输出用于无参数 SQL，任何转义变化都有正确性与注入风险。
- 要接入并发 chunk 导出，应在 `dump.rs` 建立从版本/分区判断、采样或 region 查询、边界解码、`buildWhereClauses` 到任务发送的完整链，并移植 Go `concurrentDumpTable`/`sendConcurrentDumpTiDBTasks` 的回退与取消语义；不能只让现有 helper 通过编译。
- 要补齐快照一致性，应同时定义连接建立失败的关闭规则、事务提交/回滚、未知系统变量兼容和锁释放责任，并在独立连接/一致性测试中验证错误路径。
- 修改 Go 对齐逻辑前先比较 `sql.go` 以及 `dump.go` 中直接相关函数；仅对齐目标 Go commit 的增量和必要接线，把无关的既有移植缺口记录为范围外，不借机扩建子系统。
- 若新增 Rust 源码行为，依仓库协议同步独立测试、运行 `cargo fmt --all` 并使用适用验证流程；本说明任务只新增文档，没有改变源码或测试。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；`files --filter dumpling/export` 确认本目录 Rust/Go 源与独立测试均已索引。
- 源码全貌：RustCodeGraph `node --file dumpling/export/sql.rs --offset 1/500/1000` 覆盖 1,422 行；确认本文件有 1 个枚举、1 个常量、55 个函数，无本地 struct/trait/impl/条件编译项。
- crate 边界：读取 [`Cargo.toml`](./Cargo.toml) 和 RustCodeGraph `node --file dumpling/export/lib.rs`；确认 `include!("sql.rs")`、共享导入、stub 基座与 `#[path = "sql_test.rs"]` 独立测试模块。
- 调用图：RustCodeGraph `node prepareTableListToDumpInner`、`node dumpWholeTableDirectly` 及精确 explore 显示 `prepareTableListToDumpInner -> ListAllDatabasesTables`、Rust `dumpWholeTableDirectly -> SelectAllFromTable`；`node --file dump.rs` 又确认 `dumpDatabases` 的 DDL 调用和 `dumpSQL -> detectEstimateRows`。图中 `buildWhereClauses`、`buildLockTablesSQL`、`parseSnapshotToTSO`、`pickupPossibleField`、`buildTableSampleQueries` 等没有 Rust 生产调用者，因此文中按未接线 helper 描述。
- Rust 测试：读取 [`sql_test.rs`](./sql_test.rs) 全部 782 行，并参考 `prepare_test.rs`、`dump_test.rs` 的图调用边。核心证据包括 `test_build_select_all_query`、`test_build_order_by_clause`、`test_build_select_field`、`test_parse_snapshot_to_tso`、`test_show_create_view`、`test_show_create_sequence`、`test_get_suitable_rows`、`test_go_where_clause_format_and_ranges` 和 region/table-sample builder 测试。
- Go 对照：读取 RustCodeGraph 中 [`sql.go`](./sql.go) 的对应 DDL、枚举、SELECT、边界、索引、估算和快照实现，并读取 [`dump.go`](./dump.go) 的 `concurrentDumpTable`、`sendConcurrentDumpTiDBTasks`、`selectTiDBTableSample`、`buildTiDBTableSampleQuery` 主链，差异已在上一节逐项标注。
- 人工复核结论：本文件存在于导出 SQL/元数据规划层；当前已接线流程以对象枚举、DDL 和整表查询为主；安全扩展必须保持资源关闭、标识符/字面量转义、字典序半开区间、Go 兼容和独立测试边界。
