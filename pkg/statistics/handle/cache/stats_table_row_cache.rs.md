# `pkg/statistics/handle/cache/stats_table_row_cache.rs`

## 文件定位

本文件定义了一个“查询所需的表行数与列长度快照”及其表/索引长度估算算法。它属于 Cargo crate `astersql-statistics-handle-cache`（`pkg/statistics/handle/cache/Cargo.toml`），由 `pkg/statistics/handle/cache/lib.rs` 以 `mod stats_table_row_cache` 纳入，并通过 `pub use stats_table_row_cache::*` 对 crate 外再导出公开类型和函数。同一 `lib.rs` 用 `#[path = "stats_table_row_cache_test.rs"]` 挂载独立 Rust 测试，测试逻辑没有内嵌在生产文件中。

它与 Go 的直接行为对照不在 Cargo 元数据声明的 `pkg/statistics/handle/cache` Go package，而是 `pkg/statistics/handle/storage/table_size_stats.go` 中的 `TableSizeStats`、`GetTableSizeStats`、`EstimateDataLength`和 `GetDataAndIndexLength`。Go 生产链路由 `pkg/executor/infoschema_reader.go` 的 `buildTableSizeStats` 驱动，用于填充 `information_schema.tables` 和 `information_schema.partitions` 的 `TABLE_ROWS`、`AVG_ROW_LENGTH`、`DATA_LENGTH`、`INDEX_LENGTH`。全仓 Rust 搜索只找到本文件内部调用与 `stats_table_row_cache_test.rs` / `statscache_test.rs` 测试调用；因此当前可证实的状态是“算法和测试已移植，尚未发现 Rust 生产 SQL/provider 接线”。

## 核心职责

1. 用 `StatsTableRowCacheState` 保存“物理表 ID → 行数”和“`(tableID, histID)` → 变长列总字节数”两张映射。
2. 通过 `RowStatsProvider` 隔离数据获取与估算逻辑；`UpdateByID` 可在只需 `TABLE_ROWS` 时跳过较贵的列长度读取。
3. 依据 `TableMeta` 中的公开列、公开索引、分区、全局索引和 sequence 标记，估算行数、平均行长、数据长度和索引长度。
4. 通过 `buildInTableIDsString` 保留 Go 版的 `table_id in (...)` 谓词格式，但本文件自身不执行 SQL。

## 主要符号

- `tableHistID { tableID, histID }`：列长度映射的可哈希复合键。`tableID` 可以是逻辑表 ID 或分区物理 ID，`histID` 在当前估算路径中是列 ID。
- `ColumnMeta { ID, FixedLength, Public }`：估算所需的最小列摘要。`FixedLength = Some(n)` 表示每行固定 `n` 字节；`None` 表示需查 `colLength`；`Public` 对应 Go `model.StatePublic`。
- `IndexColumnMeta { Offset, Length }`：`Offset` 指向 `TableMeta.Columns`；`Length = Some(n)` 表示前缀长度，`None` 对应 Go `types.UnspecifiedLength`，此时复用该列已计算的数据长度。
- `IndexMeta { ID, Public, Global, Columns }`：索引摘要。`ID` 当前不参与计算；`Public` 控制可见性；`Global` 决定分区表中应在逻辑表层还是分区层计算。
- `TableMeta { ID, Columns, Indices, Partitions, IsSequence }`：表结构的最小输入模型。`Partitions` 保存分区物理 ID；非空时启用分区计算分支。
- `RowStatsProvider`：同步数据源 trait。`RowCounts(ids)` 返回行数映射，`ColumnLengths(ids)` 返回原始 tuple 键的列长度映射，两者均可返回 crate 根定义的 `CacheError`。
- `StatsTableRowCacheState`：非公开快照，同时包含 `tableRows` 与 `colLength`。
- `StatsTableRowCache`：公开容器，用单个 `RwLock<StatsTableRowCacheState>` 保护整个状态。`Default` 产生两张空表。
- `GetTableRows` / `GetColLength`：单键查询；缺失键返回 `0`。
- `UpdateByID`：批量拉取并替换整个快照，而非增量合并。
- `EstimateDataLength`：对普通表、分区表或 sequence 返回 `(row_count, avg_row_length, data_length, index_length)`。
- `GetDataAndIndexLength`：针对单个逻辑/物理 ID 累加数据与索引长度。
- `buildInTableIDsString`：把 ID 数组格式化为完整谓词，例如 `[7, -2, 9]` 变为 `table_id in (7,-2,9)`。

## 执行流程

### 更新快照

