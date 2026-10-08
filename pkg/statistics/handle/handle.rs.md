# `pkg/statistics/handle/handle.rs`

## 文件定位

`handle.rs` 是 `astersql-statistics-handle` crate 的核心运行时实现。crate 根 `pkg/statistics/handle/lib.rs` 以 `pub mod handle` 声明本模块并用 `pub use handle::*` 再导出其公开 API；`pkg/statistics/handle/Cargo.toml` 则把该 crate 归入 Go 包 `pkg/statistics/handle` 的移植范围，并直接依赖 `history`、`storage`、`lockstats`、`types`、`util` 等统计子 crate。

文件中的 `Handle<B: HandleBackend>` 汇聚物理表统计缓存、DDL 事件队列、列使用记录、ANALYZE 作业、历史快照和统计版本号。应用侧的主要持有者是 `pkg/domain/domain.rs`：它用 `Arc<Mutex<Handle<DomainStatsBackend>>>` 共享句柄，并在 SQL/DML、ANALYZE、历史统计转储和启动加载路径上调用这里的 API。`pkg/statistics/handle/restricted_sql.rs` 负责持久层读写，再通过 `reconcile_cache_profiles` 或 `merge_cache_profiles` 把持久化视图合并回本句柄。

本文件不是纯门面：它包含可运行的缓存、增量、发布、历史编码/解码和生命周期逻辑。但 `attach_stats_collector`、`detach_stats_collector` 与 `StatsCache::wait_for_async_updates` 仍是兼容 Go API 的透传/空实现；私有的旧版文本 JSON 编解码函数目前只在本文件内部互相引用，没有生产入口调用，实际历史统计路径使用 storage crate 的 gzip `JsonTable` blocks。

## 核心职责

1. 维护物理表级统计状态。`TableStats` 保存行数、修改计数、版本、列/索引直方图、TopN、NDV、FMSketch 等；`StatsCache` 用物理表 ID 索引这些快照并提供容量及粗略内存计量。
2. 提供统计写入边界。`AnalyzeStatsStorage` 及 `Handle` 的发布/增量 API负责注册表、累积 DML 变化、分配版本，并原子替换缓存和历史状态。
3. 为缓存未命中生成伪统计。`physical_table_stats`/`stats_by_physical_id` 根据分区数量、临时表和系统库规则决定伪统计是否进入缓存。
4. 管理历史统计生命周期。发布或刷增量时可记录内存快照，之后按需编码为 gzip blocks，支持按版本读取、解码、显式删除和按保留期 GC。
5. 保存运行时辅助状态。包括列使用信息、ANALYZE 作业、DDL 事件、系统库 ID 缓存、lease 和初始化完成标记。
6. 抽象外部副作用。`HandleBackend` 把系统库查询、delta 持久化、worker 启停、资源关闭和 DDL handler 注册留给 Domain 等上层实现。

## 主要符号

- 常量与全局阈值：`STATS_OWNER_KEY`、`STATS_PROMPT` 对齐 Go owner 配置；`PSEUDO_PARTITION_CACHE_LIMIT` 固定为 64；`DEFAULT_DUMP_STATS_DELTA_RATIO` 为 `1/10000`。`DUMP_STATS_DELTA_RATIO_BITS` 用 `AtomicU64` 保存 `f64` 位型，`dump_stats_delta_ratio`、`set_dump_stats_delta_ratio` 及 Go 风格别名负责无锁读写。setter 拒绝负数、NaN 和无穷值。
- 错误与标识：`Error(String)` 是本文件统一错误；`StatsTableKey::new` 将库表名转为小写；`StatsMetaRow` 是 Domain 暴露的 `stats_meta` 风格摘要。
- 统计模型：`Bucket`、`ColumnStats`、`IndexStats`、`TableStats` 构成运行时统计树。`TableStats::pseudo` 创建已初始化的伪统计；`estimated_memory` 按固定对象开销及 TopN/桶数量估算缓存内存，不是精确分配器计量。
- 缓存：`StatsCache` 包装 `HashMap<i64, TableStats>`，并维护 `closed` 和 `capacity_bytes`。其 `set_capacity` 只记录非负上限，本文件没有主动驱逐算法；`wait_for_async_updates` 当前为空。
- 外部接口：`AnalyzeStatsStorage` 定义注册、记录 mutation、发布 ANALYZE 三个动作；`HandleBackend` 定义持久化和资源生命周期钩子。
- 主对象：`Handle<B>` 持有 `cache`、`backend`、`ddl_events`、`system_database_ids`、`next_stats_version`、`column_usage`、`analyze_jobs`、`historical_snapshots`、`historical_enabled`、failpoint 状态和 `lease`。
- 发布入口：`publish_runtime_stats_with_source` 校验并原子发布一批 ANALYZE profile；`flush_runtime_stats_deltas` 原子应用一批 DML delta；`apply_persisted_stats_delta` 和 `touch_stats_version` 是较窄的单表更新入口。
- 历史入口：`dump_historical_stats` 进行延迟编码，`historical_json_blocks` 取编码结果，`DecodeHistoricalJsonBlocks` 校验并还原统计，`historical_snapshot` 按不大于目标版本的最近快照查询。
- 缓存装载入口：`reconcile_cache_profiles` 以传入集合为权威并淘汰多余表；`merge_cache_profiles` 只更新指定表，保留无关表；`remove_stats_items` 只裁剪指定列/索引。
- 测试 failpoint：`EnablePanicWhenRecordingHistoricalStatsMetaForTest` 返回 `HistoricalMetaPanicGuard`。守卫用唯一 ID 注册，`Drop` 时注销；发布路径消费一个活动 ID 后 panic。

