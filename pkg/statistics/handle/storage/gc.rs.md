# `pkg/statistics/handle/storage/gc.rs` 逻辑说明

## 文件定位

该文件是 `astersql-statistics-handle-storage` crate 的统计系统表垃圾回收实现。crate 边界由 `pkg/statistics/handle/storage/Cargo.toml` 定义，`pkg/statistics/handle/storage/lib.rs` 通过 `pub mod gc` 装载并用 `pub use gc::*` 再导出公开符号。这里的 GC 清理 `mysql.stats_*` 中已删表、列、索引及过期历史统计，不是 TiKV MVCC GC。

Rust 应用中的直接上游是 `pkg/domain/domain.rs::Domain::gc_stats`：它先调用 `stats_context().gc_dropped_stats()` 处理已知 drop ID，再以 `stats_storage::new_stats_gc(self.stats_store.as_ref(), self)` 构造本文件的门面。`Domain` 同时实现 `StatsCatalog`，而 `KvStatsStore` 在 `pkg/statistics/handle/restricted_sql.rs` 实现 `SqlStore`，因此本文件处在 schema 视图与统计系统表访问的交界处。

`Cargo.toml` 的 Go 包映射是 `pkg/statistics/handle/storage`。其大量路径依赖位于恒假的 `[target.'cfg(any())'.dependencies]`，当前本文件实际只直接使用标准库和 crate 内的 `Error`/`SqlStore`；不应根据该依赖清单声称本文件已直接接入那些 crate。

## 核心职责

本文件承担五项职责：

1. 以持久化水位 `tidb_stats_gc_last_ts` 和“较大 lease 的十倍”安全偏移构造增量扫描窗口 `(last_gc, gc_version)`。
2. 对窗口中的每个物理表 ID，区分整表已删和仅列/索引已删，再删除直方图、bucket、TopN、FM Sketch、列使用记录等附属数据。
3. 对已删表实施两阶段回收：首次保留 `stats_meta` 并用新版本通知其他节点，后续扫描在已无直方图载荷时才删 meta。
4. 删除已删物理表的历史统计，并尽力清理超过保留期的全局历史行。
5. 提供严格单调的 TiDB TSO 刻度进程时间戳 `current_ts` 和向上取整批次计算 `batch_count`，供存储适配层及其他统计路径复用。

## 主要符号

- `GC_LAST_TS_VARIABLE: &str`：水位键名 `tidb_stats_gc_last_ts`，由 `get_last_gc_timestamp` 和 `write_gc_timestamp` 共用。
- `StatsCatalog`：公开 trait，用 `lease()` 给出统计 lease，用 `table_exists(physical_id)` 和 `histogram_exists(physical_id, histogram_id, is_index)` 提供当前 schema 存在性。`Domain` 的实现同时检查逻辑表 ID 和分区 ID，并按 `is_index` 分流检查 `Indices` 或 `Columns`。
- `StatsGc<'a>` / `new_stats_gc`：以借用方式绑定 `&dyn SqlStore` 和 `&dyn StatsCatalog` 的轻量门面；不拥有资源，三个公开方法仅转调同名文件级函数。
- `gc_stats(store, catalog, ddl_lease)`：一轮增量 GC 的核心入口，成功返回前推进水位。
- `delete_table_stats_from_kv(store, stats_ids, soft)`：用一个 `start_ts` 处理整批 ID。`soft=true` 由 `SqlStore` 保留直方图行但清零其统计值；`soft=false` 删除直方图行和附属载荷。
- `batch_count(total, batch)`：计算 `ceil(total / batch)`；`total <= 0` 或 `batch <= 0` 时返回 0，避免除零和无效循环。
- `clear_outdated_history_stats(store, retention)`：把保留期转成整秒后委托 `SqlStore::gc_clear_expired_history`。
- `gc_table_stats`：私有表级决策器，是“已删整表”与“存活表中的孤儿直方图”的分支点。
- `gc_history_stats_from_kv` / `delete_histogram_stats_from_kv`：分别删除某表的两类历史表，以及某个列/索引的直方图载荷；后者每次先取新 `start_ts`。
- `get_last_gc_timestamp` / `write_gc_timestamp`：水位读写封装。缺失值视为 0，非法十进制字符串转成带上下文的 `Error`。
- `current_ts`：将 Unix 物理毫秒左移 18 位，再借助进程级 `AtomicU64` 保证同毫秒和时钟回退时仍严格递增。
- `duration_to_ts`：私有刻度转换函数，先把毫秒截断到左移 18 位不溢出的上限。

