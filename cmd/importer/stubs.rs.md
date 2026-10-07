# `cmd/importer/stubs.rs`

## 文件定位

`cmd/importer/stubs.rs` 是 `astersql-cmd-importer` crate 的迁移兼容层，由 `cmd/importer/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 公开。它不是一个独立业务入口；二进制从 `bin_main.rs` 进入 crate，再由 `lib.rs::main` 转发到 `main.rs` 的 importer 主流程。该文件把 Go importer 原先直接依赖的 TiDB/Go 标准库能力压缩成当前 Rust importer 所需的最小接口，以避免在 arm64 Darwin 等环境引入完整的 KV、domain、kvproto 或 grpcio 依赖。

crate 边界由 `cmd/importer/Cargo.toml` 确认：包名为 `astersql-cmd-importer`，同时提供库和同名二进制，直接依赖只有 `toml = "0.8"` 与 `serde_json = "1"`。不过本文件的 TOML 和 JSON 处理主体仍是手写的有限适配器；它们不是通用格式实现。

## 核心职责

文件按职责分成七组兼容面（源码中的 `// --- ... ---` 分区可直接复核）：

1. `Error`、`Result`、日志与退出函数模拟 `pingcap/errors`、`pingcap/log`、`zap`、`os.Exit` 的 importer 可见语义。
2. MySQL 类型常量、`FieldType`、`TableInfo` 及精简 AST/解析器替代 Go 侧 `parser`、`types`、`model`、`ddl.BuildTableInfoFromAST` 的局部用法。
3. `CivilTime` 和解析/差值函数支撑随机日期时间生成及统计边界读取。
4. `StatsTable`、`HistogramCore`、`Bounds` 等类型以及 JSON 加载器提供 importer 使用的统计直方图形状。
5. `DB`/`Tx` 是记录型 `database/sql` 桩，记录 DSN、执行 SQL、事务开始/提交和关闭状态，并可注入失败。
6. 线程本地伪随机源提供 `rand.Intn`、`Int31n`、`Int63`、`Int63n` 形状的接口。
7. `apply_toml_to_config_fields` 只覆盖 importer 配置中已知字段。

因此它的正确性边界是“保持 `cmd/importer` 当前调用点的可观察契约”，不是完整实现 Go/TiDB 依赖。源码模块注释也明确称其为“最小可用兼容层”。

## 主要符号

- 错误与进程边界：`Error { msg, is_help }`、`Result<T>`、`cause`、`trace`、`errorf`、`fatal`、`log_error`、`log_warn`、`os_exit`、`args_from_env`。`fatal` 用 panic 表示 Go `log.Fatal` 的终止语义；`os_exit` 在非测试构建调用 `std::process::exit`，测试构建则记录退出码后 panic。
- 类型与元数据：`TypeTiny` 至 `TypeString`、`UnsignedFlag`、`HasUnsignedFlag`、`FieldType`、`ColumnInfo`、`IndexInfo`、`TableInfo`。字段只覆盖 importer 会读取的类型号、flag、长度、小数位、列偏移和索引列。
- SQL 解析：`CIStr`、`ColumnDef`、`Constraint`、`CreateTableStmt`、`CreateIndexStmt`、`StmtNode`、`parse_one_stmt`、`build_table_info_from_ast`。`CIStr` 同时保留原字符串 `O` 与小写字符串 `L`；解析器只识别 importer 所需的 `CREATE TABLE`/`CREATE INDEX` 子集。
- 时间：`CivilTime`、`parse_date`、`parse_time_of_day`、`parse_datetime`、`parse_year`、`timestamp_diff` 及 Go layout 常量函数。`CivilTime::now` 使用 Unix 时间按 UTC 形状近似墙上时间，不实现完整时区规则。
- 统计：`BoundDatum`、`Bounds`/`BoundRow`、`Bucket`、`HistogramCore`、`ColumnStats`、`StatsTable`、`table_stats_from_simplified_json`、`load_stats_file`。`load_stats_file` 读取文本后按内容在简化 JSON 与 TiDB stats JSON 适配路径之间解析。
- 数据库：`DB`、内部 `DbInner`、`Tx`、`open_db`。`DB` 是 `Arc<Mutex<DbInner>>` 的可克隆句柄；`Tx` 用独立锁跟踪事务 SQL 与完成状态。
- 随机与配置：`seed_rng`、`rand_intn`、`rand_int63`、`rand_int63n`、`rand_int31n`、`apply_toml_to_config_fields`。随机实现是线程本地 xorshift64*，并不保证与 Go `math/rand` 产生同一数列。

