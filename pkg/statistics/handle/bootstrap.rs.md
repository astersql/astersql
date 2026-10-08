# `pkg/statistics/handle/bootstrap.rs` 逻辑说明

## 文件定位

`bootstrap.rs` 属于 `astersql-statistics-handle` crate；crate 边界由 `pkg/statistics/handle/Cargo.toml` 定义，`pkg/statistics/handle/lib.rs` 通过 `pub mod bootstrap` 与 `pub use bootstrap::*` 将本文件的公开项导出。本文件把 `mysql.stats_meta`、`mysql.stats_histograms`、`mysql.stats_top_n` 和 `mysql.stats_buckets` 的查询结果组装为 `handle.rs` 中的 `StatsCache` / `TableStats`，提供轻量初始化 `Handle::init_stats_lite` 与完整初始化 `Handle::init_stats` 两条路径。

当前接线必须与算法本身分开理解：仓库内 `BootstrapBackend` 的实现仅见 `pkg/statistics/handle/bootstrap_test.rs` 的 `HistogramBackend` 和 `TransactionBackend`。生产 Domain 的启动与刷新入口 `pkg/domain/domain.rs::initialize_stats`、`update_stats` 调用的是 Domain 自身的 `init_stats_lite`，其数据来源为 `stats_store.init_tables_lite`，并没有调用本文件的 `Handle::init_stats_lite`。因此，本文件目前是已导出、受单元测试覆盖的通用 bootstrap 实现，但尚无生产后端把它接入 Domain 主启动链。

## 核心职责

- 定义 bootstrap 后端契约 `BootstrapBackend`，把事务控制、四类统计表查询、InfoSchema 存在性检查、表结构查询、内存探测、缓存配额和初始化进度更新隔离在算法之外。
- 用 `gen_init_stats_meta_sql`、`gen_init_stats_histograms_sql`、`gen_init_stats_top_n_sql_for_indexes`、`gen_init_stats_buckets_sql_for_indexes` 生成与 Go 版本一致的高优先级查询文本；`QuerySelection` 和 `GenHistogramSqlOptions` 同时描述实际后端查询范围与 SQL 生成范围。
- 先加载表级 meta，再建立列/索引直方图摘要；完整模式在缓存配额允许时继续加载索引 TopN 和桶，并更新 `fully_loaded` / `pre_scalar_ready` 状态。
- 支持全量刷新和指定物理表 ID 的局部刷新：`publish_cache` 对空 `table_ids` 整体替换缓存，对非空列表逐表合并并关闭临时缓存。
- 用 `is_full_cache` 在进程总内存四分之一和配置配额两条阈值上实施加载降级，防止 CMS、TopN 和桶无限占用内存。

## 主要符号

- 常量 `INIT_STATS_STEP`（500）决定 `LoadStrategy::MaxTableId` 的分页宽度；`INIT_STATS_PERCENTAGE_INTERVAL`（33.0）决定 meta/直方图与 TopN 阶段的进度点。
- 输入行类型 `MetaRow`、`HistogramRow`、`TopNRow`、`BucketRow` 分别映射四张 `mysql.stats_*` 表中本算法需要的字段。它们是后端与纯组装逻辑之间的数据边界，而不是数据库行读取器本身。
- `QuerySelection::{All, TableIds, Range}` 表示直方图查询的全表、显式 ID 列表或半开区间；`GenHistogramSqlOptions::{paging, table_ids, selection}` 负责构造并转换该选择。
- `BootstrapBackend: HandleBackend` 是核心依赖倒置接口。除基础 `HandleBackend` 生命周期能力外，它要求 `begin` / `commit`、四类查询、`physical_id_exists`、`table_info`、`total_memory`、`stats_cache_quota`、`init_concurrency` 和 `set_init_percentage`。
- `LoadStrategy::{MaxTableId, TableList}` 决定任务分片。`new` 在未指定表列表时选最大 ID 扫描；`tasks` 产生 `[start, end)`；`total_task_count` 保留 Go 的进度任务数估算语义。
- 公开入口 `Handle::init_stats_lite` 只加载 meta 与直方图存在性/分析标志；`Handle::init_stats` 加载完整索引统计并执行内存降级。
- 内部组装函数 `load_meta`、`load_histograms_lite`、`load_histograms_full`、`load_top_n`、`load_buckets` 分别更新缓存的一个阶段；`publish_cache` 是最终发布边界。
- `finish_transaction` 合并业务结果和 commit 结果，优先保留业务错误；`is_full_cache` 是公开的容量判定函数。