本文件没有枚举、条件编译项或内嵌测试模块；独立测试由 `lib.rs` 在 `cfg(test)` 下装载 `gc_test.rs`。

## 执行流程

`gc_stats` 的主流程如下。

1. 取 `max(catalog.lease(), ddl_lease)`，用 `saturating_mul(10)` 扩大为安全窗口，再经 `duration_to_ts` 转成 TSO 刻度偏移。
2. 调用 `store.start_ts()` 获取存储事务时钟并 `saturating_add(1)`。`SqlStore::gc_meta_ids` 的上界是排他的，加一使当前 store 版本刚提交的 meta 行也能入窗。若 `now < offset`，无可安全回收区间，直接成功返回且不写水位。
3. 计算 `gc_version = now - offset`，读取 `last_gc`，然后调用 `gc_meta_ids(last_gc, gc_version)`。默认 SQL 语义是 `version >= last_gc AND version < gc_version`；水位边界重叠不会漏行，但边界版本可能被幂等地重新检查。
4. 对每个 ID 运行 `gc_table_stats`。若表不存在且查到任何 histogram/FM-sketch identity，先 hard-delete 表统计并保留 meta；若载荷已空，直接删 meta。若表存在，逐个删除 schema 中已不存在的列/索引载荷。
5. `gc_table_stats` 成功后再次检查表存在性；已删表调用 `gc_delete_history` 同时删除 `stats_history` 和 `stats_meta_history`。
6. 全部 ID 处理成功后，尝试用固定 7 天保留期清理过期历史。这一步的错误被显式丢弃，不阻止主 GC 成功。
7. 最后才把 `gc_version` 写入水位；任何主扫描或主删除错误都会在此前返回，不会跳过失败窗口。

## 数据与状态

持久状态主要是 `mysql.tidb` 中的 GC 水位以及 `mysql.stats_meta`、`stats_histograms`、`stats_buckets`、`stats_top_n`、`stats_fm_sketch`、`column_stats_usage`、`analyze_options`、`stats_table_locked`、`stats_history` 和 `stats_meta_history` 中的行。具体 SQL 不在 `gc.rs` 中硬编排，而由 `SqlStore` 默认方法实现。

`StatsGc` 只保存两个共享借用，没有可变字段。`gc_stats` 的扫描窗口和一批删表操作分别从 `start_ts` 取版本；孤儿直方图每次删除也单独取版本，用来刷新 `stats_meta.version` 和 `last_stats_histograms_version`。

`current_ts` 的唯一内存可变状态是函数内的静态 `AtomicU64`。其数值是进程内的 TSO 样式时间戳，并不包含 PD 全局逻辑时钟协调；`KvStatsStore::start_ts` 会取它与存储 `current_version().Ver` 的较大值。

## 依赖与调用关系

- 模块装配：`storage/lib.rs` 公开模块并再导出符号，在 `cfg(test)` 下装载独立 `gc_test.rs`。
- 上游主链：`Domain::gc_stats` → `new_stats_gc` → `StatsGc::gc_stats` → `gc_stats`。`Domain::clear_outdated_history_stats` 通过同一门面直接调用历史清理。
- schema 依赖：`Domain as StatsCatalog` 从其 `RwLock` 保护的 `stats_catalog` 检查表、分区、列和索引。
- 存储依赖：`SqlStore::{gc_meta_ids,gc_histogram_identities,gc_delete_table_stats,gc_delete_meta,gc_delete_histogram,gc_delete_history,gc_clear_expired_history,gc_timestamp,set_gc_timestamp}`。`gc_histogram_identities` 合并 histogram 与 FM Sketch 身份并去重，因此即使 DDL 已提前删 histogram 行，本路径仍能回收孤儿 FM Sketch。
- 时间戳的其他生产调用者：`pkg/statistics/handle/restricted_sql.rs::{KvStatsStore::start_ts,current_stats_timestamp}` 以及 `pkg/domain/domain.rs::insert_col_stats_2_kv`。
- `batch_count` 的直接生产消费者是 `stats_read_writer.rs::SqlStore::gc_clear_expired_history`，分别以 1000 和 50 为批次删两类历史表。
- 测试边：本 crate 的 `gc_test.rs` 直接测 `batch_count`、`current_ts` 和扫描时钟；`pkg/statistics/handle/handletest/**` 通过 `Domain::gc_stats` 覆盖更高层表/分区/锁定统计语义。

