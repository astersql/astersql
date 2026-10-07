# `pkg/executor/show_stats.rs` 逻辑说明

## 文件定位

`pkg/executor/show_stats.rs` 属于 `astersql-executor` crate；crate 由 `pkg/executor/Cargo.toml` 定义，`pkg/executor/lib.rs:224` 以公开模块 `show_stats` 暴露本文件。它不是 SQL 解析器或统计信息存储实现，而是 `SHOW STATS_*`、`SHOW ANALYZE STATUS`、`SHOW COLUMN_STATS_USAGE` 的领域执行核心：输入一组与具体会话解耦的统计快照，输出 `Vec<Row>`。

完整应用中的上游入口是 `ConcreteSession::execute_show_stats`（`pkg/session/runtime/statistics.rs:2215` 附近）。该入口按 `ast::ShowStmtType` 选择本文件的 `fetchShowStatsMeta`、`fetchShowStatsHistogram`、`fetchShowStatsTopN`、`fetchShowStatsBuckets`、`fetchShowStatsHealthy`、`fetchShowStatsLocked`、`fetchShowHistogramsInFlight`、`fetchShowAnalyzeStatus` 或 `fetchShowColumnStatsUsage`，再把 `Cell` 转成协议层字符串并应用 SQL `LIKE`/`WHERE` 条件。

生产数据桥接由 `SessionShowStatsRuntime`（`pkg/session/runtime/statistics.rs:1178`）实现 `ShowStatsRuntime`。因此本文件只规定遍历、排序、分区展开、行结构和错误传播；目录、统计缓存、锁表集合、编码值解码等由会话/Domain 层提供。

## 核心职责

- 用 `ShowStatsRuntime` 把外部统计服务收敛为可测试的同步接口（`show_stats.rs:212-240`）。
- 把逻辑表按裁剪模式展开为普通表、分区表全局统计（`global`）和各物理分区（`visible_physical_tables`，`show_stats.rs:311`）。
- 为元信息、锁、直方图、桶、TopN、健康度、加载中直方图、分析任务和列使用记录生成固定列序的 `Row`。
- 对 schema、表、列和索引按名称或 ID 做确定性过滤/排序，令输出稳定；其中生产会话当前使用默认 `ShowFilters`，最终 SQL 谓词主要在 `execute_show_stats` 对物化后的行执行。
- 跳过伪统计，按统计类型决定空值、时间戳、索引标记和编码边界的展示方式。

本文件不更新统计、不触发 analyze、不管理事务，也不直接访问存储；这些均在 `ShowStatsRuntime` 的实现侧完成。

## 主要符号

- `Cell`、`Row`、`Timestamp`（`show_stats.rs:28-43`）：执行层中立结果模型。`Cell` 可表达 NULL、文本、整数、浮点和毫秒 Unix 时间；`Row` 是按 SHOW 结果 schema 排列的单元格数组。
- `ShowStatsError`（`show_stats.rs:48-69`）：保存静态操作名和消息，并实现 `Display`/`Error`。它是 runtime 边界及 fetch 方法的统一错误类型。
- `ShowFilters`（`show_stats.rs:73-78`）：支持 schema 精确值、schema LIKE、数据库集合和表集合。`schema_allowed`、`table_allowed` 消费这些条件。
- `SimpleTableInfo`、`PartitionDefinition`、`ColumnInfo`、`TableInfo`（`show_stats.rs:82-109`）：将完整元数据裁剪为 SHOW 所需的表 ID、名称、分区和可见列。
- `CacheItemMemoryUsage`、`Bucket`、`Histogram`、`TopNItem`、`TopN`、`ColumnStats`、`IndexStats`、`TableStats`（`show_stats.rs:113-192`）：与 Go `statistics.Table` 展示面对应的只读快照。`TableStats` 同时承载伪统计、分析版本、行数、健康度、列和索引。
- `TableItemId`、`ColumnStatsUsage`（`show_stats.rs:196-207`）：以物理表 ID、列 ID、是否索引为键关联列统计使用时间。
- `ShowStatsRuntime`（`show_stats.rs:212`）：唯一外部依赖边界；所有方法都是必需项，避免生产实现用默认成功值静默掩盖缺失能力。
- `ShowExec<R>`（`show_stats.rs:243-247`）：拥有 runtime、过滤器和累积结果 `rows`。`new` 初始化空结果，`appendRow` 追加结果，`rows_page` 对已物化数据安全分页。
- `fetchShowStats*` 系列（`show_stats.rs:334-919`）：公开的语句级入口；`appendTableFor*`、`histogramToRow`、`topNToRows`、`bucketsToRows` 负责表/对象级展开。
- 本文件没有模块级常量、条件编译项或异步函数。

