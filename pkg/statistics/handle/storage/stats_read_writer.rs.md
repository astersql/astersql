# `pkg/statistics/handle/storage/stats_read_writer.rs` 逻辑说明

## 文件定位

该文件是 `astersql-statistics-handle-storage` crate 的共享数据模型、系统表访问抽象和统计读写门面。crate 由 `pkg/statistics/handle/storage/Cargo.toml` 定义，`lib.rs` 通过 `pub mod stats_read_writer` 装载并以 `pub use stats_read_writer::*` 再导出本文件符号；同一 crate 的 `read.rs`、`save.rs`、`update.rs`、`gc.rs` 都依赖这里的类型或 trait。

它位于统计对象与 `mysql.stats_*` 系统表之间：`SqlStore` 把实际 SQL 执行器抽象成 `start_ts`/`execute`，`StatsHandler` 提供 store、lease 和历史 meta 记录回调，`StatsReadWriter` 再把保存 ANALYZE、刷新版本、读取 meta 和装载表统计组合成较稳定的上层接口。当前 `Cargo.toml` 的路径依赖全部位于恒假条件 `[target.'cfg(any())'.dependencies]` 下，因此当前实现实际只直接依赖标准库和本 crate；不能据此声称它已直接接入这些外部 crate。

## 核心职责

本文件承担四组职责：

1. 定义 `Error`、`Value`、`Row` 以及 bucket、histogram、列统计、表统计和 ANALYZE 结果等跨子模块共享的数据结构。
2. 以 `SqlStore` 统一统计 SQL 执行，并提供 `gc.rs` 所需的默认 GC 查询、删除、历史清理和水位读写实现。
3. 以 `StatsReadWriter` 将 `save.rs`、`read.rs`、`update.rs` 的核心能力与 `StatsHandler` 的历史记录/lease 语义连接起来，并维护按物理表 ID 索引的内存加载缓存。
4. 以 `sql_bytes` 把二进制统计载荷编码为 SQL 十六进制字面量，供 `save.rs` 写 bucket 边界、TopN、CM Sketch 和 FM Sketch。

该文件不是 Go `statsReadWriter` 的完整等价实现：Rust 当前只有五个门面方法，Go 文件中的单列保存、批量 meta 保存、分区加载、按需直方图加载、JSON/历史导出导入等仍不在本结构体上。

## 主要符号

- `Error(pub String)`：crate 内轻量错误，`Display` 原样输出字符串并实现 `std::error::Error`；没有错误类型枚举或 source 链。
- `Value` / `Row`：SQL 结果的简化 datum 与行包装。`Row::{int,uint,bytes,text}` 按列下标转换；缺列或类型不匹配返回零值，`text` 使用有损 UTF-8。`Int`/`UInt` 间转换使用 Rust `as`，不做范围检查。
- `SqlStore: Send + Sync`：必须由后端实现 `start_ts` 和 `execute`；其余九个 `gc_*`/水位方法有默认 SQL 实现。`gc_histogram_identities` 会合并 histogram 与 FM Sketch 身份并排序去重，避免 histogram 行已先删除时遗留孤儿 sketch。
- `Bucket`、`Histogram`、`TopNItem`、`ColumnStats`、`TableStats`：统计信息的内存表示。`TableStats.columns`/`indices` 以字符串键保存，当前 `read.rs` 用十进制 `hist_id` 字符串作为键。
- `AnalyzeResults`：一次待保存 ANALYZE 的表 ID、snapshot、行数基线、修改计数基线、统计版本、MV/全局索引标志和列/索引统计集合；`columns` 元组首项用布尔值区分索引。
- `StatsHandler: Send + Sync`：提供 `Arc<dyn SqlStore>`、lease 以及 `record_historical_stats_meta(version, source, analyze, id)` 回调。
- `StatsReadWriter` / `new_stats_read_writer`：拥有 handler，并以 `Mutex<HashMap<i64, TableStats>>` 保存最近加载的表统计；构造函数总是从空缓存开始。
- `StatsReadWriter::{change_global_stats_id,update_stats_meta_version_for_gc,save_analyze_result,stats_meta_count_and_modify_count,table_stats_from_storage}`：当前公开门面全集。
- `sql_bytes`：crate 内可见的二进制 SQL 编码器，输出小写十六进制 `x'..'`；空切片输出 `x''`。

文件没有常量、条件编译项或内嵌测试模块；独立测试由 `lib.rs` 在 `cfg(test)` 下装载 `stats_read_writer_test.rs`。

## 执行流程