1. 调用者把 ID 切片与 `RowStatsProvider` 传给 `UpdateByID`。
2. `UpdateByID` 先调用 `RowCounts(ids)`。若失败，立即用 `?` 返回，旧快照不变。
3. 只有 `need_column_lengths` 为 `true` 时才调用 `ColumnLengths(ids)`；否则使用空映射。这是对 Go 路径中“仅查 `TABLE_ROWS` 不读 `mysql.stats_histograms`”优化的抽象。
4. 两次 provider 读取都成功后才取写锁，替换 `tableRows`，并把 `(i64, i64)` tuple 键转换为 `tableHistID`后替换 `colLength`。
5. 因为是整体替换，本次未返回的 ID 不会保留；当 `need_column_lengths = false` 时，之前的列长度也会被清空。

### 估算表大小

1. `EstimateDataLength` 先从逻辑表 `t.ID` 取行数，并调用 `GetDataAndIndexLength(t, t.ID, row_count)`。普通表直接使用这组结果。
2. 若 `Partitions` 非空，逻辑表层数据长度不作为最终数据长度：函数将 `row_count` 和 `data_length` 清零，再遍历每个分区 ID，累加分区行数、数据长度和本地索引长度。逻辑表层预先计算的 `index_length` 被保留，用来容纳全局索引。
3. `GetDataAndIndexLength` 先建立与 `Columns` 等长的 `column_length`。它跳过非公开列；固定长度列用 `FixedLength * rows`，变长列以 `(pid, column.ID)` 查快照。结果同时加入 `data` 并写到对应 offset。
4. 再遍历公开索引。对分区表，全局索引只在 `pid == t.ID` 时计算，非全局索引只在 `pid != t.ID` 的分区层计算。索引列指定前缀长度时用 `rows * Length`，否则复用 `column_length[Offset]`。
5. 最终以 `data_length / row_count` 计算平均行长；行数为零时平均值为零。sequence 只在最后把返回的 `row_count` 强制为 `1`，长度与平均值仍基于原始统计行数计算，与 Go 顺序一致。

## 数据与状态

- `tableRows: HashMap<i64, u64>` 是完整快照，值语义是某逻辑表或分区的近似行数。
- `colLength: HashMap<tableHistID, u64>` 的值是该列跨所有行的总字节数，对应 Go 从 `mysql.stats_histograms.tot_col_size` 读取的数据，不是单行平均长度。
- 缺失行数或列长度与真实值为零在 API 上都表现为 `0`，无法通过 getter 区分。
- `UpdateByID` 的 `ids` 是 provider 查询范围，但本类不检查 provider 返回的 key 是否恰好属于该范围；返回映射被原样接受。
- 所有长度与行数累加、乘法都显式使用 `wrapping_add` / `wrapping_mul`，以匹配 Go `uint64` 溢出回绕，而不是 panic、饱和或返错。
- `IndexMeta.ID` 目前仅是元数据的一部分，本文件没有以它查询 `colLength`；Go 也是逐个索引列估算索引长度。

## 依赖与调用关系

- 直接 Rust 依赖只有标准库 `HashMap`、`RwLock` 以及 crate 根的 `CacheError`。目标文件没有直接使用 `Cargo.toml` 中的其他内部 crate 或外部 crate，也没有 feature/`cfg` 分支。
- 内部调用边为：`EstimateDataLength → GetTableRows`、`EstimateDataLength → GetDataAndIndexLength`、`GetDataAndIndexLength → GetColLength`；`UpdateByID → RowStatsProvider::RowCounts` 并在需要时调用 `RowStatsProvider::ColumnLengths`。
- `pkg/statistics/handle/cache/lib.rs` 是 Rust 模块入口和公开 API 出口。`stats_table_row_cache_test.rs` 直接验证更新和溢出语义；`statscache_test.rs` 验证格式化、普通表、分区表、全局/本地索引、非公开元数据与 sequence。
- RustCodeGraph `query` 定位到本文件的唯一 Rust `UpdateByID`，并同时定位到 Rust/Go 两个 `EstimateDataLength`、`GetDataAndIndexLength`和 `buildInTableIDsString`。图的 `explore` 返回目标文件和两个 Rust 测试上下文。精确 `callers`/`callees` 查询在当前共享索引负载下超时，因此又用全仓 `rg` 补齐了未返回的调用点。
- Go 生产上游边为 `pkg/executor/infoschema_reader.go::buildTableSizeStats → pkg/statistics/handle/storage/table_size_stats.go::GetTableSizeStats`，随后信息模式行构建使用 `TableSizeStats::EstimateDataLength`。这条 Go 边是理解 Rust 算法应用位置的直接证据，不等于 Rust 已有同等生产接线。