## 执行流程

### 构造与接线

`Handle::new` 先在“有 notifier 且非测试”时调用 `HandleBackend::register_ddl_handler`，随后创建空缓存、容量为 1000 的 DDL 队列和各类运行时容器。历史记录默认只在测试模式开启，lease 初始为零。Domain 通常再用 `Arc<Mutex<_>>` 共享该对象。

### DML 增量刷新

Domain 在 `pkg/domain/domain.rs` 的统计预刷路径中先区分锁表/非锁表 delta，并补出分区对应的全局表 delta，然后调用 `flush_runtime_stats_deltas(..., "flush stats")`。该函数先验证 source 非空、修改行数非负、所有表已注册；之后计算下一版本，克隆缓存和历史 map，在副本中用饱和加法更新行数/修改计数，并把实时行数下限钳制为 0。历史功能开启时同步附加未编码快照。所有步骤成功后才替换原状态并推进 `next_stats_version`，因此校验或编码失败不会留下部分内存提交。

`dump_stats_delta_to_kv` 是另一条后端持久化边界，直接委托 `HandleBackend::dump_stats_delta`。`preflush_stats_delta` 保留错误给 ANALYZE/语句层；`flush_stats` 则吞掉错误并调用 `warn_flush_error`，适合后台尽力刷新。

### ANALYZE 发布

`publish_runtime_stats` 把 source 固定为 `analyze` 后转到 `publish_runtime_stats_with_source`。后者依次验证：source 非空；同批物理 ID 不重复且已注册；若给出 jobs，每个 job 至少有一个物理 ID、只能引用本批 profile，并且 jobs 的物理 ID 并集完整覆盖 profiles。多个 job 可以覆盖同一物理表，以兼容 Go 中列与索引分别执行全局合并作业。

提交时先克隆缓存和历史 map。每个 profile 被写入同一版本，`last_analyze_version`/`last_stats_hist_version` 同步推进，`modify_count` 清零，`analyze_count` 取当前实时行数，`pseudo` 置为 false；历史开启时保存内存快照。最后一次性替换缓存和历史、把版本水位推进到两者最大值，并用 `record_analyze_jobs` 更新作业列表。`AnalyzeStatsStorage::analyze_table_stats` 复用这条原子提交边界，而不是另写简化发布流程。

### 启动加载与 schema 对账

`pkg/statistics/handle/restricted_sql.rs` 从持久化记录组装 `TableStats` 后调用两类入口。完整初始化/对账用 `reconcile_cache_profiles`：先移除传入集合之外的表，再保留已有表的重对象，刷新元信息并按新 schema 裁剪列和索引；lease 非零且对应统计版本非零时，已有列/索引会清掉 buckets/TopN 并标记未完整加载，以便异步加载。lite-init 用 `merge_cache_profiles`，只合并请求的 ID，避免驱逐调用者未要求重载的缓存条目。

### 查询与伪统计

`physical_table_stats` 先查缓存，未命中则构造 `TableStats::pseudo`。非分区表通常可缓存；分区表只有在缓存表数小于 64 时可缓存；本地/全局临时表永不缓存。随后 `is_system_table` 检查 database ID：内存 schema 或已缓存的系统库直接命中，否则询问 backend；系统库或查询报错都返回临时伪统计而不入缓存，普通库才缓存伪统计。`non_pseudo_physical_table_stats` 只返回已存在且非伪的引用。