## 执行流程

Importer 主链中的典型流程如下：

1. `main.rs::main` 调用 `args_from_env`；参数失败时通过日志适配和 `os_exit` 保留退出码边界。
2. `config.rs` 在读取配置文件时调用 `apply_toml_to_config_fields`，只把 `[db]`、`[ddl]`、`[stats]`、`[sys]` 下的已知键覆盖到配置字段；整数字段解析失败返回 `Error`。
3. `parser.rs::parseTableSQL`/`parseIndexSQL` 调用 `parse_one_stmt`。建表路径分割顶层逗号、解析列类型/选项/约束，再由 `build_table_info_from_ast` 顺序分配列 ID、偏移及索引 ID；建索引路径提取表名、唯一性和列名。
4. `main.rs::run_with_args` 可通过 `stats.rs::loadStats` 进入 `load_stats_file`，将 JSON 桶、边界和列/索引 ID 映射到 `StatsTable`，随后将有桶的直方图挂到 importer 列上。
5. `db.rs::createDB` 通过 `open_db` 创建记录型句柄；`job.rs` 通过 `DB::Begin`、`Tx::Exec`、`Tx::Commit` 执行批次。SQL 同时记入事务局部列表和 DB 总执行列表，便于 parity 测试观察 DDL/DML 顺序。
6. `data.rs`、`rand.rs`、`stats.rs` 通过随机与时间函数产生普通值或在直方图边界内采样；`seed_rng` 让测试可重复。
7. 主流程结束后 `db.rs::closeDBs` 遍历调用 `DB::Close`，关闭状态保留在共享内部对象中。

RustCodeGraph 给出的下游边包括：`parse_one_stmt -> parse_create_table/parse_create_index`，`build_table_info_from_ast -> ColumnInfo/IndexColumn/IndexInfo/TableInfo`，`load_stats_file -> table_stats_from_simplified_json/table_stats_from_tidb_json`，`apply_toml_to_config_fields -> parse_toml_i32/trim_toml_value`，`rand_int63 -> next_u64`，`os_exit -> exit_slot/exit_process`。

## 数据与状态

- `PROCESS_EXIT_CODE: OnceLock<Mutex<Option<i32>>>` 仅为测试退出探针保存最近一次退出码；`take_exit_code` 取出后清空。非测试退出不会返回。
- `TableInfo.Columns` 的 `Offset` 来自 AST 列顺序，ID 从 1 开始；`Indices` 的 ID 也从 1 递增。列级主键、唯一键乃至 `AutoIncrement` 都会在这个兼容层中被视为需要生成索引信息，表级约束只收录能在列列表中解析到的列。
- `CivilTime` 是无时区的年月日时分秒值。默认值全零；时间解析只按固定分隔符拆分，未实现 Go `time` 的全部日期合法性和时区校验。
- `StatsTable` 使用两个 `HashMap<i64, ColumnStats>` 分别保存列与索引统计；`Bounds.rows` 以 `BoundDatum::{Int, Str, Time}` 保存上下界。类型不匹配或越界读取通常回退零值/空值。
- `DB` 克隆共享 `DbInner`。其中 `execs` 记录所有执行 SQL，`begins`/`commits` 记录次数，`closed` 和三个 `fail_*` 标志控制生命周期与错误注入。`Tx.done` 防止重复提交或提交后继续执行。
- `RNG` 是线程局部 `Cell<u64>`，默认种子固定；不同线程各自持有状态，不共享全局序列。
- TOML 覆盖器的 `section` 是逐行状态；未知 section/键和无 `=` 的行被忽略。

## 依赖与调用关系

上游调用均位于同一 crate：

- `config.rs` 使用 `Error`/`Result`，并通过配置覆盖函数吸收 TOML 字段。
- `parser.rs` 使用 AST、类型、元数据、`parse_one_stmt`、`build_table_info_from_ast` 和 `fatal`。
- `data.rs` 使用 `CivilTime` 与随机函数；`rand.rs` 使用日期/时间解析、差值和警告日志。
- `stats.rs` 使用 `HistogramCore`、`StatsTable`、`load_stats_file`、`timestamp_diff`、随机函数及 `fatal`。
- `db.rs` 使用 MySQL 类型/flag、`DB`/`Tx`、`open_db`、随机函数、`trace` 和日志。
- `job.rs` 使用 `DB` 事务接口和 `fatal`；`main.rs` 使用 `DB`、参数、退出和日志接口。

