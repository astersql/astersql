# [`pkg/statistics/handle/storage/save.rs`](./save.rs)

## 文件定位

本文件属于独立 crate `astersql-statistics-handle-storage`；crate 入口 `pkg/statistics/handle/storage/lib.rs` 以 `pub mod save` 声明模块并用 `pub use save::*` 重导出公开函数。它位于统计信息的持久化写侧：接收 `stats_read_writer.rs` 定义的内存统计结构和 `SqlStore` 抽象，把表级元数据、直方图、桶、TopN、CMSketch、FMSketch 及列分析时间转换为对 `mysql.stats_*` 系统表的 SQL。

当前 Rust 生产主链中，可确认的入口是 `StatsReadWriter::save_analyze_result` 调用 `save_analyze_result_to_storage`（`stats_read_writer.rs:336-368`）。其余四个公开函数已由 crate 根重导出，但仓库内 Rust 搜索仅发现本文件内部调用或独立测试调用；因此它们是可用的存储 API，不能据此声称已接入与 Go 完全相同的 DDL、全局统计和导入链路。

## 核心职责

- `save_analyze_result_to_storage` 持久化一次 `ANALYZE` 结果，串联表级 `stats_meta` 更新和所有列/索引统计明细写入，并返回供历史统计记录使用的版本号。
- `save_column_or_index_stats_at_version` 统一执行单个列/索引统计的“删除旧附属行、写入新行”顺序，避免旧 TopN、FMSketch 或桶残留。
- `save_top_n`、`save_buckets` 将明细限制为每批最多 10 个元组，并在非首元组使 SQL 接近 1 MiB 时提前截断，控制单条内部 SQL 大小。
- `save_column_or_index_stats`、`insert_column_stats_to_kv` 和 `insert_table_stats_to_kv` 提供较窄的覆盖写入/整表导入入口；`save_meta_to_storage` 只更新表级计数与版本。

本文件不负责统计计算、SQL 事务的开始/提交/回滚、缓存刷新或历史快照记录。统计数据结构、错误类型和存储执行接口来自 `stats_read_writer.rs`；事务原子性取决于传入 `SqlStore` 的会话/事务实现。

## 主要符号

- `BATCH_INSERT_SIZE: usize = 10`：TopN 和桶的最大批次元组数，与 Go `batchInsertSize` 一致。
- `MAX_INSERT_LENGTH: usize = 1024 * 1024`：批量 INSERT 的目标长度上限。首个元组即使自身超过上限也会被写入，从而保证循环前进。
- `save_top_n(...)`：私有函数。使用 `sql_bytes` 把 `TopNItem::encoded` 编为十六进制 SQL 字面量，写入 `mysql.stats_top_n`。
- `save_buckets(...)`：私有函数。将 `Histogram::buckets` 的累计 `count` 转成相对前一桶的增量，连同边界、重复数和桶内 NDV 写入 `mysql.stats_buckets`。
- `save_column_or_index_stats_at_version(...)`：私有核心函数。参数决定版本、统计格式版本以及是否保存 CMSketch/FMSketch、是否更新 `column_stats_usage`。
- `save_analyze_result_to_storage(...) -> Result<u64, Error>`：公开主入口。`0` 是有业务含义的返回值，表示过期 v2 ANALYZE 被跳过，或已有 meta 的 MV/全局索引辅助 ANALYZE 不应触发表级历史 meta 记录。
- `save_column_or_index_stats(...)`：公开单对象覆盖入口；自行取 `start_ts`，v2 不保存 CMSketch，也总是清理但不重新插入 FMSketch，不更新时间戳。
- `save_meta_to_storage(...)`：公开单表 meta 更新入口，调用者显式提供版本、计数和修改计数。
- `insert_column_stats_to_kv(...)`：公开列统计写入入口；与上一入口相同地复用核心函数，但固定 `is_index=false`。
- `insert_table_stats_to_kv(...)`：公开整表写入入口；先写 meta，再遍历 `TableStats::columns` 和 `indices`。索引版本取 `max(table.version, index.histogram.last_update_version)`。

文件中没有 trait、struct、impl、宏或条件编译项；五个公开函数通过 crate 根重导出，三个私有函数封装 SQL 细节。