## 执行流程

1. `ConcreteSession::execute_show_stats` 从会话取得 `DomainStatsContext` 与动态分区裁剪开关，构造 `SessionShowStatsRuntime`、默认 `ShowFilters` 和 `ShowExec`（`pkg/session/runtime/statistics.rs:2219-2227`）。
2. 语句分派到相应 fetch 方法。多数统计明细入口先经 `sorted_schema_names`，跳过 `is_mem_or_sys_schema` 判定的 `mysql`、`information_schema`、`performance_schema`、`metrics_schema`、`sys` 与 BR 临时 schema。
3. 元信息和健康度路径先建立分区表映射，再通过 `schema_simple_table_infos` 遍历表；动态裁剪模式输出逻辑表 ID 对应的 `global` 行，静态模式不输出 global，二者都遍历物理分区。仅 `non_pseudo_stats` 返回的真实统计可进入 append 阶段（`fetchShowStatsMeta`、`fetchShowStatsHealthy`）。
4. 锁、直方图、桶、TopN 路径使用 `schema_table_infos` 和 `visible_physical_tables`。普通表产生空分区名；动态分区表产生 global 加各分区；静态分区表只产生各分区。`fetchShowStatsLocked` 先一次性获取相关物理 ID 的锁集合，再按 ID 稳定输出。
5. 直方图路径仅输出 `initialized` 的列/索引，包含版本、NDV、NULL 数、平均大小、相关性、加载状态和内存拆分。`versionToTime` 通过 TiDB TSO 右移 18 位取得物理毫秒。
6. 桶与 TopN 路径先按列 ID 遍历并收集列类型，再按索引 ID 遍历；索引通过组成列名映射类型。`value_to_string` 解码列值或复合索引元组，随后按固定列序追加行。
7. 列使用路径先加载整个 `BTreeMap<TableItemId, ColumnStatsUsage>`，再为逻辑表 global 键及每个分区键匹配非隐藏列，缺失记录不输出。
8. 会话层将 `Cell` 格式化为字符串，应用 `ShowStmt.Pattern`/`Where`，并构造 `ConcreteRecordSet`（`pkg/session/runtime/statistics.rs:2330` 附近）。

## 数据与状态

`ShowExec` 的可变状态只有 `rows`，fetch 调用采用追加语义，不会自动清空；同一实例连续调用多个 fetch 会合并结果，调用方若需要独立结果应新建实例或自行清空。`rows_page` 返回借用切片，并用 `min` 与 `saturating_add` 防止越界及 `offset + limit` 溢出（`show_stats.rs:265-269`）。

表统计是值快照：本文件为稳定排序会克隆列、索引和分区列表，不修改 runtime 中的统计对象。`BTreeMap`/`BTreeSet` 同时用于确定性遍历、物理 ID 去重和列使用键查找。输出顺序一般是 schema 名、表 ID、分区 ID、列/索引 ID；TopN 项和桶保持其原始内部顺序。

分区名约定是兼容契约：普通表为 `""`，分区表逻辑统计为 `"global"`，物理分区为分区名。`is_index` 在输出中编码为 `0/1`。未分析的 meta 行将 `Last_analyze_time` 写为 `Cell::Null`；列使用的两个可选时间也分别映射为 NULL。

当前生产适配器 `show_table_stats`（`pkg/session/runtime/statistics.rs:1222`）把持久化统计转换为本文件模型：加载状态简化为 `allLoaded`/`allEvicted`，内存拆分当前填默认零值，健康度当前按 `modify_count == 0` 映射为 100，否则为 0。这些是现状，不是本文件自行计算出的完整 Go 统计语义。

## 依赖与调用关系

上游主链为：SQL `SHOW` AST → `ConcreteSession::execute_show_stats`（`pkg/session/runtime/statistics.rs`）→ `ShowExec::fetchShow*`（本文件）→ `rows` → 会话层 `ConcreteRecordSet`。`pkg/executor/lib.rs:224` 提供 crate 公开模块入口，`pkg/executor/lib.rs:327-328` 仅在测试配置中挂载独立测试文件 `show_stats_test.rs`。

下游边界为 `ShowStatsRuntime`：