下游仅使用 Rust 标准库（集合、格式化、文件、I/O、同步、时间、线程局部存储、进程）及文件内部辅助函数。尽管 `Cargo.toml` 声明 `toml` 与 `serde_json`，当前 `stubs.rs` 的有限 TOML/JSON 解析没有直接调用它们。

RustCodeGraph 对主要入口的 callees 有结果，但对这些同 crate 上游 callers 查询返回空；上游关系因此由 `rg` 对 `cmd/importer/*.rs` 的 `crate::stubs` 引用补证。索引还把常见符号名与仓库其他 `stubs.rs` 混合展示，所以不能把其他 crate 的同名 `record`、`calls` 等结果归到本文件。

## 错误处理与边界

- `Error` 只保存消息和帮助标志；`cause`/`trace` 原样返回，不形成 Go errors 的包装链。
- `fatal` 总是 panic；它可被测试捕获，但与 Go `log.Fatal` 的日志刷新及立即进程退出并不完全相同。真正退出码由 `os_exit` 在生产构建执行。
- SQL 解析器对非 CREATE 语句返回 `StmtNode::Other`，由 `parser.rs` 决定是否报错；缺少表名、括号、`ON`、列列表或规定关键字时返回明确错误。它只做有限词法处理，复杂转义、完整 MySQL 文法和所有类型修饰符不在保证范围内。
- `build_table_info_from_ast` 遇到约束中不存在的列会跳过该列；若一个约束最终没有有效列，则不生成索引。
- 时间解析检查字段数和整数转换，但没有完整验证月份、日期、时分秒范围；`timestamp_diff` 对未知单位返回 0。
- 统计 JSON 适配器只识别所需对象/数组/数值/字符串形状。畸形 JSON 由加载入口返回错误；边界读取的宽松默认值可能隐藏类型不匹配，扩展时应补独立测试。
- `DB::Exec`/`Begin` 在关闭后报 `sql: database is closed`；事务重复完成报与 Go `database/sql` 相近的错误。`Tx::Exec` 当前先写事务局部 `execs`，再检查 DB 的 `fail_exec`，所以失败 SQL 可能存在于事务局部观察状态但不会进入 DB 总日志。
- `rand_intn`、`rand_int31n`、`rand_int63n` 对非正上界 assert panic；调用方必须维持正区间不变量。
- TOML 解析只对三个整数字段严格转换，未知字段静默忽略；引号去除也只是首尾字符处理，不支持完整转义语义。

## 并发与资源生命周期

`DB` 和 `Tx` 使用 `Arc<Mutex<...>>`，允许 worker 线程持有克隆句柄并串行更新执行记录。锁中毒通常被转换成 `Error`；少数 `Tx.done`/局部 `execs` 锁直接 `unwrap`，锁中毒会 panic。锁的持有区间较短，没有跨 I/O 的真实数据库操作，因为本实现只记录状态。

事务生命周期为 `DB::Begin -> Tx::Exec* -> Tx::Commit`。成功提交设置 `done = true` 并增加 DB commit 计数；没有显式 rollback/drop 回滚逻辑，未提交事务被丢弃时仅释放内存。`DB::Close` 是幂等地设置 `closed`，但已有 `Tx` 的 `Exec`/`Commit` 没有统一检查 `closed`，因此不能把它视作真实数据库连接池。

随机状态是 thread-local：锁无竞争，但相同 seed 在不同线程上各自产生独立序列，worker 调度也可能影响整体输出组合。测试需要在实际执行随机调用的线程中调用 `seed_rng`。

退出探针的 `OnceLock<Mutex<_>>` 生命周期覆盖进程；`take_exit_code` 消耗状态以避免测试间残留。文件加载通过 `fs::read_to_string` 一次性读取，没有流式资源或后台任务。

## 与 Go 版本的对应关系

仓库没有 `cmd/importer/stubs.go`；Rust 文件横向替代多个 Go 依赖：

