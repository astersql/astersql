# `pkg/statistics/handle/history/history_stats.rs`

## 文件定位

该文件是 `astersql-statistics-handle-history` crate 中的历史统计基础实现，由同目录 `lib.rs` 的 `pub mod history_stats` 声明并整体重导出。crate 边界由 `pkg/statistics/handle/history/Cargo.toml` 定义：库入口是 `lib.rs`，Go 对应包是 `pkg/statistics/handle/history`。该 manifest 中原 Go 实现所需的 session/storage/model 等依赖全部放在 `target.'cfg(any())'.dependencies` 下，因而当前 Rust 文件通过本地 trait 抽象隔离具体 SQL 和 session 类型，自身不直接连接数据库。

在当前 Rust 应用中，该 crate 已被 `pkg/statistics/handle/Cargo.toml` 作为普通依赖引入，但对外实际复用的主要是 `HistoricalStatsMeta` 和 `MAX_COLUMN_SIZE`：`pkg/statistics/handle/handle.rs` 重导出前者，并在历史 JSON 分块时使用后者。本文件的 `StatsHistory` 门面目前仅在同 crate 的独立单元测试中构造；完整应用的 ANALYZE 路径现由 `pkg/session/runtime/statistics.rs` 调用 `DomainStatsContext::record_historical_stats_to_storage`，后者在 `pkg/domain/domain.rs` 中直接协调当前 `Handle` 与 `stats_store`。因此，本文件是可测的迁移语义核心和共享类型/常量边界，不应误述为当前所有生产请求的直接运行入口。

## 核心职责

- 用 `HistoryStore` 封装历史统计开关、当前 meta 读取、历史 meta 替换和统计块插入，使算法不依赖具体 SQL session。
- 用 `StatsSnapshot` 封装快照导出、表统计初始化判定和按大小分块，将“生成数据”与“持久化数据”分开。
- 由 `StatsHistory` 组合两个 trait object，提供单表/分区快照落盘、批量 meta 记录和开关查询三个高层操作。
- 保持 Go `history_stats.go` 的关键持久化语义：5 MiB 块上限、分区版本取最大值、整批块共用微秒精度时间戳、meta 缺失报错，以及首个块写入错误后停止。

## 主要符号

- `MAX_COLUMN_SIZE: usize = 5 << 20`：单个 `stats_data` 块的 5 MiB 上限。它是本 crate 已接入上层 `handle.rs` 编码和 storage 分块路径的共享常量。
- `HistoryTimestamp = String`：持久化时间的文本类型，格式为 `YYYY-MM-DD HH:MM:SS.ffffff`。
- `HistoricalStatsMeta`：可克隆的历史 meta 值对象，包含 `table_id`、`version`、`modify_count`、`row_count` 和 `source`。当前 `pkg/statistics/handle/handle.rs` 直接重导出它。
- `Error(String)`：本地轻量错误容器，支持克隆和等值比较，便于 trait 边界传递以及单元测试精确断言。
- `HistoricalTable`：快照载体，`version` 是整表版本，`partition_versions` 是分区版本集，`encoded` 是待分块的编码数据。本文件不直接读取 `encoded`，它由 `StatsSnapshot::blocks` 的实现解释。
- `HistoryStore: Send + Sync`：持久化端口。`historical_enabled`、`stats_meta`、`replace_meta_history`、`insert_history_block` 分别对应开关、当前 meta、历史 meta 和历史块存储。
- `StatsSnapshot: Send + Sync`：快照端口。`dump_stats`、`table_initialized` 和 `blocks` 依次提供快照、缓存状态和分块。
- `StatsHistory`：持有 `Arc<dyn HistoryStore>` 和 `Arc<dyn StatsSnapshot>` 的门面。`new` 注入两个端口；方法 `record_historical_stats_to_storage`、`record_historical_stats_meta` 和 `check_historical_stats_enable` 分别对应 Go `statsHistoryImpl` 的三个操作。
- 同名自由函数 `record_historical_stats_meta`：验证 ID/版本，读取 `(modify_count, count)` 后替换历史 meta。
- 同名自由函数 `record_historical_stats_to_storage`：选择版本、分块、生成时间戳，然后顺序写入所有块。
- `current_history_timestamp` 和 `civil_date_from_days`：私有 UTC 时间格式化链。后者把 Unix epoch 起算的天数转换为格里高利年、月、日。本文件没有条件编译项。