- schema/表元数据：`all_schema_names`、`partitioned_table_infos`、`schema_*_table_infos`；
- 统计数据：`non_pseudo_stats`、`physical_stats`、`column_stats_usage`、`analyze_status_rows`；
- 会话语义：`dynamic_partition_prune_enabled`、`wildcard_match`；
- 存储/格式化：`locked_table_ids`、`value_to_string`、`histograms_in_flight`、`log_nonfatal`。

生产实现最终调用 `DomainStatsContext` 的 catalog、持久化物理统计、锁表集合、分析任务与列使用记录，以及 `astersql_statistics::DecodeRuntimeStatsValueWithTypes`（`pkg/session/runtime/statistics.rs:1340-1566`）。本文件源码本身只直接依赖 Rust 标准库的 `BTreeMap`、`BTreeSet` 和格式化 trait；统计、domain、session 等 crate 依赖位于适配器一侧。`pkg/executor/Cargo.toml` 没有为本模块设置专用 feature，唯一列出的 crate feature `nextgen` 与此文件无条件编译无关。

## 错误处理与边界

`schema_table_infos`、`physical_stats`、锁查询、值解码、分析状态和列使用错误均通过 `?` 立即返回 `ShowStatsError`。由于 `rows` 是边遍历边追加的，错误发生前已经写入的行不会回滚；调用方必须在错误时丢弃该执行器结果。生产会话用 `session_error("SHOW ...", error)` 添加语句上下文。

Meta 与 Healthy 的 `schema_simple_table_infos` 特意返回“数据 + 可选错误”；本文件调用 `log_nonfatal` 后继续处理已有表列表，对齐 Go `terror.Log(err)` 的非致命行为。当前 `SessionShowStatsRuntime::log_nonfatal` 是空实现，所以 Rust 生产路径不会真正记录该错误。

伪统计不会出现在 meta、histogram、bucket、TopN；健康度依赖 `non_pseudo_stats`，缺失健康度也不输出。TopN 为 `None`、空桶、未初始化 histogram、缺失列使用键均合法地产生零行。索引引用的列名未出现在列统计时，类型回退为 `0`（`appendTableForStatsBuckets`/`appendTableForStatsTopN`），随后解码是否成功由 runtime 决定。

`versionToTime` 将超出 `i64` 的物理毫秒饱和到 `i64::MAX`。schema 系统库识别使用 ASCII 小写；生产 runtime 的 `wildcard_match` 当前只完整支持 `%`、尾部 `%` 前缀和无通配符的忽略大小写比较，不等同于 Go `collate.WildcardPattern` 的全部 LIKE 语义。不过生产 `execute_show_stats` 当前以默认 `ShowFilters` 构造执行器并在行物化后另行处理 SQL pattern，因此这一简化主要影响直接使用 `ShowFilters.field_pattern_like` 的调用者。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、事务、文件句柄或显式锁。`ShowExec` 独占 runtime 和结果向量，所有 fetch 方法需要 `&mut self`，因此同一实例不能被安全地并发写入；是否可跨线程取决于泛型 `R`，本文件没有施加 `Send`/`Sync` 约束。

每次 fetch 都同步获取 runtime 提供的快照并在方法返回前完成行物化，不持有 catalog 或统计缓存的借用。生产 `SessionShowStatsRuntime` 持有可克隆的 `DomainStatsContext`；内部锁及快照一致性由 domain/statistics 层负责。较大 schema、桶或 TopN 会线性增长 `rows` 内存；`rows_page` 只在全部物化之后切片，不是流式限流机制。