## 执行流程

`save_analyze_result_to_storage` 的流程如下：

1. 调用 `SqlStore::start_ts` 获取本次统计版本，并以 `-table_id` 和真实 `table_id` 组成 `FOR UPDATE` 查询。负 ID 是与 Go 路径一致的伪元行锁键，用于降低点查与批量点查的锁顺序死锁风险；Rust 文件只发出 SQL，不直接管理锁生命周期。
2. 若查到的 snapshot 不小于结果 snapshot，且结果为 v2、不是 MV/全局索引，则直接返回 `0`，不写入任何统计明细。
3. 没有 meta 行或结果不是 v2 时，用 `REPLACE` 写 `stats_meta`；MV/全局索引把 snapshot/count 置零。已有 v2 MV/全局索引只刷新两个版本字段，并把最终返回版本保留为 `0`。普通 v2 则将 `modify_count` 更新为 `max(current_modify-base_modify_count, 0)`；行数按 `analyze_snapshot` 选择增量合并或覆盖，并钳制到非负。
4. 遍历 `AnalyzeResults::columns`，依次调用 `save_column_or_index_stats_at_version`。每个对象先删旧 TopN 再分批插入，删旧 FMSketch 后按开关插入，`REPLACE` 直方图元行，再删旧桶并分批插入；普通列最后 UPSERT `column_stats_usage.last_analyzed_at`。
5. 任一步返回错误即停止并向上传播；全部完成后返回 `saved_version`。

`save_top_n` 和 `save_buckets` 都维护 `[offset, limit)` 窗口。每成功拼入一个元组才推进 `end`，执行成功后令 `offset=end`；长度判断只作用于第二个及后续元组，所以即使单项很大也不会形成零进展循环。桶的 `bucket_id` 使用全局数组下标，批次切分不会重编号。

`insert_table_stats_to_kv` 不自行获取新时间戳，而使用 `TableStats::version` 贯穿 meta 与列统计；索引可因 `last_update_version` 更大而使用更高的直方图版本。HashMap 的遍历顺序未定义，因此列与索引之间的 SQL 顺序不是稳定接口。

## 数据与状态

输入状态定义在 `stats_read_writer.rs`：`AnalyzeResults` 保存表 ID、snapshot、计数基线、统计版本、MV/全局索引标记和 `(is_index, ColumnStats)` 列表；`ColumnStats` 聚合 `Histogram`、可选 CMSketch/FMSketch、TopN 和统计版本；`TableStats` 用两个 `HashMap` 保存列与索引。

持久化不变量包括：

- `stats_histograms.version`、`stats_meta.version` 和 `last_stats_histograms_version` 使用调用路径选定的统计版本；ANALYZE 主路径使用 `start_ts`。
- 直方图内存桶的 `count` 是累计值，存储行的 `count` 是相邻桶差值；因此调用者必须提供按顺序且累计计数不下降的桶。代码未显式校验该前置条件。
- `total_column_size` 写入前以 `max(0)` 钳制，meta 的合并 `count` 和 `modify_count` 也不会写成负数。
- v2 路径将 `cm_sketch` 写为 SQL `NULL`；非 v2 是否写 CMSketch由 `save_cmsketch` 参数控制。
- 每次保存都会先删除旧 FMSketch；只有 `save_fm_sketch=true` 且载荷存在时才重新插入，防止陈旧草图继续可见。

本文件自身不保存内存全局状态。所有可观察状态都在数据库或测试 `SqlStore` 的语句记录中；字节载荷通过 `sql_bytes` 生成 `x'...'`，数值直接格式化进内部 SQL。

## 依赖与调用关系

直接 Rust 依赖均从 crate 根导入：`AnalyzeResults`、`ColumnStats`、`Histogram`、`TableStats`、`TopNItem`、`Error`、`SqlStore` 和 `sql_bytes`，实际定义均在 `stats_read_writer.rs`。`SqlStore::start_ts` 提供版本，`SqlStore::execute` 是所有查询和写入的唯一 I/O 边界。

已验证的调用边：