## 错误处理与边界

- `UpdateByID` 是唯一返回 `Result` 的方法。`RowCounts` 失败，或行数成功但 `ColumnLengths` 失败时，都会在获取写锁前返回 `CacheError`，因而保留整个旧快照。
- `std::sync::RwLock` 的 `read()` / `write()` 结果被直接 `unwrap()`；若其他线程持有写锁时 panic 导致锁中毒，后续 getter 或更新会 panic，不会转换为 `CacheError`。
- `GetDataAndIndexLength` 假定每个 `IndexColumnMeta.Offset < TableMeta.Columns.len()`。输入不满足该不变式时，`column_length[column.Offset]` 会越界 panic；函数内没有元数据校验。
- 缺失统计默认为零，算法不返“统计不完整”错误。非公开列/索引被安静忽略。
- 空 ID 切片如何解读由 provider 决定。`buildInTableIDsString(&[])` 返回 `table_id in ()`，这是有测试的 Go 格式对齐；调用者不应不加判断地假设该字符串在所有 SQL 方言中都可单独执行。
- 长度溢出是故意的 Go 兼容语义，不是可报告错误；修改为 checked/saturating 运算会改变兼容行为。

## 并发与资源生命周期

- 单个 `RwLock` 使 `tableRows` 和 `colLength` 的“替换动作”是同一个临界区；读者不会看到一张表已替换而另一张表尚未替换的中间状态。
- provider 读取和 tuple-key 转换在写锁外完成，因此慢数据源不会长时阻塞 getter。代价是并发调用两次 `UpdateByID` 时，最终快照由“最后获得写锁的调用”决定，不保证按调用开始顺序。
- `GetTableRows` 和 `GetColLength` 每次单独获取读锁。`EstimateDataLength` 会多次调用它们，不在一个持续读锁内完成；若估算期间发生并发更新，一次估算可能混合两个时刻的值。所以源码注释中的“同一快照”只能保证单次替换/单次 getter，不保证整个复合估算的一致读。
- 该类不持有数据库连接、任务、通道或事务；provider 仅以借用的 trait object 在 `UpdateByID` 同步调用期间存活。`HashMap` 和锁由 Rust RAII 在 cache 析构时自动释放。
- Go `TableSizeStats` 是每查询新建、不共享，因而不需要锁；Rust 引入 `RwLock` 让对象可被共享更新，这是结构差异，不能据此推断它具有 Go “每查询独立快照”的全部生命周期保证。

## 与 Go 版本的对应关系

| Rust | Go | 对应与差异 |
| --- | --- | --- |
| `tableHistID` | `storage.tableHistID` | 字段和键语义一致。 |
| `StatsTableRowCacheState` + `StatsTableRowCache` | `TableSizeStats` | 映射语义一致；Rust 拆成锁容器和内部状态，Go 对象按查询创建且无锁。 |
| `RowStatsProvider` + `UpdateByID` | `GetTableSizeStats` + `getRowCountTables` + `getColLengthTables` | Rust 把 SQL 读取抽象为 provider；目标文件没有 Go 中的 `sessionctx.Context`、`util.ExecWithOpts`、负 `tot_col_size` 截零或 failpoint。 |
| `ColumnMeta` / `IndexMeta` / `TableMeta` | `model.ColumnInfo` / `model.IndexInfo` / `model.TableInfo` | Rust 只保留估算所需字段，由布尔值表示 public/sequence，用 `Option<u64>` 表示固定长度或前缀长度。 |
| `GetTableRows` / `GetColLength` | 同名 Go 方法 | 缺失键均返零。Go 还允许 nil receiver 并返零；Rust `&self` 不存在 nil receiver 对应语义。 |
| `EstimateDataLength` | 同名 Go 方法 | 普通/分区/sequence 顺序与全局索引保留规则一致。 |
| `GetDataAndIndexLength` | 同名 Go 方法 | public 过滤、定长/变长列、索引前缀、分区全局/本地索引规则一致；Rust 显式 wrapping 以保证 Go `uint64` 溢出行为。 |
| `buildInTableIDsString` | 同名 Go 函数 | 产生相同的完整 `table_id in (...)` 谓词，包括空列表的 `table_id in ()`。 |