## 执行流程

轻量路径 `Handle::init_stats_lite` 的顺序如下：

1. 调用 `BootstrapBackend::begin`。即使 begin 返回错误，也调用 `commit`，然后由 `finish_transaction` 返回 begin 错误；这一行为由 `init_stats_lite_commits_when_begin_fails` 固定。
2. `load_meta` 调用 `query_meta(table_ids)`，过滤 `physical_id_exists == false` 的残留统计，为每个有效物理表创建 `TableStats`。加载后等待临时缓存的异步更新边界。
3. `GenHistogramSqlOptions::table_ids` 把空列表解释为全表、非空列表解释为 `TableIds`，后端据此 `query_histograms`。
4. `load_histograms_lite` 只更新表级 `stats_version` / `last_analyze_version`，并为索引写入 `analyzed`、为列写入 `analyzed_or_synthesized`，不加载 CMS、TopN 或桶。
5. `publish_cache` 在全量模式替换全局缓存，在局部模式逐表合并；最后提交事务。

完整路径 `Handle::init_stats` 的顺序如下：

1. 将进度置为 0，先调用 `total_memory`，再开始事务。无论闭包最终成功或失败，函数退出前都会把进度置为 100；若内存探测失败则尚未 begin，因此不会 commit。
2. `load_meta` 后把进度置为 33，并用 `LoadStrategy::new(max_table_id, table_ids)` 生成范围任务。
3. 第一阶段对每个范围调用 `query_histograms(Range)`。每批加载前以 `is_full_cache` 判断是否跳过 CMS；`load_histograms_full` 仍会保留直方图元信息、版本、NDV、空值数、大小和相关度。
4. 第二阶段仅在缓存未满时运行。每个范围先查询拥有桶的表 ID，再查询 TopN；`load_top_n` 忽略找不到目标索引的行，也忽略“没有 CMS 且统计版本不高于 1”的旧版索引，随后按编码值排序 TopN。没有桶的表会在此阶段直接把全部索引标记为完整加载。
5. 将进度置为 66。第三阶段仅在缓存未满时加载桶；`load_buckets` 将桶追加到对应索引并把触及表的索引标记为 `fully_loaded`。阶段完成后，临时缓存中的全部表被设置 `pre_scalar_ready = true`。
6. 等待更新、发布缓存，并由 `finish_transaction` 合并业务结果与 commit 结果。当前 Rust 实现逐范围串行执行；`init_concurrency().max(1)` 的结果仅赋给 `_concurrency`，没有用于创建 worker。

## 数据与状态

`load_meta` 建立后续阶段的主键集合：`TableStats::physical_id` 来自 `MetaRow::table_id`，`version`、`modify_count`、`realtime_count` 直接映射；`last_analyze_version` 初始化为 `snapshot`，`last_stats_hist_version` 为 `max(last_histogram_version.unwrap_or(snapshot), snapshot)`，防止直方图版本倒退。只有通过 `physical_id_exists` 的物理表进入缓存，`max_physical_id` 也只统计这些表。

`load_histograms_full` 还要求 `table_info(table_id)` 存在，并分别用 `TableInfo::index_ids` / `column_ids` 拒绝已从 schema 删除的对象。索引的 `cms_loaded` 仅在缓存未满且 `cm_sketch` 非空时为真；无论 CMS 是否加载，索引的版本、NDV、空值数、总列大小和相关度都会保留。列的 `analyzed_or_synthesized` 与 `loaded_or_evicted` 在 `stats_version != 0`、`ndv > 0` 或 `null_count > 0` 任一成立时置真，对应“新增带默认值列可合成统计”的语义。