- 上游：`StatsReadWriter::save_analyze_result` → `save_analyze_result_to_storage`；它还会在保存耗时超过半个 lease 时推进 meta 版本，并在非零版本时记录历史 meta。
- 内部：`save_analyze_result_to_storage`、`save_column_or_index_stats`、`insert_column_stats_to_kv`、`insert_table_stats_to_kv` → `save_column_or_index_stats_at_version`。
- 内部：`save_column_or_index_stats_at_version` → `save_top_n`、`save_buckets`、`SqlStore::execute`、`sql_bytes`。
- 测试：`save_test.rs` 直接覆盖 MV/全局索引返回零版本及单列保存清理 FMSketch；`stats_read_writer_test.rs` 覆盖 ANALYZE 的直方图、FMSketch和分析时间 SQL。

`pkg/statistics/handle/storage/Cargo.toml` 确认该目录本身是 crate 边界，`lib.rs` 是 crate 入口。manifest 的常规 `[dependencies]` 为空；所列旧 TiDB 子 crate 依赖全部处于 `target.'cfg(any())'`，该条件永假，说明当前实现刻意通过本地轻量抽象自包含。父 crate `astersql-statistics-handle` 以正常 path dependency 引用本 crate。

## 错误处理与边界

所有 I/O 返回统一的 `Result<_, Error>`，并通过 `?` 原样传播第一个 `start_ts` 或 `execute` 错误；本文件不重试、不记录日志，也不补偿已经成功的前序 SQL。只有上层 `StatsReadWriter::save_analyze_result` 会把“慢保存后推进版本失败”转换成稳定的用户提示。

空 TopN 或空桶不会发出 INSERT。批量长度限制不是单行硬上限：为了保证前进，第一个元组可使 SQL 超过 1 MiB。`save_buckets` 对累计计数执行普通 `i64` 减法；异常的递减/溢出输入未在此校验。`save_analyze_result_to_storage` 假定 meta 查询的首行对应真实表（负伪 ID 通常不存在），没有对返回行的 table ID 再排序或辨识。

SQL 由内部数值和 `sql_bytes` 产生；当前接口没有接收任意表名或原始用户字符串。与 Go 的参数化转义执行相比，这是一层简化抽象。失败发生在“先删旧行、后插新行”之间时是否回滚，完全依赖外部 `SqlStore` 是否在同一事务中执行这些语句；单看本文件不能保证独立调用具有原子性。

## 并发与资源生命周期

文件没有线程、异步任务、channel、显式 mutex 或资源句柄。并发协调唯一可见机制是 `save_analyze_result_to_storage` 发出的 `SELECT ... FOR UPDATE`；锁何时释放由 `SqlStore` 背后的事务决定。`SqlStore` trait 要求 `Send + Sync`，因此实现可被跨线程共享，但本文件逐条同步调用它，不引入并行写入。