## 执行流程

1. 单表/分区落盘从 `StatsHistory::record_historical_stats_to_storage(database, table_id, is_partition)` 开始。它先调用 `StatsSnapshot::dump_stats`；如果返回 `None`，把“没有可记录统计”视为成功空结果并返回版本 `0`。
2. 有快照时，门面调用自由函数 `record_historical_stats_to_storage`。函数在 `partition_versions` 非空时取其最大值，否则使用 `HistoricalTable::version`。
3. 调用 `StatsSnapshot::blocks(table, MAX_COLUMN_SIZE)` 一次性获得分块，随后仅调用一次 `current_history_timestamp`，因此同批块共用时间戳。
4. 按 `enumerate()` 的零基序号逐块调用 `HistoryStore::insert_history_block`。全部成功后返回选定版本；任一写入失败立即返回该错误。
5. 批量 meta 记录从 `StatsHistory::record_historical_stats_meta(version, source, enforce, table_ids)` 开始。版本为 `0` 时直接返回；目标列表始终排除 ID `0`，且在 `enforce == false` 时只保留 `table_initialized` 为真的表。
6. 目标列表建立后查询 `HistoryStore::historical_enabled`。关闭或查询报错时均不执行 meta 写入；开启时逐表调用自由函数 `record_historical_stats_meta`。
7. 单表 meta 函数拒绝零 ID/零版本，再以 `(table_id, version)` 查询当前 meta。有值时把 `modify_count`、`count`、版本和来源交给 `replace_meta_history`；无值时返回明确错误。门面的批量循环故意丢弃单表结果，使某表失败不中断后续表。

## 数据与状态

`StatsHistory` 本身只保存两个 `Arc` 端口，不缓存版本、表数据或交易状态。真实可变状态属于 `HistoryStore`/`StatsSnapshot` 的实现，因而两个 trait 都要求 `Send + Sync`。`HistoricalTable` 是一次调用的值对象，完成分块后不在门面内留存。

版本有两层含义：普通表使用 `HistoricalTable::version`；分区快照使用 `partition_versions` 的最大值代表本批最新统计。空快照的哨兵版本是 `0`，而 meta 路径把 `0` 视为无效版本，这保证空落盘不会产生 meta 记录。

每块的持久化键组件由 `physical_id`、零基 `sequence`、所选 `version` 和共享 `timestamp` 组成。时间戳基于 `SystemTime::now()` 的 UTC Unix 时间，微秒以下精度被舍去；系统时间早于 epoch 时 `duration_since` 的错误被 `unwrap_or_default` 归为 epoch。

## 依赖与调用关系

- crate 内部：`lib.rs` 公开并重导出本模块，且仅在 `cfg(test)` 下编入 `history_stats_aster_unit_test.rs`。
- 下游端口：`StatsHistory::record_historical_stats_to_storage` 调用 `StatsSnapshot::dump_stats`，再由自由函数调用 `StatsSnapshot::blocks` 和 `HistoryStore::insert_history_block`。`StatsHistory::record_historical_stats_meta` 调用 `table_initialized`、`historical_enabled`、`stats_meta` 与 `replace_meta_history`。`current_history_timestamp` 调用 `civil_date_from_days`。
- RustCodeGraph 对应边：图查询确认自由落盘函数调用 `blocks`、`insert_history_block` 和 `current_history_timestamp`，并确认 `current_history_timestamp -> civil_date_from_days`；元数据测试函数是 `record_historical_stats_meta` 的直接调用者。由于门面方法与自由函数同名，图的广域查询会混入同名边，本文档只采用了经文件限定和源码流程复核的边。
- 已接线上层：`pkg/statistics/handle/handle.rs` 使用 `HistoricalStatsMeta` 和 `MAX_COLUMN_SIZE`。该 crate 中的运行时 `Handle` 有自己的历史编码/存储路径；`pkg/domain/domain.rs` 的 `DomainStatsContext::record_historical_stats_to_storage` 以此路径向 `mysql.stats_history` 写块。`pkg/session/runtime/statistics.rs` 在 ANALYZE 完成且历史开关启用后调用该 domain 入口。这些生产路径并不直接构造本文件的 `StatsHistory`。
- 对照接口：`pkg/statistics/handle/types/interfaces.rs::StatsHistory` 保留了 Go 风格的应用级签名（含 `TableInfo` 和 `physical_id`），但本文件的具体 `StatsHistory` 没有实现该 trait：它的快照端口只接收 `database/table_id/is_partition`。扩展时不应假定两者已自动接线。