### 历史统计

发布和 delta 刷新只保存 `HistoricalTableStats { stats, source, json_blocks: None, created_at }`。`dump_historical_stats` 按版本倒序寻找最近快照，仅在 `json_blocks` 尚为空时调用 `encode_historical_json_blocks`。编码函数把运行时模型映射为 storage crate 的 `JsonTable`，将本文件独有的布尔/类型元数据编码进名称字段，再调用 `json_table_to_blocks` 生成受最大列大小约束的 gzip blocks。

`DecodeHistoricalJsonBlocks` 先验证 gzip member 头、块长度和结尾布局，再调用 storage crate 解码；它拒绝非历史表、额外 predicate columns、字段名与 map key 不一致、重复/错位 ID、非法布尔标记和畸形元数据。`gc_historical_stats_older_than` 依据 `SystemTime` 保留期清理；时间倒退时 `duration_since(...).unwrap_or_default()` 将年龄视为零，避免误删。

## 数据与状态

- 表状态以物理 ID 为键。分区与普通表共享 `TableStats` 结构，调用者负责传入正确物理 ID。
- `version` 是当前统计版本；`last_analyze_version` 和 `last_stats_hist_version` 记录 ANALYZE/直方图水位；`next_stats_version` 是句柄级单调水位，所有推进使用 `saturating_add` 防止整数回绕。
- `realtime_count` 经负 delta 后最小为零；`modify_count` 对非负修改行数做饱和累加。ANALYZE 成功时修改计数清零，并将 `analyze_count` 设置为实时行数，供后续自动分析比例使用。
- 列与索引都保存 NDV、null count、总大小、相关度、TopN、桶和 FMSketch。列另有字段类型和平均大小；索引另有 CMSketch/完整加载状态。
- 历史 map 的值按插入顺序保存，查询使用反向遍历寻找 `stats.version <= requested_version` 的首项。找不到历史项时 `historical_snapshot` 会回退为当前缓存的非历史视图；`historical_json_blocks` 不做这种回退。
- `remove_tables` 删除缓存和对应列使用记录，但故意保留历史快照，等待 GC；`remove_historical_snapshots` 才显式删除历史。
- `clear` 只清缓存、DDL 队列、backend 会话统计列表和系统库 ID 缓存。列使用、ANALYZE 作业和历史快照属于嵌入服务状态，必须保留，这一点由 `handle_test.rs::clear_preserves_embedded_service_state` 固化。

## 依赖与调用关系

上游调用证据：

- `pkg/domain/domain.rs::DomainHistoricalStatsStore::dump_historical_stats` 调用 `dump_historical_stats` 和 `historical_json_blocks`；`DomainStatsContext::decode_historical_json_blocks` 调用公开解码函数。
- `pkg/domain/domain.rs` 的 delta 提交路径调用 `flush_runtime_stats_deltas`，然后从 `stats_meta` 取出待持久化快照；ANALYZE 路径经 `AnalyzeStatsStorage::analyze_table_stats` 发布，并把历史任务交给 worker。
- `pkg/statistics/handle/restricted_sql.rs::reconcile_tables_impl` 在事务提交后调用 `reconcile_cache_profiles` 或 `merge_cache_profiles`，确保内存视图只在持久层成功后更新。
- `pkg/statistics/handle/bootstrap.rs` 调用 `StatsCache::wait_for_async_updates`；当前实现为空，因此这里只保留 API 顺序，没有真实等待语义。
- RustCodeGraph 报告该文件被 Domain、executor、DXF 等 55 个文件引用；对几个核心方法执行 `callers/callees` 未返回边，因此以上精确调用点又用 `rg` 补证。

下游依赖证据：

- `astersql_statistics_handle_storage`：`JsonTable`/`TableStats` 映射、gzip block 编解码及 bucket/TopN 类型。
- `astersql_statistics_handle_history`：`HistoricalStatsMeta` 和单列最大大小 `MAX_COLUMN_SIZE`。
- `crate::runtime_stats`：运行时作业、列使用和历史摘要类型。
- 标准库 `HashMap`/`HashSet`/`VecDeque` 保存状态，`AtomicU64` 保存全局阈值与 failpoint ID，`Arc<Mutex<_>>` 只用于 failpoint 内部；主 `Handle` 的跨线程互斥由上层 Domain 提供。