ANALYZE 写入的生命周期是“取得 start_ts → 锁定/读取 meta → 更新 meta → 顺序替换各列/索引附属行 → 返回版本”。旧 TopN、FMSketch 和桶在新数据插入前删除，要求整个序列由调用者的事务包裹才能对并发读者呈现原子替换。`StatsReadWriter` 的缓存 mutex 与历史记录生命周期位于 `stats_read_writer.rs`，本文件不接触缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/storage/save.go`。以下语义已经对齐：批量大小和 SQL 长度常量；TopN 分批写入；桶累计计数转 delta；ANALYZE 使用 start TS；负伪表 ID 与真实 ID 一并 `FOR UPDATE`；过期普通 v2 结果跳过；v1/无 meta、MV/全局索引和普通 v2 的三类 meta 分支；先清理旧附属行再写新数据；`tot_col_size` 非负；列分析时间 UPSERT。

当前 Rust 不是 Go 文件的等量移植，重要差异如下：

- Go 通过真实 `sessionctx`/事务、RecordSet drain/close、参数化转义、上下文、日志和 failpoint 工作；Rust 通过同步 `SqlStore` 和字符串 SQL 模拟这些边界，没有相应资源关闭、日志或故障注入逻辑。
- Go 从 Datum 转换桶边界为 Blob；Rust 的 `Histogram` 已持有编码后的 `Vec<u8>`，因此只负责十六进制字面量编码。
- Go 只在分区统计的 ANALYZE 路径保存 FMSketch（`needDumpFMS`）；Rust ANALYZE 当前固定传 `save_fm_sketch=true`，没有表 ID 类型信息可复现该判断。这是已确认的语义差异。
- Go 会跳过 nil 的普通虚拟列直方图；Rust 的 `columns` 只包含具体 `ColumnStats`，没有 nil/虚拟列分支。
- Go `SaveColOrIdxStatsToStorage` 还能按 `count` 参数更新 meta 并返回版本；Rust `save_column_or_index_stats` 只写列/索引明细且返回 `()`。
- Go `SaveMetaToStorage` 批量 UPSERT 多个 `MetaUpdate` 并可选择是否刷新 `last_stats_histograms_version`；Rust `save_meta_to_storage` 是单表 UPDATE，版本由调用者传入。
- Go `InsertColStats2KV` 为新增列根据默认值建立占位直方图/桶；Rust `insert_column_stats_to_kv` 保存一份已经构造好的完整 `ColumnStats`，不是同一 DDL 语义。
- Go `InsertTableStats2KV` 基于 schema 创建零值占位行并返回 start TS；Rust 同名语义入口导入已有 `TableStats` 内容。Rust 搜索也未发现它们接入 Go 对应的 DDL 调用链。

因此扩展或修复时应以具体函数契约比较，不应仅凭相似名称认定一一等价。

## 扩展指南

- 修改 ANALYZE 的 snapshot/计数合并规则时，优先改 `save_analyze_result_to_storage`，同时扩充 `save_test.rs` 的旧 snapshot、普通 v2、v1、MV/全局索引和 `analyze_snapshot` 分支测试；还应验证 `StatsReadWriter::save_analyze_result` 对返回 `0` 的历史记录抑制。
- 新增或改变直方图附属表时，在 `save_column_or_index_stats_at_version` 中保持“清旧数据—写新数据”的完整顺序，并同步读取/GC 路径；新增回归测试应放在独立的 `save_test.rs` 或 `stats_read_writer_test.rs`，不要内嵌到生产文件。
- 调整批处理时同时检查 `save_top_n` 和 `save_buckets` 的进展不变量、首个超大元组行为、桶 ID 连续性，以及 `MAX_INSERT_LENGTH` 按字节/字符计数的兼容风险。
- 若要继续对齐 Go DDL API，不应把完整 DDL 逻辑硬塞入现有简化函数；先明确 `SqlStore` 是否能表达事务、受影响行数、Datum 转换和上下文取消，再分别设计新增列占位统计与整表 schema 初始化契约。
- 若要对齐 FMSketch 行为，需要为 Rust 输入补充“是否分区统计”的可靠信息，并新增普通表不写、分区表写入、旧值始终清理的测试。
- 性能风险主要是逐对象多条 DELETE/INSERT 和字符串构造；兼容风险集中在系统表列、stats version、历史 meta 版本语义以及 Go/Rust API 名称相似但契约不同。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，目标目录的 `save.rs`、`save_test.rs`、`stats_read_writer.rs`、`stats_read_writer_test.rs`、`save.go` 均已索引。
- RustCodeGraph `node --file pkg/statistics/handle/storage/save.rs --offset 1 --limit 400`：核对完整 344 行源文件、全部常量和八个函数，以及该文件被三个相邻 Rust 文件使用。
- RustCodeGraph `node`：读取 `pkg/statistics/handle/storage/lib.rs`、`stats_read_writer.rs`、`save_test.rs`、`stats_read_writer_test.rs` 和完整 `save.go`，核对模块导出、数据结构、生产入口、测试断言与 Go 分支。
- 精确符号查询 `query save_analyze_result_to_storage`、`query save_column_or_index_stats_at_version`、`query insert_table_stats_to_kv`、`query save_meta_to_storage`，并用仓库 `rg` 消除同名 Go/Rust符号歧义，确认当前可见调用点。
- Cargo 边界：`pkg/statistics/handle/storage/Cargo.toml`、`pkg/statistics/handle/Cargo.toml`、`pkg/statistics/Cargo.toml`；确认独立 crate、父 crate path dependency 及永假迁移依赖区。
- 人工复核范围：文档只陈述可由上述源码、调用点、manifest 和测试验证的当前行为；没有把 Go 已接线能力或事务原子性推断为 Rust 现状。