`save_analyze_result` 是写入主流程。它先记录 `Instant`，调用 `save_analyze_result_to_storage(store, results, analyze_snapshot)`。该下游函数取 `start_ts`，锁读当前 meta，拒绝覆盖相等或更新的 v2 snapshot，更新/替换 `stats_meta`，再逐项写 histogram、TopN、bucket、FM Sketch 和列分析时间；返回 0 表示无需记录新的表级历史 meta。若返回非零版本，handler lease 为正且总耗时达到 lease 的一半，门面再次调用 `update_stats_meta_version_and_last_histogram_version` 推进 meta 版本，避免其他节点错过增量刷新。二次推进失败会被转换成稳定的公共错误文本。最终版本非零时，以调用方传入的 `source` 和 `analyze=true` 记录历史 meta。

`update_stats_meta_version_for_gc` 调用同一版本更新函数；版本非零才用固定 source `schema_change`、`analyze=false` 记录历史。`change_global_stats_id` 委托 `update.rs::change_global_stats_id`，顺序改写六张统计系统表中的 `table_id`。

读取侧中，`stats_meta_count_and_modify_count` 委托 `read.rs` 并固定 `for_update=false`，丢弃“meta 行是否缺失”的第三返回值，所以无行时对调用者表现为 `(0, 0)`。`table_stats_from_storage` 先短时加锁并克隆同 ID 缓存作为加载基线，释放锁后调用 `read.rs::table_stats_from_storage`；后者按 snapshot 读取 meta，若 meta 存在会清空基线中的列/索引后完整重建，避免 DDL 已删除对象残留。成功后再次加锁写回克隆，并返回新表统计；下游失败时缓存不变。

`SqlStore` 的 GC 默认流程由 `gc.rs` 调用：扫描版本窗口，枚举并去重 histogram/FM Sketch 身份，根据表、列和索引是否仍存在选择整表删除、meta 删除或单 histogram 删除；历史清理分别按 1000 行 meta-history 和 50 行 stats-history 分批，水位保存在 `mysql.tidb`。

## 数据与状态

持久状态是 `mysql.stats_meta`、`stats_histograms`、`stats_buckets`、`stats_top_n`、`stats_fm_sketch`、`column_stats_usage`、`analyze_options`、`stats_table_locked`、`stats_history`、`stats_meta_history` 和 `mysql.tidb` 中的行。目标文件直接拼接 GC SQL；保存、读取和 ID 变更的具体 SQL分别位于 `save.rs`、`read.rs` 和 `update.rs`。

内存状态集中在 `StatsReadWriter.cached_tables`。缓存键仅是 `table_id`，snapshot 不参与键；它不是带版本淘汰策略的全局 stats cache，而是为下一次装载提供可克隆的基线。`TableStats`/`ColumnStats` 和内部 `Vec<u8>` 在读写缓存时会深度克隆，换取锁外执行存储 I/O。

版本来自 `SqlStore::start_ts`，用于统计持久化、GC 删除及跨节点刷新。`AnalyzeResults.snapshot` 则是防止旧 ANALYZE 回退 meta 的独立比较值。`StatsReadWriter` 不保存事务对象；多条 SQL 是否在同一事务内由具体 `SqlStore` 实现和调用环境决定。

## 依赖与调用关系

- 模块关系：`lib.rs` 再导出本文件，因此 `read.rs`/`save.rs`/`update.rs`/`gc.rs` 可通过 `crate::{...}` 使用其类型和 trait。
- 写路径：`StatsReadWriter::save_analyze_result` → `save.rs::save_analyze_result_to_storage` → `SqlStore::{start_ts,execute}`；慢写分支再调用 `update_stats_meta_version_and_last_histogram_version`，成功后调用 `StatsHandler::record_historical_stats_meta`。
- 读路径：`StatsReadWriter::table_stats_from_storage` → `read.rs::table_stats_from_storage` → histogram、TopN/CMSketch、FM Sketch 读取辅助函数 → `SqlStore::execute`。
- ID/版本路径：两个门面分别调用 `update.rs::{change_global_stats_id,update_stats_meta_version_and_last_histogram_version}`。
- GC 路径：`gc.rs::{gc_stats,delete_table_stats_from_kv,clear_outdated_history_stats}` 调用 `SqlStore` 的默认 GC 方法；`gc_clear_expired_history` 还调用 `gc.rs::batch_count`。
- SQL 字节编码：`save.rs::{save_top_n,save_buckets,save_column_or_index_stats_at_version}` 调用 `sql_bytes`。
- 测试接线：`stats_read_writer_test.rs` 直接构造 mock `SqlStore`/`StatsHandler` 并调用 `new_stats_read_writer`、保存下游与读取下游。精确 Rust 引用搜索未发现 `StatsReadWriter` 门面在生产文件中的构造或方法调用，因此当前可确认的生产复用主要是共享类型/`SqlStore`/`sql_bytes`，不能把 Go 主链调用者直接当成 Rust 已接线事实。