RustCodeGraph 能索引目标文件的 25 个符号并给出 `StatsGc <- new_stats_gc` 的实例化边；对本文件 snake_case 函数的名称级 callers/callees 查询未返回跨文件边，上述其他关系由已索引的相关文件片段和精确 `rg` 引用补证。

## 错误处理与边界

除两个纯计算函数外，所有存储操作都返回 `Result<_, Error>` 并使用 `?` 保留首个错误。主扫描的取时钟、读水位、查 ID、表/直方图/历史删除或最终写水位任一失败都会向上返回。只有主扫描后的七天历史保留清理是 best-effort，其错误被 `let _ = ...` 丢弃。

水位缺失从 0 开始；水位非法时明确失败，不会悖然重扫。`now < offset` 防止无符号减法下溢，lease 乘法、`start_ts + 1` 和原子时间戳递增都使用饱和算术。`duration_to_ts` 还在左移前截断超大时长。

一个容易误读的边界是已删表两阶段流程：有 histogram/FM-sketch identity 时 hard delete 不删 `stats_meta`；只有后续扫描观察到载荷为空才删 meta。Go/Rust 分区测试均记录了一个已知边界：逻辑表的 meta-only 行在 drop 后可能不再落入常规版本窗口（`FIXME #68076`）。

`gc.rs` 通过 trait 方法串行发出操作，自身没有声明跨多条 SQL 的事务边界。Go 实现对公开批量删除和某些内部删除使用 `FlagWrapTxn`/`WrapTxn`；不能仅凭 Rust 门面就声称它具有相同原子性。

## 并发与资源生命周期

`StatsGc` 不启动任务、不建立通道，也不持有会话或事务守卫；它的生命期由 store/catalog 借用限定，方法返回后没有本文件所有的后台资源。`SqlStore: Send + Sync` 允许实现被线程间共享，但单轮 `gc_stats` 内的 ID 和 histogram 循环是顺序执行的。

`Domain as StatsCatalog` 每次查询会取 `stats_catalog` 的读锁，锁毒化时会 panic；这是 `Domain` 适配层的并发约束，不是 `gc.rs` 的可恢复 `Error` 路径。

`current_ts` 用 `AtomicU64::fetch_update(Ordering::AcqRel, Ordering::Acquire, ...)` 在并发首次/后续调用间竞争，每个成功调用返回 `max(physical, previous + 1)`。`gc_test.rs::current_ts_is_strictly_monotonic_under_concurrency` 以 8 个线程各生成 256 个值，排序后验证 2048 个值无重复且严格递增。该原子状态存活至进程结束，无显式释放或重置入口。

## 与 Go 版本的对应关系

Rust `StatsGc`/`StatsCatalog` 是 Go `statsGCImpl` 绑定 `types.StatsHandle` 与 `infoschema.InfoSchema` 的拆分适配。`gc_stats`、`delete_table_stats_from_kv`、`clear_outdated_history_stats`、`gc_history_stats_from_kv`、`delete_histogram_stats_from_kv`、`gc_table_stats` 和水位读写，分别对应 `gc.go` 中同名或 camel-case 符号。十倍最大 lease、版本窗口、表的两阶段删除、列/索引存在性判定、水位只在主流程成功后推进，以及历史保留清理尽力而为，均保留了 Go 意图。

已确认的实现差异如下：

- Go 用 `oracle.GoTimeToTS(time.Now())` 得到墙钟上界；Rust 用 `store.start_ts()?.saturating_add(1)`，直接与后端事务版本时钟对齐，并因排他上界增加一个逻辑 tick。`gc_uses_the_store_transaction_clock_for_its_scan_window` 专门验证这一点。
- Go 的历史保留期来自 `vardef.HistoricalStatsDuration`；Rust `StatsGc::clear_outdated_history_stats` 允许上游传入，但 `gc_stats` 内部尽力清理固定为 7 天。
- Go `forCount` 假定正批大小；Rust `batch_count` 对非正参数显式返回 0。
- Go 记录过期历史清理警告；Rust 目标文件直接忽略该错误，没有日志依赖。
- Go 删除路径显式包装事务；Rust 由 `SqlStore` 语义承载多条 SQL，`gc.rs` 本身没有事务封装。
- Rust `Domain::gc_stats` 额外先处理 `dropped_stats_ids`，以免已知 drop 因持久化水位窗口而延后；这是 Rust 应用层必需一起考虑的局部接线。
- `current_ts` 是 Rust 存储适配的进程内工具，不是 `gc.go` 中的独立对应函数。