## 错误处理与边界

- `dump_stats` 或 `blocks` 返回错误时通过 `?` 原样传播，且分块前不会产生存储写入。`history_stats_aster_unit_test.rs::record_storage_propagates_snapshot_errors_without_writes` 覆盖了这两个边界。
- `dump_stats == None` 不是错误，落盘入口返回 `Ok(0)` 且不写块；测试 `record_storage_returns_zero_when_snapshot_is_absent` 固定了该契约。
- 块写入没有本文件级的交易或回滚：第一个 `insert_history_block` 错误会立即停止，但先前已成功的块保留。`record_storage_stops_at_first_insert_error` 明确验证了该部分写入语义。如需原子性，必须由 `HistoryStore` 的实现或上层交易边界提供，不能仅修改循环的返回值。
- 单表 meta 入口对零 ID/零版本返回 `tableID ..., version ... are invalid`，对查不到当前 meta 返回 `no historical meta stats can be recorded`。`record_meta_rejects_zero_and_missing_current_meta` 覆盖了这些错误。
- 批量 meta 入口是“尽力而为”的无返回值 API：`historical_enabled` 查询错误被 `unwrap_or(false)` 当作关闭，每个单表 meta 错误被 `let _ = ...` 忽略。这保证后续表继续，但当前也没有日志或错误聚合；这是可观测性边界，不应被误认为每表都已成功。
- `current_history_timestamp` 对 epoch 之前的系统时间降级为 `1970-01-01 00:00:00.000000`，不会 panic。当前单元测试只校验格式和同批一致性，未直接覆盖闰年、跨日或 epoch 前分支。

## 并发与资源生命周期

`Arc<dyn HistoryStore>` 和 `Arc<dyn StatsSnapshot>` 使门面可以共享后端所有权；trait 的 `Send + Sync` 约束要求后端自行保证并发安全。本文件不创建线程、任务、通道、锁或连接池，也没有 `Drop` 清理逻辑。同一个 `StatsHistory` 的并发调用是否隔离，取决于具体 store/snapshot 实现的内部锁和交易策略。

单次落盘的临时资源包括 `HistoricalTable`、全部 `Vec<Vec<u8>>` 分块和时间戳字符串。因为 `blocks` 一次性返回全部块，峰值内存由后端的编码和分块实现决定；本文件没有流式背压。块按序号串行插入，这保持顺序和首错即停语义，但写入延迟随块数线性增长。

该生命周期与 `pkg/domain/historical_stats.rs::HistoricalStatsWorker` 不同：后者管理有界 `sync_channel` 和 worker 投递，而本文件只实现收到一次调用后的同步持久化逻辑，不管理队列。

## 与 Go 版本的对应关系

- Rust `StatsHistory` 对应 Go `statsHistoryImpl`，三个方法对应 `RecordHistoricalStatsToStorage`、`RecordHistoricalStatsMeta` 和 `CheckHistoricalStatsEnable`。Rust 通过两个 trait 注入能力；Go 持有 `types.StatsHandle`，并通过 session pool 与 `CallWithSCtx` 打开事务/session 边界。
- Go 在分区和非分区分支分别调用 `TableStatsToJSON` 与 `DumpStatsToJSON`；Rust 把该差异收入 `StatsSnapshot::dump_stats(database, table_id, is_partition)` 实现。Go 用 `storage.JSONTableToBlocks`，Rust 通过 `StatsSnapshot::blocks`。
- 两者在无快照时都返回版本 `0`，在有分区时都取分区版本最大值，并都用 `5 << 20` 作为分块上限。
- Go 用 `time.Now().Format("2006-01-02 15:04:05.999999")`；Rust 用 `SystemTime` 和公历换算构造相同长度和微秒字段的 UTC 文本。这里存在一项语义差异：Go `time.Now()` 的格式化使用该 `Time` 的 location（通常是本地时区），Rust 实现明确是 UTC。现有测试只保证文本形状、微秒精度和同批共享，不保证时区一致。
- Go 的 meta 函数使用 `SELECT ... FOR UPDATE` 后 `REPLACE ... NOW(6)`，具体锁和事务由 session context 提供。Rust 只表达 `stats_meta` 后 `replace_meta_history` 的顺序，必须由 `HistoryStore` 实现保持两步的事务/锁语义。
- Go 批量 meta 路径在开关查询或单表写入失败时记录日志；Rust 当前把开关错误当作 `false`，并静默丢弃单表错误。Go 在 `enforce == true` 时保留输入列表（包括理论上的零 ID，然后由单表函数报错）；Rust 无论 `enforce` 值都预先排除零 ID。这两点是当前迁移差异。
- `pkg/statistics/handle/handletest/handle_test.go::TestRecordHistoricalStatsToStorage` 验证 Go 的整合行为；Rust 对应的 `go_test_record_historical_stats_to_storage` 验证当前 domain/handle 生产接线。本文件自身的行为由同目录 `history_stats_aster_unit_test.rs` 的六个独立测试覆盖。