Cargo 中还有多个 `cfg(any())` 依赖，按 Rust 条件编译语义该条件恒为 false；它们记录更完整 Go 子系统的移植边界，但不是本文件当前编译路径的直接可用依赖。

## 错误处理与边界

- `set_dump_stats_delta_ratio` 使用断言处理非法进程配置，会 panic 而非返回 `Error`。
- 注册要求正 table ID 且不能重复；mutation 要求 `modified_rows >= 0` 且表已存在。row delta 允许为负，但最终行数不低于零。
- 批量发布/刷新在修改副本前完成可预见校验，并只在全部成功后替换原状态。`pkg/testkit/stats_runtime_aster_unit_test.rs::publish_rejects_unknown_profile_without_partial_cache_history_or_jobs` 证明未知 profile 或空 source 不会部分修改缓存、历史或 jobs。
- 历史 source 在发布入口必须非空；真正编码时还拒绝 NUL 和控制字符。解码对 gzip 布局、UTF-8/元数据、ID、一致性和数值溢出进行显式检查。
- 旧版 `decode_legacy_historical_json_blocks` 通过提取数字重建对象，再用规范编码逐字节比对，拒绝尾随数字或非规范结构；但当前生产入口没有调用它，不应把它描述成现行持久格式。
- `stats_by_physical_id` 对系统库查询错误采取“返回但不缓存伪统计”的降级；`flush_stats` 只告警；相反 `preflush_stats_delta` 和显式发布/编码入口向上返回错误。
- 多处 poisoned mutex 使用 `expect`，测试 failpoint 明确 panic。这些属于进程内不变量破坏或测试注入，不转换为业务错误。

## 并发与资源生命周期

`Handle` 自身大部分方法要求 `&mut self`，不在内部为主状态加锁。生产共享由 `pkg/domain/domain.rs` 的 `Arc<Mutex<Handle<DomainStatsBackend>>>` 串行化；文档或新调用者不能假设其方法天然无锁并发安全。

进程级 dump 比例用 Acquire/Release 原子访问，读取无需锁。历史 failpoint 状态使用 `Arc<Mutex<HashSet<u64>>>`：每个 RAII guard 注册独立 ID，析构注销；触发时只消费一个活动 ID。`next_id` 用 Relaxed，因为唯一性只依赖原子递增，不承担其他内存同步。

缓存/历史批量提交采用 clone-then-swap，换取更大的瞬时内存开销来保证内存原子性；这不是持久层事务，跨内存与外部 KV 的原子性由 Domain/`restricted_sql` 的更外层流程协调。历史 JSON 延迟编码减少发布临界区工作量，但首次 dump 会在持有上层 Handle mutex 时完成编码。

`start_worker` 只委托 usage worker；`close` 的顺序是关闭 pool、标记缓存关闭、关闭 usage、关闭 auto-analyze、清系统库 ID 缓存。它不清除统计数据，也没有关闭 DDL 队列。`StatsCache::close` 仅设置标志，未实现等待或资源释放；真实资源生命周期在 backend。

## 与 Go 版本的对应关系

- Rust `Handle<B>` 对应 `pkg/statistics/handle/handle.go::Handle`，但 Go 通过嵌入 `StatsGC`、`StatsUsage`、`StatsHistory`、`StatsAnalyze`、`StatsReadWriter`、`StatsCache` 等接口组合多个子系统；Rust 当前把一部分核心运行时状态直接收拢在单一泛型对象中，并通过 backend/storage/history crate 接线。
- `STATS_OWNER_KEY`、`STATS_PROMPT`、collector attach/detach、`Clear`、`NewHandle`、物理表查询、系统库判定、`FlushStats`、`StartWorker`、`Close` 都有直接 Go 对照。
- 伪统计缓存规则与 Go 一致：非分区表可缓存；分区伪统计缓存上限为 64；临时表与系统表不缓存；系统库 ID 缓存避免重复借 session。Rust 把 Go 的 InfoSchema/session 查询收敛到 `HandleBackend::system_schema`。
- Rust `clear` 保留 usage/analyze/history 状态，符合 Go `Clear` 只清 StatsCache、DDL channel、session delta collector 和 system DB cache 的语义；独立 Rust 测试专门防止误做“全量清空”。
- Go `NewHandle` 逐个构造所有嵌入服务和 DDL handler；Rust `new` 只初始化当前迁移所需状态并通过 backend 注册 handler。Cargo 中恒 false 的 `cfg(any())` 依赖显示其余子系统尚未进入当前编译接线，不能据此宣称完整等价。
- Rust 新增了面向当前运行时的批量原子发布、内存历史快照和 storage `JsonTable` 映射；这些行为在 Go 中分散于 analyze、history、storage 和 Domain 流程，并非 `handle.go` 单文件逐函数翻译。
- Go 的统计缓存具备异步更新实现；Rust `wait_for_async_updates` 仍为空。Rust collector attach/detach 也仍为透传，因此扩展时必须先确认调用链是否需要恢复真实行为。