Go 回归证据主要在 `pkg/executor/test/infoschema/infoschema_test.go`：`TestTableRowsOnlySkipsColumnLengthRead` 用 failpoint 确认只请求 `TABLE_ROWS` 时不读列长度，并验证普通表、分区表及全局索引的结果。Rust `stats_table_row_cache_test.rs::table_rows_only_update_skips_column_length_provider` 验证抽象后的跳过行为和旧列长度清空；`statscache_test.rs::estimate_data_length_matches_go_public_partition_and_sequence_rules` 验证估算规则。

## 扩展指南

- 接入真实 Rust 存储读取时，应在本文件外实现 `RowStatsProvider`，并保留 `need_column_lengths = false` 不触发列长度查询的性能契约。同时要明确移植 Go `getColLengthTables` 的负值截零、错误传播及空 ID 列表语义，不要把这些职责隐式塞入估算函数。
- 增加表/列/索引元数据语义时，优先修改 `ColumnMeta`、`IndexColumnMeta`、`IndexMeta` 或 `TableMeta`，再同步 `GetDataAndIndexLength`。必须与 `model.TableInfo` 当前 Go 判定顺序比对，特别是 non-public 状态、表达式/隐藏列、索引前缀与全局索引。
- 要求一次复合估算读取单一一致快照时，修改点应是 `EstimateDataLength` / `GetDataAndIndexLength` 的读取方式，例如在一次读锁下完成纯计算或克隆状态后计算；需同时评估锁持有时间、克隆成本和并发更新语义。
- 改变更新为增量合并时，要特别保护“只读行数后不能继续暴露上一轮列长度”的现有行为，否则会把不同查询范围/时刻的值混在一起。
- 更改数学运算必须保留 Go `uint64` wrapping 语义，除非上层兼容契约明确变更。不要用编译模式下可能 panic 的默认 `+` / `*` 替代现有 wrapping 调用。
- 新增行为应放在独立测试文件 `pkg/statistics/handle/cache/stats_table_row_cache_test.rs`；如果是 crate 对外组合行为，同步扩展 `pkg/statistics/handle/cache/statscache_test.rs`。用户可见的信息模式语义还应对照 Go `pkg/executor/test/infoschema/infoschema_test.go`，直到 Rust 有对应集成测试面。
- 性能风险集中在列长度数据源、大快照 `HashMap` 替换和锁竞争；正确性风险集中在分区全局/本地索引双计或漏计、offset 越界、缺失值被视为零及并发混合快照。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `11467` 个文件，其中 Rust `7032` 个；`files --filter pkg/statistics/handle/cache` 列出目标生产文件、`stats_table_row_cache_test.rs`、`statscache_test.rs` 及模块入口。
- RustCodeGraph 符号证据：`query StatsTableRowCache`、`query UpdateByID`、`query EstimateDataLength`、`query GetDataAndIndexLength`、`query buildInTableIDsString`、`query RowStatsProvider`；`explore "pkg/statistics/handle/cache/stats_table_row_cache.rs StatsTableRowCache StatsTableRow"` 返回目标源码与相关 Rust 测试上下文。
- 图查询限制：精确 `callers`/`callees` 命令在共享 RustCodeGraph 负载下多次 30 秒无输出，已中止；本文档的调用边另由目标文件的已索引源码上下文和全仓精确符号搜索交叉核对。
- Rust 源码/装配证据：`pkg/statistics/handle/cache/stats_table_row_cache.rs`、`pkg/statistics/handle/cache/lib.rs`、`pkg/statistics/handle/cache/Cargo.toml`、根 `Cargo.toml` workspace 成员记录。
- Rust 测试证据：`pkg/statistics/handle/cache/stats_table_row_cache_test.rs` 的 `table_rows_only_update_skips_column_length_provider`、`data_and_index_lengths_match_go_uint64_overflow`、`accumulated_lengths_match_go_uint64_overflow`；`pkg/statistics/handle/cache/statscache_test.rs` 的 `build_in_table_ids_string_matches_go_predicate` 和 `estimate_data_length_matches_go_public_partition_and_sequence_rules`。
- Go 对照/主链证据：`pkg/statistics/handle/storage/table_size_stats.go`、`pkg/executor/infoschema_reader.go::buildTableSizeStats`、`pkg/executor/test/infoschema/infoschema_test.go::TestTableRowsOnlySkipsColumnLengthRead`，以及同文件的普通表/分区表长度断言。
- 人工复核结论：文档已分开 Rust 已有算法、Rust 测试接线和 Go 生产接线；未把 Go SQL 实现、nil receiver 或每查询无锁生命周期误写成 Rust 现状，也未把测试覆盖解释为生产可达性。