RustCodeGraph 对重名方法的名称级 callers 查询未能消歧；上述关系由目标节点的 callee 边、已索引相邻源码和精确引用检索共同核对。

## 错误处理与边界

所有 store 和 handler 组合操作以 `Result<_, Error>` 返回，通常用 `?` 原样传播首个错误。唯一主动改写错误的是 `save_analyze_result` 的慢写二次版本推进：它隐藏底层错误，返回与 Go 公共行为一致的重试提示；独立测试 `slow_analyze_version_refresh_uses_canonical_error` 固定了该字符串。

`Row` 转换是宽松边界：缺列、`Null` 或类型不匹配静默变成 0/空字节，`text` 对非法 UTF-8 有损替换。这使 SQL 适配器简单，但也可能把结果 schema 漂移伪装成零值；扩展查询列时必须同步列序和测试。

GC SQL 通过 `format!` 拼接数字和内部表名；`gc_timestamp`/`set_gc_timestamp` 还直接插入 `variable_name`。当前调用者使用常量 `tidb_stats_gc_last_ts`，若未来接受外部字符串，需先增加参数化或转义。默认 GC 方法没有自动回滚：中途 `execute` 失败时，之前语句是否回滚取决于 store 提供的事务边界。

缓存锁使用 `expect("stats cache mutex poisoned")`，锁毒化会 panic，而不是返回 `Error`。加载失败不覆盖缓存；meta 行不存在时，下游会返回已有缓存（若有），这是当前兼容行为，不等价于“确认表无统计”。

## 并发与资源生命周期

`SqlStore` 和 `StatsHandler` 均要求 `Send + Sync`，handler 由 `Arc` 共享。`StatsReadWriter` 自身不启动线程、任务或通道；其资源生命周期由最后一个 handler `Arc` 和 writer 所有权决定。

缓存只在读取基线和插入结果时各持锁一次，实际 SQL I/O 在锁外执行，因此慢存储不会长期阻塞其他缓存访问。同一 table ID 的并发加载可能都读取相同旧基线并并行访问存储，最后完成者覆盖缓存；文件没有 per-key 单飞、snapshot 新旧比较或 compare-and-swap。调用方若允许不同 snapshot 并发，必须接受“最后写入缓存者获胜”，而每个调用仍返回自己加载的结果。

`save_analyze_result` 用墙钟 `Instant` 只测本次同步调用耗时；lease 为零时禁用慢写判断。`SqlStore` 的 GC 删除和清理循环均串行执行，目标文件不提供并行度控制。多语句事务、连接借还和取消均不由这些 trait 显式建模。

## 与 Go 版本的对应关系

Rust `StatsReadWriter` 对应 `stats_read_writer.go::statsReadWriter`，`StatsHandler` 对应 Go 的 `statstypes.StatsHandle` 中本文件实际需要的子集，`SqlStore` 则把 Go 中 `util.CallWithSCtx` 获得的 session/context 和直接系统表 SQL进一步抽象为可测试接口。

已对齐的核心意图包括：全局统计 ID 改写；schema change 后更新 meta 版本并记录历史；保存 ANALYZE 后记录历史；保存超过 lease 窗口时再次推进版本；读取 count/modify_count；以既有缓存为装载基线。慢保存二次更新失败的公共错误文本也由 Rust 测试和 Go `TestFailedToHandleSlowStatsSaving` 对齐。

需要明确的差异如下：