## 扩展指南

- 新增表级状态时，应同步检查 `TableStats`、ANALYZE 发布、delta 刷新、`reconcile_cache_profiles`/`merge_cache_profiles`、历史 storage 映射和 `DecodeHistoricalJsonBlocks`，否则实时缓存与历史恢复会出现字段漂移。
- 修改历史格式时优先扩展 `astersql-statistics-handle-storage` 的 `JsonTable` 路径，并保留严格解码校验。若改变名称中的元数据编码或 gzip block 规则，需要同步 Domain 持久化/恢复测试；不要误改未接线的 legacy 私有路径来替代现行实现。
- 新增 backend 副作用应放入 `HandleBackend` 并由生产 backend 实现；同时补独立 `*_test.rs` fake backend 测试。不要把 Rust 测试嵌入本生产文件。
- 修改伪统计规则时，重点同步 Go `getStatsByPhysicalID`/`isSystemTable` 语义，覆盖分区第 64/65 项、临时表、内存 schema、backend 错误和普通库首次缓存。
- 修改发布流程必须保持“先完整校验和构造副本，后一次性替换”的不变量，并在 `pkg/testkit/stats_runtime_aster_unit_test.rs` 增加失败前后缓存、历史、jobs 全相等的回归。
- 修改 `clear`/`remove_tables`/`close` 时区分缓存状态、嵌入服务状态、历史持久化生命周期和真实 backend 资源，避免提前丢失历史或关闭顺序倒置。
- 实现 `wait_for_async_updates` 或 collector attach/detach 前，应先定位 Go 注册点和调用者契约；当前空实现是已知迁移边界，不能只为测试通过加入无行为桩。
- 性能风险主要来自批量 clone cache/history、历史编码时的大块内存、线性倒序历史查找和粗略缓存计量；扩大数据规模前应增加基准或大对象测试，而非假设计量值等于真实堆占用。

## 验证依据

- 源码：`pkg/statistics/handle/handle.rs` 全部 1756 行；主要证据符号包括 `Handle`、`StatsCache`、`TableStats`、`publish_runtime_stats_with_source`、`flush_runtime_stats_deltas`、`stats_by_physical_id`、`dump_historical_stats` 和 `DecodeHistoricalJsonBlocks`。
- crate 边界：`pkg/statistics/handle/Cargo.toml`、`pkg/statistics/handle/lib.rs`。目标目录及其 `pkg/statistics` 父层没有 `doc.go`。
- Go 对照：`pkg/statistics/handle/handle.go`；集成行为参考 `pkg/statistics/handle/handletest/handle_test.go` 中的真实 Domain/SQL 统计读取与版本更新场景。
- Rust 独立测试：`pkg/statistics/handle/handle_test.rs::clear_preserves_embedded_service_state`；`pkg/statistics/handle/analyze_runtime_aster_unit_test.rs::handle_exposes_lease_and_forced_delta_flush` 与 `canonical_handle_context_observes_live_analyze_and_history_state`；`pkg/testkit/stats_runtime_aster_unit_test.rs::publish_rejects_unknown_profile_without_partial_cache_history_or_jobs`。
- 真实调用点：`pkg/domain/domain.rs` 的历史转储、解码、delta flush 与 ANALYZE 路径；`pkg/statistics/handle/restricted_sql.rs::reconcile_tables_impl` 的事务后缓存对账。
- RustCodeGraph：索引状态为 11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/statistics/handle` 和分段 `node --file pkg/statistics/handle/handle.rs` 用于读取全貌，`query` 定位关键入口。核心方法的 `callers/callees` 查询未返回边，因而用上述精确 `rg` 调用点作为补充证据。
- 本任务只新增说明文档，未修改 Rust/Go/Cargo，也未运行 Cargo。交付前以任务指定命令确认目标文档恰有 11 个固定二级章节，并人工检查占位、未接线路径与 Go 差异均已明确标注。