`StatsCache` 的内存占用由 `handle.rs::StatsCache::memory_consumed` 汇总 `TableStats::estimated_memory`。`is_full_cache` 在 `consumed >= total_memory / 4` 或“非零 quota 且 `consumed >= quota`”时返回真。注意当 `total_memory < 4` 时整数除法阈值为 0，任何缓存（包括零占用）都会视为已满；后端必须提供合理的物理内存值。

全量发布把临时 `StatsCache` 移入 `Handle::replace_cache`；局部发布则 `drain` 临时缓存、覆盖全局同 ID 条目、等待全局缓存更新，然后 `close` 临时缓存。`handle.rs` 中当前 `wait_for_async_updates` 是空实现，`close` 只记录关闭标志，但本文件保留这些调用点以维持与 Go LFU 异步准入及资源关闭协议的结构对齐。

## 依赖与调用关系

上游关系：`pkg/statistics/handle/lib.rs` 导出本模块；仓库局部搜索确认 `Handle::init_stats` 的直接调用仅在 `pkg/statistics/handle/bootstrap_test.rs`，`Handle::init_stats_lite` 除同文件测试外还出现在名称相同但类型不同的 Domain/handletest 路径。由于生产代码没有 `BootstrapBackend` 实现，泛型约束阻止生产 `Handle` 实例调用本文件入口。`pkg/domain/domain.rs::initialize_stats` 是实际启动统计入口之一，但它调用 Domain 固有方法而非本文件方法。

下游关系：本文件直接依赖 `crate::handle` 的 `Handle`、`HandleBackend`、`StatsCache`、`TableInfo`、`TableStats`、`IndexStats`、`ColumnStats`、`Bucket` 和 `Error`。所有外部 I/O 通过 `BootstrapBackend` 完成，算法层本身不直接依赖 SQL 执行器、InfoSchema 或内存探测库。`Cargo.toml` 表明该 crate 直接依赖 statistics、types、meta model、kv、datum、stmtctx 及 history/lockstats/storage/types/util 等子 crate；本文件实际源码仅使用同 crate 的 handle 抽象和标准库 `HashSet`。

SQL 生成函数当前没有被本文件的加载流程直接调用：加载流程把结构化范围传给后端的 `query_*` 方法。因此后端实现若使用这些 SQL helper，必须自行保证 `QuerySelection` 与 SQL 参数一致。RustCodeGraph 已将 `init_stats`、`init_stats_lite`、`load_meta`、`load_histograms_full`、`load_top_n`、`load_buckets`、`is_full_cache` 定位到本文件；其全局 callers/callees 命令未在限定时间内返回，直接调用关系由上述局部源码搜索补证。

## 错误处理与边界

- 所有可恢复 I/O 错误以 `handle.rs::Error` 返回，`?` 会立即停止当前加载阶段；临时缓存不会发布，所以整体发布保持“成功后可见”。本文件没有 rollback 能力，失败后仍尝试 `commit`，行为与 Go 中 defer commit 的现状对齐，而不是常规事务回滚语义。
- `finish_transaction` 在业务和 commit 同时失败时返回业务错误；只有业务成功时才暴露 commit 错误。begin 失败后也会尝试 commit，完整和轻量测试都覆盖了这一边界。
- `GenHistogramSqlOptions::paging` 要求 `lo < hi`；`table_ids` 要求所有 ID 非负；TopN/桶 SQL 的分页模式也要求有效范围。`LoadStrategy::new` 的最大 ID 模式要求非负，`TableList::total_task_count` 要求列表非空。这些违规输入使用 `assert!` panic，而非 `Error`。
- 查询结果中已删除的 physical ID、找不到 `TableInfo` 的表、schema 中不存在的列/索引、缓存中不存在的表以及找不到目标索引的 TopN/桶行都会被跳过，不会使初始化失败。
- `load_top_n` 只以“TopN 查询返回过的 table_id”作为 touched 集合。如果某表没有任何 TopN 行，即便 `tables_with_buckets` 也不含该表，本函数不会把它的索引标为完整；这与 Go 分块函数依赖当前 table 对象的处理范围相近，扩展时需用测试明确空结果语义。
- 完整路径的桶阶段只有在进入该阶段时才为所有临时表设置 `pre_scalar_ready`。若缓存进入阶段前已满，或加载 TopN 后变满，则跳过整个桶分支，保持 false；若进入分支后中途达到配额而提前 break，当前实现仍把所有表置 true。该标志只是 Rust 的简化状态，不等同于 Go 对每张表执行真实 `CalcPreScalar` 的数值计算。