## 扩展指南

- 新增存储实现时，优先实现 `HistoryStore`。要明确 `stats_meta + replace_meta_history` 是否需要同一事务及行锁，并确保 `insert_history_block` 对 `(table_id, sequence, version)` 的重试/重复键语义与 Go `ON DUPLICATE KEY UPDATE` 一致。相关测试应放在独立的 `history_stats_aster_unit_test.rs` 或实现所属 crate 的独立测试文件，不应内嵌到生产源文件。
- 新增快照格式或流式分块时，接入 `StatsSnapshot::dump_stats`/`blocks`，并同步检查 `pkg/statistics/handle/handle.rs` 对 `MAX_COLUMN_SIZE` 的两处使用。修改版本选择必须增加普通表、空分区、多分区和分区版本无序的测试。
- 若要把本文件的 `StatsHistory` 门面接入完整应用，需先解决它与 `pkg/statistics/handle/types/interfaces.rs::StatsHistory` 的签名差异，并与现有 `Handle`/`DomainStatsContext` 历史路径选定单一权威实现。不应在不处理交易、日志、`TableInfo` 和分区 `physical_id` 的情况下直接替换生产路径。
- 增加并行写块前，需保留序号稳定、同批时间戳一致和失败后可预测的部分写入/回滚契约，同时补充并发、重试和大快照内存占用测试。
- 修改时间戳逻辑时，应在独立测试中覆盖 epoch、闰年、跨日和时区预期，并核对 `mysql.stats_history.create_time` 的 DATETIME(6) 语义。当前 `SystemTime::now()` 不可注入，若需稳定的日期测试，应先抽出时钟或纯格式化函数。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，目标目录内 `history_stats.rs` 识别出 22 个符号；通过 `node --file` 读取了目标全文，并对 `record_historical_stats_to_storage`、`record_historical_stats_meta`、`current_history_timestamp` 和 `civil_date_from_days` 执行了 `query/callers/callees`。
- 源码与装配：`pkg/statistics/handle/history/history_stats.rs`、`pkg/statistics/handle/history/lib.rs`、`pkg/statistics/handle/history/Cargo.toml`、`pkg/statistics/handle/Cargo.toml`。
- 直接 Rust 调用/接线证据：`pkg/statistics/handle/handle.rs`（`HistoricalStatsMeta` 重导出和 `MAX_COLUMN_SIZE` 用法）、`pkg/domain/domain.rs::DomainStatsContext::record_historical_stats_to_storage`、`pkg/domain/domain.rs::analyze_stats_table`、`pkg/session/runtime/statistics.rs`的 ANALYZE 完成路径、`pkg/domain/historical_stats.rs::HistoricalStatsWorker`、`pkg/statistics/handle/types/interfaces.rs::StatsHistory`。
- Rust 测试：`pkg/statistics/handle/history/history_stats_aster_unit_test.rs` 覆盖分区最大版本与共享时间戳、空快照、meta 过滤与继续处理、无效/缺失 meta、零版本/关闭开关、快照/分块错误和首个插入错误停止；`pkg/statistics/handle/handletest/handle_test.rs::go_test_record_historical_stats_to_storage` 覆盖当前 Rust 生产接线的整合落盘。
- Go 对照：`pkg/statistics/handle/history/history_stats.go`、`pkg/statistics/handle/types/interfaces.go`、`pkg/statistics/handle/handle.go`、`pkg/domain/historical_stats.go`、`pkg/statistics/handle/handletest/handle_test.go::TestRecordHistoricalStatsToStorage`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文件存在且上述固定二级标题恰好 11 个；人工复核重点是不将本地门面误写成已直接接入的唯一生产路径。