- Go 通过 `util.CallWithSCtx(..., FlagWrapTxn)` 显式借 session 并包事务；Rust trait 本身不声明事务边界。
- Go 的慢写阈值使用 `cache.LeaseOffset * lease`、带日志和 failpoint；Rust固定使用 `lease / 2`，没有日志/failpoint，测试以 2ns lease 触发分支。
- Go `TableStatsFromStorage` 接受 `TableInfo`、`loadAll` 和 lease，并从 handle cache 取基线；Rust只接收 table ID/snapshot，使用 writer 私有缓存，也无法返回“缺少 meta”的 `nil` 语义。
- Rust 的统计数据结构和 `Value`/`Row` 是简化模型；Go 使用完整 `statistics.Table`、`types.Datum`、session context 与 schema 元数据。
- Go `StatsReadWriter` interface 还有大量本文件 Rust 结构体未实现的方法；仓库另有 `pkg/statistics/handle/types/interfaces.rs::StatsReadWriter`，其 PascalCase trait 与这里的结构体没有已确认的 impl 接线。
- Rust `SqlStore` 集成了 Go `gc.go`/存储会话层的一部分职责，这是为了让同 crate 的 `gc.rs` 复用，并非 Go `statsReadWriter` 结构体自身的逐方法映射。

Go 测试覆盖真实 SQL/域集成和 failpoint；相邻 Rust 测试主要是 mock SQL 语句与错误行为单测，不能据此宣称两侧集成覆盖等价。

## 扩展指南

新增读写门面方法时，应先判断逻辑属于 `read.rs`、`save.rs`、`update.rs` 或 `gc.rs`，把系统表算法留在相应文件，本文件只负责 handler/lease/history/cache 组合；同时在独立 `stats_read_writer_test.rs` 增加 mock store/handler 回归，禁止把测试写入生产源文件。

扩展 `Value`/`Row` 时要保持 SQL 列类型和列序一致，并测试空行、`Null`、有符号/无符号越界和非法 UTF-8。若把宽松零值改成错误返回，将影响所有 `SqlStore` 默认 GC 方法及 read/save 路径，属于兼容性变更。

扩展缓存时需明确 key 是否包含 snapshot、是否允许旧 snapshot 覆盖新结果、何时失效以及并发同 key 的顺序语义。若增加等待、锁或异步任务，还需避免在持有 `cached_tables` 锁时调用 store/handler，并增加并发独立测试。

新增统计载荷或系统表时，要同时检查 `SqlStore::{gc_histogram_identities,gc_delete_table_stats,gc_delete_histogram}`、`update.rs::CHANGE_GLOBAL_STATS_TABLES`、保存和读取路径，防止 ID 迁移或 GC 遗漏。多语句一致性若有要求，应在具体 store 提供明确事务封装，而不是假设 trait 默认方法天然原子。

继续对齐 Go 时必须逐项保留真实语义，优先补齐类型/handler trait 的正式接线和事务上下文，再移植额外方法；同步参考 `stats_read_writer.go`、`types/interfaces.go` 和对应 Go 测试，不能用简化桩替代。性能重点是大载荷克隆、逐条/分批 SQL 数量、缓存锁竞争和慢写版本刷新频率。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/storage` 确认目标、相邻模块、Go 对照和独立测试；`node --file` 完整读取 `stats_read_writer.rs` 415 行、`stats_read_writer_test.rs` 229 行和 `lib.rs` 50 行。
- 符号/调用证据：查询 `StatsReadWriter`、`new_stats_read_writer`、`save_analyze_result`、`table_stats_from_storage` 及 callers/callees；callee 图确认保存门面调用 handler 的 `store`/`lease`/`record_historical_stats_meta`，读取门面调用 store。重名导致 callers 不完整，已用精确 Rust 引用搜索核对生产/测试接线。
- crate 与下游实现：`pkg/statistics/handle/storage/Cargo.toml`、`lib.rs`、`read.rs::{stats_meta_count_and_modify_count,table_stats_from_storage}`、`save.rs::save_analyze_result_to_storage`、`update.rs::{change_global_stats_id,update_stats_meta_version_and_last_histogram_version}`、`gc.rs` 对 `SqlStore` 默认方法的调用。
- Go 对照：`pkg/statistics/handle/storage/stats_read_writer.go`、`stats_read_writer_test.go` 和 `pkg/statistics/handle/types/interfaces.go`；Rust 接口现状另核对 `pkg/statistics/handle/types/interfaces.rs`。
- Rust 测试：`stats_read_writer_test.rs::{analyze_save_updates_histogram_auxiliary_rows,table_stats_load_drops_histograms_missing_from_storage,slow_analyze_version_refresh_uses_canonical_error}`，以及该文件中的 JSON 兼容用例。测试证明持久化附属行、完整加载清除已删 histogram 和慢写公共错误；它没有覆盖所有 `SqlStore` GC 默认方法或 `StatsReadWriter` 的全部门面。
- 未运行 Cargo 或代码测试：任务与总计划明确这是纯文档分析且禁止 Cargo；完成判断使用源码/调用边人工复核和任务规定的 11 章节结构验证。