## 并发与资源生命周期

`Handle::init_stats` 读取 `BootstrapBackend::init_concurrency`，但当前只保存为 `_concurrency`，三个任务阶段均通过普通 `for` 循环串行执行。因此后端不应假设 query 方法会被本算法并发调用；与 Go `initstats.RangeWorker` 的并发模型尚未对齐。

临时缓存从 `load_meta` 创建，组装阶段独占 `&mut StatsCache`。全量成功时所有权转移给 Handle；局部成功时条目被 drain 后显式关闭临时缓存。加载阶段在读写相邻批次之间调用 `wait_for_async_updates`，为未来异步 LFU 实现保留顺序屏障；当前 `StatsCache` 实现是 `HashMap` 且该方法为空，不产生线程同步。

事务生命周期由入口统一管理：完整路径的内存探测发生在 begin 之前；begin 之后无论中间查询是否失败都执行一次 commit。进度状态在完整路径的最外层收尾为 100，即错误也表示“初始化尝试已结束”，而非“加载成功”。轻量路径不更新进度。`BootstrapBackend` 的 `&mut self` 方法签名也将同一 Handle 上的事务与查询限制为顺序可变访问。

## 与 Go 版本的对应关系

主要一一对应关系如下：Rust `load_meta` 对应 Go `initStatsMeta` / `initStatsMeta4Chunk`；`load_histograms_lite` 对应 `initStatsHistograms4ChunkLite`；`load_histograms_full` 对应 `initStatsHistograms4Chunk`；`LoadStrategy` 对应 `loadStrategy`、`maxTidStrategy` 和 `tableListStrategy`；`load_top_n` 对应 `initStatsTopN4Chunk`；`load_buckets` 对应 `initStatsBuckets4Chunk`；两个公开入口对应 Go `InitStatsLite` / `InitStats` 及其 with-session 核心函数；`is_full_cache` 对应 Go `isFullCache`。

已保持的关键语义包括：500 的分页步长、33% 的阶段进度、SQL 字面量和旧集群 `ORDER_INDEX(...,tbl)` 提示、meta 先于 histogram、snapshot 初始化分析版本、过滤已删除 schema 对象、列合成统计判定、旧版无 CMS 索引跳过 TopN、TopN 排序、内存四分之一或配置配额阈值、全量替换/局部合并和 defer 风格 commit。

尚未完全对齐的地方必须视为迁移限制：

- Go 使用 `initstats.RangeWorker` 按配置并发度执行 histogram、TopN、bucket 三阶段；Rust 读取并发度但串行执行。
- Go 从 SQL record set 分 chunk 流式读取并在 chunk 之间等待 LFU 异步更新；Rust 后端一次返回 `Vec<Row>`，会把单个范围的全部结果保存在内存中。
- Go 对 CMS/TopN 做真实反序列化并保留对象；Rust 只根据字节是否为空记录 `cms_loaded`，没有解码 `cm_sketch` 内容。
- Go 列/索引对象携带完整 schema 类型、主键属性、直方图对象及 loaded status；Rust 的 `TableInfo` 和统计结构是摘要模型。
- Go 桶阶段调用 `CalcPreScalar` 计算标量；Rust 仅设置 `pre_scalar_ready` 布尔值。
- Go 的 panic recovery、日志、failpoint、系统 session 池、内部事务来源标记和 record-set close 不在本文件的抽象实现中。
- Go 生产 `Handle` 已接入该路径；Rust 本文件没有生产 `BootstrapBackend` 实现，Domain 当前使用另一条 lite 恢复实现。