`gc_test.rs` 前半的 Go 风格集成用例被放在 `_GO_DRAFT_ARCHIVE` 原始字符串中，当前不会被 Rust 测试运行器编译或执行。它们与 `gc_test.go::{TestGCStats,TestGCPartition,TestGCColumnStatsUsage,TestExtremCaseOfGC}` 一致，只能作为迁移意图证据；当前真正可运行的相邻 Rust 测试是文件末尾的三个用例。

## 扩展指南

修改扫描窗口时，首先保持三个不变量：上界的排他语义、只在所有主操作成功后推进水位、以存储版本时钟避免漏扫同毫秒提交。最可能修改 `gc_stats`/`duration_to_ts`，并应在独立 `gc_test.rs` 中扩展存储替身，覆盖上界、水位和错误后不推进。

新增要回收的统计载荷时，通常应扩展 `SqlStore::gc_delete_table_stats` 和/或 `gc_delete_histogram`，而非把 SQL 插入 `gc.rs`；若载荷可在 histogram 行提前消失后成为孤儿，还要更新 `gc_histogram_identities` 的身份合并逻辑。测试应证明整表 hard/soft 删除、列与索引分流、分区 ID 和幂等重试。

改变两阶段删表或 `Domain` 的 drop 预处理时，需同时检查其他 TiDB 节点感知 `stats_meta.version`/`last_stats_histograms_version` 的兼容契约，不得为减少扫描次数而直接合并两阶段。这类变更还应覆盖 `handletest` 中的表、分区和 lockstats 用例。

修改 `current_ts` 时必须保持全并发调用严格递增、左移 18 位的刻度兼容和饱和边界；同步扩展 `current_ts_is_strictly_monotonic_under_concurrency`。修改历史批次时则需同步 `batch_count` 测试和 `SqlStore::gc_clear_expired_history`，并评估单次 SQL 锁持有时间、日志量和大表删除性能。

所有 Rust 测试逻辑应继续放在独立 `gc_test.rs` 或相关 `handletest` 文件，不应内嵌回生产 `gc.rs`。若要把 `_GO_DRAFT_ARCHIVE` 中的用例变为可运行 Rust 回归，必须用真实 Rust API 重写并删除对应草稿，不能把字符串归档当成覆盖率。

## 验证依据

- RustCodeGraph 索引：`status` 确认 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/storage` 确认目标、入口、Go 对照和独立测试；`node --file` 读取了 `gc.rs` 全 204 行、`gc_test.rs` 全 257 行、`gc.go` 全 362 行和 `gc_test.go` 全 159 行。
- RustCodeGraph 符号证据：`query StatsGc` 同时找到 Rust `StatsGc`/`new_stats_gc` 和 Go `statsGCImpl`/`NewStatsGC`；`node StatsGc` 确认 `new_stats_gc` 实例化边。`query current_ts`/`query batch_count` 确认相邻测试和生产消费者。名称级 callers/callees 对 snake_case 函数未产生跨文件输出，已用精确引用检索补证。
- crate 与存储边界：`pkg/statistics/handle/storage/Cargo.toml`、`storage/lib.rs`、`storage/stats_read_writer.rs::SqlStore` 及其全部 GC 默认方法。
- Rust 上游和适配：`pkg/domain/domain.rs::{impl StatsCatalog for Domain,Domain::gc_stats,Domain::clear_outdated_history_stats,insert_col_stats_2_kv}`，以及 `pkg/statistics/handle/restricted_sql.rs::{impl SqlStore for KvStatsStore,current_stats_timestamp}`。
- Go 语义对照：`pkg/statistics/handle/storage/gc.go`，以及 `gc_test.go::{TestGCStats,TestGCPartition,TestGCColumnStatsUsage,TestExtremCaseOfGC}`。
- Rust 测试现状：`gc_test.rs::{canonical_stats_gc_batch_count_covers_empty_exact_and_partial_batches,current_ts_is_strictly_monotonic_under_concurrency,gc_uses_the_store_transaction_clock_for_its_scan_window}` 是当前可运行用例；同文件 `_GO_DRAFT_ARCHIVE` 只是不可执行迁移草稿。`pkg/statistics/handle/handletest/handle_test.rs` 与 `handletest/lockstats/*_test.rs` 提供更高层 `Domain::gc_stats` 回归路径。
- 未运行 Cargo 或代码测试：本任务只生成文档，总计划和任务文件明确禁止 Cargo；完成依据为上述源码/调用边事实复核与任务规定的 11 章节结构验证。