`fetchShowHistogramsInFlight` 的 Go 实现会调用 `CleanFakeItemsForShowHistInFlights`，可能顺便清理假加载项；当前 Rust runtime 只返回固定 `Cell::Signed(0)`（`pkg/session/runtime/statistics.rs:1486-1488`），因此没有对应清理副作用。这是迁移差异，扩展时不能假设本文件已有完整资源生命周期语义。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/executor/show_stats.go`。Rust 基本保留同名方法与主要分支：动态/静态分区展开、伪统计跳过、稳定列/索引顺序、TSO 时间转换、桶与 TopN 的 `ValueToString`、列使用同时展示 global/partition、锁表查询以及分析状态行转发。

主要结构差异是 Go 方法直接依赖 `ShowExec` 的 session/domain/stats handle，而 Rust 把这些依赖抽成 `ShowStatsRuntime` 并将展示数据复制为轻量结构。这使独立测试可注入 runtime，也明确了 executor 与 session 的 crate 边界。

当前已验证的语义差异/简化包括：

- Go `versionToTime` 返回 MySQL `types.Time`，Rust先生成毫秒 `Timestamp`，最后由会话层格式化。
- Go 用真实 `cardinality.AvgColSize`、加载状态和 `MemoryUsage`；Rust生产适配器保留 average size，但加载状态是二值且内存字段为零。
- Go `GetStatsHealthy` 计算真实健康度；Rust生产适配器目前只给出 100/0。
- Go in-flight 路径读取并清理 stats handle；Rust目前固定输出 0。
- Go 的 LIKE 使用 collation wildcard；Rust runtime 的内部 matcher 只覆盖有限模式，而生产 SQL 行过滤由 session 层的 `show_like` 另行完成。
- Go 从 live InfoSchema/stats handle 读取；Rust生产路径通过 `DomainStatsContext` 的 catalog 与持久化统计快照读取。

语义证据来自 `pkg/executor/show_stats.go` 的同名函数及 `pkg/executor/show_stats_test.go`；Rust 对应回归位于独立文件 `pkg/executor/show_stats_test.rs`，没有把测试嵌入生产源文件。

## 扩展指南

新增一种 SHOW 统计视图时，优先在 `ShowExec` 增加单一公开 fetch 入口和必要的私有/表级 append 辅助，并在 `ConcreteSession::execute_show_stats` 增加 AST 分派、列名及错误上下文。若需要新数据，只扩展 `ShowStatsRuntime` 的最小方法，并同步实现 `SessionShowStatsRuntime`；不要让本文件直接依赖 session 或 domain crate。

修改分区语义时应集中检查 `visible_physical_tables`，同时审查 meta/healthy 的专用遍历，因为后两者没有调用该辅助方法。修改值展示需同步审查 `topNToRows`、`bucketsToRows`、生产 `value_to_string` 及 Go `statistics.ValueToString`，尤其是复合索引、日期时间、NULL 和缺失类型映射。

增加过滤下推时需明确 SQL 层后过滤与 `ShowFilters` 的职责，避免两处大小写/LIKE 规则不一致。若把分页改成流式，必须重构当前“全部写入 `rows` 后切片”的所有权模型，并评估错误发生后的部分结果行为。

测试应继续放在独立的 `pkg/executor/show_stats_test.rs`，优先扩展已有 `TestShowStatsMeta`、`TestShowStatsLocked`、`TestShowStatsHistograms`、`TestShowStatsBuckets`、`TestShowStatsBucketWithDateNullValue`、`TestShowStatsHasNullValue`、`TestShowColumnStatsUsage`、`TestShowAnalyzeStatus`；还应同步核对 Go `pkg/executor/show_stats_test.go`。兼容风险集中在结果列顺序、分区名、稳定排序、时间/NULL 格式和错误传播；性能风险集中在全 schema 扫描、统计结构克隆、全量物化及每个编码边界的重复解码。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 Rust/Go 文件；文件节点 `node --file pkg/executor/show_stats.rs --offset 1 --limit 1000` 返回目标文件完整 919 行，并报告该文件的引用面。
- RustCodeGraph `query ShowStatsRuntime --kind trait` 与 `node ShowStatsRuntime`：定位 trait 于 `pkg/executor/show_stats.rs:212`；`query SessionShowStatsRuntime --kind struct` 与 `node SessionShowStatsRuntime`：定位生产适配器于 `pkg/session/runtime/statistics.rs:1178`。当前索引未能按泛型 impl 方法名解析 `ShowExec::fetchShowStatsMeta`/`fetchShowStatsBuckets` 的 callers/callees，因此方法级边由精确源码引用补齐，而未臆造图边。
- 读过的生产边界：`pkg/executor/show_stats.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`、`pkg/session/runtime/statistics.rs`。
- Go 对照：`pkg/executor/show_stats.go`；同名 fetch/append/转换方法逐项核对。
- 测试证据：`pkg/executor/show_stats_test.rs` 和 `pkg/executor/show_stats_test.go`。Rust 测试覆盖 meta 过滤与动态/静态分区、锁、直方图、整数/日期桶、NULL 单列与复合索引、truncate、列使用 global/partition、分析任务状态；`TestShowStatusSnapshot` 属于相邻 SHOW 行为而非本文件的直接入口。
- 本任务是纯文档分析，按任务约束未运行 Cargo。结构验收使用任务指定命令，要求本文件存在且恰好包含上述 11 个固定二级章节。