## 扩展指南

接入生产时，首要扩展点是为真实 statistics Handle 后端实现 `BootstrapBackend`，并决定 Domain 是否统一调用本文件入口。实现必须把 `QuerySelection::Range` 解释为半开区间，提供与同文件 SQL helper 一致的排序，且在 `table_info` / `physical_id_exists` 中使用同一 InfoSchema 快照。接线后应在 Domain 启动、刷新与指定表恢复场景添加独立集成测试，不能仅依赖 mock backend。

若补齐 Go 并发语义，修改重点是 `Handle::init_stats` 中三个 `strategy.tasks()` 循环和目前未使用的 `_concurrency`。需要保持阶段间顺序、每批异步缓存屏障、首个错误传播、配额短路以及进度单调性；后端若需并发，还必须重新设计当前 `&mut self` 接口。性能测试应关注范围结果 `Vec` 峰值、锁竞争与局部刷新大量离散 ID 时的任务数。

若增强统计保真度，应修改 `IndexStats` / `ColumnStats` 及 `load_histograms_full`，真实解码 CMS，计算 pre-scalar，并明确畸形编码的错误策略。任何改变都要同步 `pkg/statistics/handle/bootstrap_test.rs`；SQL 文本变化还要同步 `pkg/statistics/handle/bootstrap_test.go`，并与 `bootstrap.go` 的对应 helper 核对旧集群索引提示兼容性。

建议补充的回归边界包括：`last_histogram_version < snapshot`、删除表/删除列或索引、无 TopN 且无桶、TopN 有桶/无桶、版本 1 无 CMS、配额在三个阶段之间变满、业务错误与 commit 双重失败、commit 单独失败、局部刷新不影响未指定表，以及多范围任务末端超过最大 table ID。若未来改变当前断言，需决定无效 ID/范围应继续 panic 还是转为 `Error`，避免后端生成非法 SQL。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/bootstrap.rs` 确认目标文件已索引且含 56 个符号。
- RustCodeGraph `node --file pkg/statistics/handle/bootstrap.rs --offset 1 --limit 620`：读取目标文件 551 行全貌；`query` 分别定位 `BootstrapBackend`、`init_stats`、`init_stats_lite`、`load_meta`、`load_histograms_full`、`load_top_n`、`load_buckets`、`finish_transaction` 和 `is_full_cache`。全局 `callers/callees` 查询在 30 秒限定内未返回，因此未把其空输出当作“没有调用者”的证据。
- 源与边界文件：`pkg/statistics/handle/bootstrap.rs`、`pkg/statistics/handle/handle.rs`、`pkg/statistics/handle/lib.rs`、`pkg/statistics/handle/Cargo.toml`。
- Go 对照：`pkg/statistics/handle/bootstrap.go`；重点核对 meta/histogram/TopN/bucket 组装、SQL helper、load strategy、`InitStatsLite`、`InitStats`、`isFullCache` 和并发 worker。
- 独立测试：`pkg/statistics/handle/bootstrap_test.rs` 覆盖 SQL、索引直方图字段保留、内存探测失败、begin 失败后的 commit 与进度收尾；`pkg/statistics/handle/bootstrap_test.go` 覆盖同路径 SQL 字面量。
- 接线搜索：仓库内 `BootstrapBackend` 仅有上述 Rust 测试的两个实现；`pkg/domain/domain.rs::initialize_stats`、`update_stats`、`init_stats_lite` 证明当前生产启动路径使用 restricted statistics store，而不是本文件的泛型入口。
- 本任务只新增说明文档，不修改运行时代码，未运行 Cargo。交付前使用任务文件给定的结构命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核所有“当前支持”结论均有上述源码、测试或调用搜索依据。