- `main.go` 的 `errors.Cause`、`log.Error/Fatal`、`os.Args`、`os.Exit` 对应错误/日志/参数/退出适配。
- `parser.go` 的 `parser.New().ParseOneStmt`、AST 类型、`ddl.BuildTableInfoFromAST`、`model.TableInfo`、`types.FieldType` 对应精简 SQL/元数据区。Go 使用完整 TiDB parser/DDL；Rust 只支持 importer 目前所需子集。
- `stats.go` 的 `storage.TableStatsFromJSON`、statistics/types 边界接口和 `types.TimestampDiff` 对应统计与时间区。Rust JSON 适配器只恢复当前测试所需字段。
- `db.go`/`job.go` 的 `database/sql`、MySQL driver、事务接口对应记录型 `DB`/`Tx`。Go 连接真实数据库，Rust 桩不会发起网络连接。
- `data.go`/`rand.go` 的 `math/rand` 与 `time.Time` 对应线程本地 RNG 和 `CivilTime`。接口语义用于维持范围、格式和可重复测试，而非逐值兼容 Go PRNG 或完整时间库。
- `config.go` 的 BurntSushi TOML 解码对应有限字段覆盖器；Go 对结构化 TOML 类型更严格、更完整。

Go 侧直接测试只有 `cmd/importer/db_test.go` 的十进制格式案例；Rust 侧独立测试更广，位于 `parity_test.rs`、`parser_test.rs`、`stats_test.rs`、`rand_test.rs`、`config_test.rs`、`data_test.rs`、`db_test.rs`，它们通过上层模块间接覆盖桩契约。`parity_test.rs` 明确覆盖退出码、非法 TOML、CREATE 解析/MySQL 类型号、DB 失败无死锁、畸形统计 JSON、完整流水线、资源关闭和事务提交次数。

## 扩展指南

- 新增 SQL 语法时，优先修改 `parse_one_stmt` 及其私有解析函数，并同步 `parser.rs` 的消费逻辑；在独立 `parser_test.rs` 或 `parity_test.rs` 增加成功、畸形输入和 Go 对照案例。不要把完整 SQL parser 无边界地重写进此桩。
- 新增 MySQL 类型时，需要同时核对类型号、`parse_type` 的长度/小数位、`db.rs` 的值生成分派以及 `rand_test.rs`/parity 类型断言。
- 扩展统计格式时，修改 `table_stats_from_simplified_json` 或 TiDB JSON 适配路径，并在 `stats_test.rs` 使用具名列/索引、空桶、错误类型和日期边界覆盖；注意手写 JSON 解析对转义和嵌套的限制。
- 若接入真实数据库驱动，应替换 `open_db`、`DB`、`Tx` 整个契约并保留 DDL/DML 顺序、错误传播、批次提交和关闭验证；不能只让现有记录桩“看起来连接成功”。
- 新增配置项时，需同步 `config.rs` 的配置结构/CLI 优先级、`apply_toml_to_config_fields` 的 section/key 分支以及 `config_test.rs` 的类型错误覆盖。
- 修改并发状态时，避免在持锁状态调用可能 panic 或阻塞的外部逻辑；明确关闭后事务行为、失败前后记录顺序和锁中毒策略。
- 若未来真实 TiDB Rust 依赖能够覆盖这些能力，应先用现有独立测试固定可观察契约，再逐组替换；保留轻量桩或删除它应以调用点全部迁移为准。

## 验证依据

- 源文件：`cmd/importer/stubs.rs`（2276 行；RustCodeGraph 文件节点显示 190 个符号）。主要分区、公开类型/函数和私有辅助实现均已检查。
- crate 装配：`cmd/importer/Cargo.toml`、`cmd/importer/lib.rs`、`cmd/importer/bin_main.rs`、`cmd/importer/main.rs`。
- Rust 调用点：`cmd/importer/config.rs`、`data.rs`、`rand.rs`、`stats.rs`、`parser.rs`、`db.rs`、`job.rs`。
- Rust 独立测试：`cmd/importer/parity_test.rs`、`parser_test.rs`、`stats_test.rs`、`rand_test.rs`、`config_test.rs`、`data_test.rs`、`db_test.rs`。测试逻辑没有放入生产源文件。
- Go 对照：`cmd/importer/main.go`、`config.go`、`data.go`、`rand.go`、`stats.go`、`parser.go`、`db.go`、`job.go`、`db_test.go`。
- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter cmd/importer` 确认目标和相邻模块；`node --file cmd/importer/stubs.rs` 核对源码；对 `parse_one_stmt`、`build_table_info_from_ast`、`load_stats_file`、`open_db`、`apply_toml_to_config_fields`、`rand_int63`、`timestamp_diff`、`os_exit` 运行 callers/callees。callers 对这些入口未返回本 crate 上游，故用上述 Rust 调用点的 `rg` 结果补证。
- 结构验收应使用任务指定命令，要求文件存在且恰有 11 个固定二级标题。本任务是纯文档分析，按计划不运行 Cargo。
