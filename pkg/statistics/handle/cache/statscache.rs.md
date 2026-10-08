# `pkg/statistics/handle/cache/statscache.rs`

源文件：[`statscache.rs`](statscache.rs)

## 文件定位

本文件是 `astersql-statistics-handle-cache` crate 的完整统计表缓存门面。crate 入口 `pkg/statistics/handle/cache/lib.rs` 将本模块私有声明后再导出其公开项；`pkg/statistics/handle/cache/Cargo.toml` 的 `[lib] path = "lib.rs"` 证明其 crate 边界。它把底层 `StatsCache`（实际存储、配额和驱逐）包装成实现 `stats_types::StatsCache` 的 `StatsCacheImpl`，并负责从 `mysql.stats_meta` 增量刷新表统计、批量提交变更和刷新健康度指标。

RustCodeGraph 的文件节点把本文件列为被 `stats_table_row_cache.rs`、`statscache_test.rs` 和 `restricted_sql.rs` 使用；进一步精确搜索表明，仓库内 Rust 代码对 `NewStatsCacheImpl`、`StatsCacheImpl` 的直接引用目前只出现在本文件、独立测试和基准中。因此可以确认接口已实现且由测试覆盖，但不能据此宣称它已经由 Rust 生产主链构造。Go 对照文件 `statscache.go` 则有来自 domain、session、executor、planner、DDL 等模块的生产调用者，这是 Go 主链现状而不是 Rust 调用证据。

## 核心职责

1. `StatsCacheImpl` 用 `RwLock<Arc<StatsCache>>` 发布当前缓存快照，在配额开关变化对应的两类后端语义下统一提供读取、更新、替换、清空、容量和驱逐 API。
2. `Update` 经 `StatsCacheHandle` 取得 session pool、schema 元信息和存储统计，查询按版本排序的 `mysql.stats_meta`，复用未变化的直方图数据，并把更新/删除按 10 条一批送入缓存。
3. `GetNextCheckVersionWithOffset` 从缓存最大统计版本向前回退五个 lease 对应的 TSO 偏移，覆盖“版本较小的事务反而较晚提交”的乱序窗口。
4. `UpdateStatsHealthyMetrics` 把缓存表分为 total、pseudo、无需 analyze 以及多个健康度区间并写入 Prometheus gauge。
5. `impl stats_types::StatsCache` 将上述具体实现暴露给 handle 层的动态接口；`StatsCacheHandle` 则把刷新所需的 handle 能力缩成最小边界，便于真实 handle 和独立测试共用。

## 主要符号

- `LeaseOffset: i64 = 5`：版本回看窗口的租约倍数。`GetNextCheckVersionWithOffset` 按 Go `time.Duration` 的有符号纳秒乘法及毫秒截断语义计算偏移。
- `batchSizeOfUpdateBatch: usize = 10`：刷新提交的默认批大小，与 Go 常量一致。
- `StatsCacheHandle`：刷新依赖边界。必需方法为 `lease`、`session_pool`、`table_stats_from_storage`；默认 `table_info_by_id` 先按表 ID 查找，再按分区 ID 查找，最后调用 `ModelMeta`，并把错误转为 `stats_types::Error`。
- `impl<T: stats_types::StatsHandle> StatsCacheHandle for T`：把完整 handle 自动适配到上述窄接口。`SharedHandle<H>` 再包一层 `Arc<H>`，避免在具体 handle 与 `Arc<dyn StatsHandle>` 间进行无效的 trait-object 强转。
- `StatsCacheImpl`：核心门面。`cache` 保存可替换的共享缓存；`handle` 在生产构造器中存在、测试构造器中可为空；`settings` 只用于测试显式选择 quota 后端和容量。
- `NewStatsCacheImpl` / `NewStatsCacheImplForTest`：分别构造带 handle 和不带 handle 的门面。底层缓存创建失败会返回 `CacheError`；无 handle 的实例不能执行 `Update`。
- `cacheOfBatchUpdate<F>`、`newCacheOfBatchUpdate`、`internalFlush`、`addToUpdate`、`addToDelete`、`flush`：保存更新表和删除 ID。任一侧在追加前已经达到 `batchSize` 时，会把两侧一起交给回调并清空；显式 `flush` 仅在至少一侧非空时执行。
- `Load` / `replace` / `Replace`：取得当前 `Arc` 快照，或原子地在写锁内交换快照。`replace` 关闭旧缓存并以新缓存成本更新 `statscacheinner::set_cost`；公开 `Replace` 共享传入门面的当前底层 `Arc`。
- `UpdateStatsCache`：quota 启用时原地调用 `StatsCache::Update`；禁用时用 `CopyAndUpdate` 创建新快照再替换。`skip` 控制指定表刷新时是否禁止推进全局最大版本。
- `Update`：增量刷新主入口，具体分支见“执行流程”。
- `Close`、`Clear`、`MemConsumed`、`Get`、`Put`、`TriggerEvict`、`WaitForAsyncUpdates`、`MaxTableStatsVersion`、`Values`、`Len`、`SetStatsCacheCapacity`：底层缓存操作的门面。其中 `Get` 支持与 Go 同名路径的 failpoint；`Clear` 新建失败只告警并保留原缓存。
- `UpdateStatsHealthyMetrics` / `statsHealthyBucketIndex`：健康度分类与桶边界映射。0 到 99 使用首个严格大于健康度的正上界，100 落入专用闭区间桶。
- `integer`：只接受 `SqlValue::Integer` 或 `Unsigned`；其他值返回带列号的错误。无符号数通过 `as i64` 保持当前移植语义。
- `LoadTimer`：RAII 计时器，在任何退出路径的 `Drop` 中把刷新耗时写入 `metrics::stats::StatsDeltaLoadHistogram`（指标初始化后才记录）。

## 执行流程

`Update(ctx, schema, ids)` 的刷新流程如下：

1. 进入时创建 `LoadTimer`；若 `handle` 缺失，立即返回 `statistics handle is not configured`。
2. `ids` 非空表示只刷新指定已分析表：SQL 使用 `tbl` 索引提示，把调用者切片复制后排序、去重并作为 `StringList` 绑定到 `IN (%?)`，同时令 `skip = true`。全量刷新使用 `idx_ver` 提示且 `skip = false`。两者都以 `GetNextCheckVersionWithOffset()` 为版本下界并按 `version` 排序。
3. 通过 `stats_util::call_with_sctx` 借用真实 session，再由 `exec_rows` 执行受限 SQL。session/SQL 错误在遍历前返回，不会产生缓存更新。
4. 创建批大小为 10 的 `cacheOfBatchUpdate`；回调将当前更新和删除列表交给 `UpdateStatsCache`。
5. 对每行解析 `version`、物理表 ID、修改数、行数、snapshot 和直方图版本；第六列为 `NULL` 时直方图版本按 0 处理。每轮先检查 `ExecutionContext::is_cancelled`。
6. `table_info_by_id` 找不到物理 ID（表或分区已删除）时把 ID 加入删除批。若旧缓存版本不小于新版本且 `TblInfoUpdateTS` 未变，则整行跳过。
7. 旧表存在、存储直方图版本非零且旧 `LastStatsHistVersion` 已覆盖它时，使用 `CopyAs(MetaOnly)` 复用列/索引统计；否则用 `table_stats_from_storage(info, id, false, 0)` 重载。加载错误只告警并保留旧缓存，`Ok(None)` 则安排删除。
8. 在新副本上写入 meta 字段。仅当 `LastAnalyzeVersion == 0` 且 snapshot 非零时用 snapshot 初始化它，避免已分析但只刷新 `_row_id` 等情形被误判为从未分析。
9. 将结果加入更新批，循环正常结束后显式 `flush` 残余并返回成功。

批处理有意保留 Go 的“部分提交”行为：满 10 条的批次已经可见后，如果随后遇到取消或行解码错误，未满的尾批不会刷新，也不会回滚前批。`statscache_test.rs::refresh_errors_cancel_without_flushing_pending_batch` 用 13 行和第 12 次加载取消验证缓存只保留前 10 行。

## 数据与状态

- 当前缓存由 `RwLock<Arc<StatsCache>>` 管理。普通读操作仅短暂持有读锁以克隆 `Arc`，随后在锁外访问底层对象；整体替换持有写锁，只交换指针而不复制完整统计。
- `handle` 是只读共享依赖。`None` 允许测试缓存门面、指标和更新后端，但刷新必须报错，不能静默退化。
- `settings: Option<(bool, i64)>` 固定测试实例的 quota 选择和容量，使测试不必修改进程全局配置；生产实例使用 `config::get_global_config().performance.enable_stats_cache_mem_quota` 和 `NewStatsCache()`。
- 缓存版本由底层 `StatsCache::Version` 定义。指定 ID 刷新传入 `skip = true`，避免只加载部分表却推进全局检查点，从而漏掉其他表的 delta。
- `cacheOfBatchUpdate` 的两个向量共享一次回调边界。触发条件是“追加前长度等于 batchSize”，所以批大小为 10 时第 11 次同侧追加先提交前 10 条，再保存新条目。
- 表统计通过 `Arc<StatisticsTable>` 共享。更新 meta 时总是基于新加载表或 `CopyAs(MetaOnly)` 的副本，不能修改已发布旧快照；测试会比较更新前后的 `Arc` 内容以验证这一点。
- 健康度 gauge 是进程级全局状态。每次刷新先在局部定长数组中完整重算，再逐项覆盖 gauge，而不是做增量加减。

## 依赖与调用关系

上游接口是 `stats_types::StatsCache`，本文件实现其完整方法集，并由 `pkg/statistics/handle/cache/lib.rs` 再导出。当前 Rust 索引和 `rg` 只确认独立测试 `statscache_test.rs`、基准 `bench_test.rs` 直接构造 `StatsCacheImpl`；未找到 Rust 生产模块调用 `NewStatsCacheImpl`，因此生产接线状态为“未由直接调用边验证”。`restricted_sql.rs` 的实际职责是统计持久化/KV 适配，虽然 RustCodeGraph 文件级关系把它列为使用者，但精确符号搜索没有显示它构造本门面。

主要下游关系为：

- `StatsCacheImpl::new` -> crate 内 `NewStatsCache` / `NewStatsCacheWithCapacity` -> `StatsCache` 底层后端（map 或 LFU）。
- `Update` -> `StatsCacheHandle::{lease, session_pool, table_info_by_id, table_stats_from_storage}`，以及 `stats_util::{call_with_sctx, exec_rows}`、`statistics::Table::CopyAs`。
- `UpdateStatsCache` -> `StatsCache::{Update, CopyAndUpdate}`；`replace` -> `StatsCache::{Close, Cost}` 和 `statscacheinner::set_cost`。
- 查询/容量/驱逐门面 -> 同名或相应的 `StatsCache` 方法。
- `UpdateStatsHealthyMetrics` -> `StatisticsTable::{MeetAutoAnalyzeMinCnt, IsAnalyzed, GetStatsHealthy}` 与 `astersql-statistics-handle-metrics` 的桶配置/gauge。
- `Get` -> `fail::eval` 的兼容 failpoint 路径。
- `LoadTimer::drop` -> `metrics::stats::StatsDeltaLoadHistogram`。

Cargo 直接依赖与这些调用一致：`statistics`、`stats-types`、`stats-util`、`config`、`vardef`、`model`、`metrics`、`log`、`fail` 和 handle metrics；内部 cache、LFU、map cache 由 crate 其他模块组装。测试另依赖 `infoschema`、`testfailpoint` 和 cache testutil。

## 错误处理与边界

- 构造时底层容量非法等错误直接作为 `CacheError` 返回；`Clear` 因签名无返回值，创建失败只写 warning，且不得关闭或替换当前缓存。
- `Update` 在缺少 handle、借 session/执行 SQL 失败、行整数类型非法、schema 元信息转换失败或上下文取消时返回错误。RAII 计时仍会记录这些退出路径。
- 单表存储加载失败被视为可能存在并发 DDL：写 warning、跳过该表、继续处理后续行；加载结果为 `None` 则删除该 ID。此分支不会把瞬时错误误作删除。
- 已删除表/分区的物理 ID 只写 debug 并排入删除批。表 ID 和分区 ID 都由 `table_info_by_id` 支持。
- 第六列 `last_stats_histograms_version` 允许 `NULL`；其他五列以及非空第六列必须是有符号或无符号整数。
- `statsHealthyBucketIndex` 仅对 0..=100 有定义；Rust 用 `debug_assert!`，release 构建不会为越界值强制失败，因此调用者必须维持 `GetStatsHealthy` 的范围不变量。
- trait 的 `Replace(&dyn StatsCache)` 强制向下转为同一 `StatsCacheImpl`，不同实现会 panic；这与 Go 的具体类型断言约束对应，扩展多实现替换时必须先重新设计契约。
- `RwLock::{read, write}.unwrap()` 假设锁不被 poison；回调或底层操作在持锁区 panic 可能导致后续访问 panic。缓存业务错误不通过该锁传播。
- `Close` 直接下传，测试证明连续调用两次可接受；具体幂等保证仍由底层 `StatsCache` 实现承担。

## 并发与资源生命周期

`RwLock` 只保护当前缓存 `Arc` 的发布点。读路径克隆后释放锁，使并发读不被长期阻塞；替换路径在写锁内交换指针，随后调用旧缓存 `Close`。由于已有读者持有旧 `Arc`，旧对象的内存释放仍由引用计数决定，但 `Close` 会立即启动其资源关闭语义。quota 后端的内部并发队列不在本文件实现，调用者需要用 `WaitForAsyncUpdates` 建立“异步写已对后续 Get 可见”的同步点。

批量刷新器是局部可变对象且其回调为 `FnMut`，不会跨线程共享。它没有 `Drop` 自动 flush：正常流程必须显式调用 `flush`；错误/取消提前返回时，尾批故意丢弃，已满批次则保留。此行为是兼容语义，不应随意改成事务式全有或全无。

`LoadTimer` 的析构覆盖正常和错误返回。健康度 gauge、全局配置和 failpoint 都是进程级共享状态，测试 `global_configuration_metrics_and_failpoints` 因此启动子测试进程隔离修改，并用 RAII guard 重置健康度 gauge。`SharedHandle` 与表对象使用 `Arc`，没有本文件自建线程、channel 或事务；session 的借出/归还由 `call_with_sctx` 管理，测试确认错误路径也归还 session。

## 与 Go 版本的对应关系

主对照为 `pkg/statistics/handle/cache/statscache.go`，Rust 基本逐段保留：五倍 lease 回看、两种索引提示、指定 ID 排序去重、按版本读取、每 10 条批量提交、旧直方图复用、DDL 竞态时跳过、snapshot 补 `LastAnalyzeVersion`、quota 原地更新与 map copy-on-write 分支、旧缓存关闭、健康度分类和 100 的闭区间桶。

显式语言适配包括：Go 的 `atomic.Pointer[StatsCache]` 对应 `RwLock<Arc<StatsCache>>`；Go interface 对应 `stats_types::StatsCache` trait；Go `context.Context` 对应 `ExecutionContext`；Go `[]any` SQL 参数对应 `SqlValue`；Go 指针表对象对应 `Arc<StatisticsTable>`；Go defer 计时对应 `LoadTimer::Drop`。Rust 为支持具体 handle 与动态 `Arc<dyn StatsHandle>` 增加了 `StatsCacheHandle`/`SharedHandle`，并为测试增加 `settings`，这些不是业务简化。

差异和迁移状态必须注意：Rust `Update` 接受 `&[i64]` 并复制后排序，不修改调用者切片；Go 原地排序/compact 可变参数。Rust `Clear` 可在测试 settings 下重建指定后端。Rust `integer` 显式校验行类型。Rust `statsHealthyBucketIndex` 使用 `debug_assert!`，Go 使用 `intest.Assert`。更重要的是，Go 文件已有 14 个生产使用文件，而 Rust 构造器当前未检索到生产调用，因此只能确认实现和测试对齐，不能确认应用主链已经切换到它。

Rust 独立测试覆盖范围明显超过同路径 Go 单测：除 Go 的批处理和指标桶外，还覆盖 TSO 偏移、put 后 delete 顺序、SQL 选择与参数、双后端版本推进、直方图复用、缺失/失败加载、取消时部分批、替换快照、动态 lease、配置/指标/failpoint、真实 schema/分区和动态 trait handle 构造。Go 测试仍是分类规则和原始移植意图的重要证据。

## 扩展指南

- 修改增量 SQL、行布局或刷新条件时，入口是 `StatsCacheImpl::Update` 和 `integer`。必须同步 `statscache_test.rs` 中 SQL 文本/参数、刷新双后端、复用/缺失/失败、取消尾批和 schema/分区测试；列布局变化还需核对 `restricted_sql.rs` 提供的 `mysql.stats_meta` 语义。
- 增加 handle 所需能力时，优先扩展 `StatsCacheHandle` 的最小接口，同时更新对 `stats_types::StatsHandle` 的 blanket impl、`SharedHandle` 转发和测试 `TestHandle`，避免让缓存 crate 依赖完整上层实现细节。
- 改动批大小或 flush 时机时，保持“触发时两侧一起提交”和错误后的部分批语义；同步 Rust `TestCacheOfBatchUpdate`、`refresh_errors_cancel_without_flushing_pending_batch` 及 Go `TestCacheOfBatchUpdate`。
- 增加缓存后端或改变 quota 分支时，修改 `StatsCacheImpl::new`、`quota_enabled`、`UpdateStatsCache`、`Clear` 和 `replace`，并在 quota=true/false 两种 settings 下验证版本推进、旧快照隔离、成本指标和关闭生命周期。
- 改动健康度桶必须同时核对 handle metrics 的 `HEALTHY_BUCKET_CONFIGS`、`UpdateStatsHealthyMetrics` 和 `statsHealthyBucketIndex`，并更新 Rust/Go 的边界值及 gauge 期望；尤其保留 pseudo、unneeded analyze 与 total 的互斥/总量关系。
- 若要让 `Replace` 接受异构实现，不能只移除 `expect`；应先扩展 `stats_types::StatsCache` 的快照/克隆契约，明确资源所有权和旧缓存关闭责任。
- 若接入 Rust 生产主链，首先应在实际 handle 构造位置注入 `NewStatsCacheImpl`，并补独立集成测试证明周期性全量刷新、指定表刷新和关闭顺序；当前没有直接调用证据，不应仅靠现有单元测试宣称已接线。
- Rust 测试逻辑继续放在同目录独立 `statscache_test.rs`（由 `lib.rs` 的 `#[path]` 引入），不要内嵌回生产文件。性能变更可同步 `bench_test.rs` 的 update/put/get 基准。

## 验证依据

- RustCodeGraph `status`：项目索引存在，包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录文件清单包含 `statscache.rs`、Rust/Go 测试和 Go 对照。
- RustCodeGraph `node --file pkg/statistics/handle/cache/statscache.rs`：完整读取 553 行，确认全部常量、trait、结构、函数、impl、错误分支和 `stats_types::StatsCache` 适配；文件节点报告三个文件级使用者。
- RustCodeGraph 精确查询：`query NewStatsCacheImpl --kind function --json` 同时定位 Go 和 Rust 构造器；`query statsHealthyBucketIndex --kind function --json` 定位两种语言的桶函数。通用 `explore` 对常见调用词产生跨仓库噪声，精确 callers/callees 命令未返回可用边，因此没有把噪声结果当作事实。
- 已读模块与依赖：`pkg/statistics/handle/cache/lib.rs`、`pkg/statistics/handle/cache/Cargo.toml`。目标目录不存在 `doc.go`。
- 已读直接实现证据：`pkg/statistics/handle/cache/statscache.rs`；精确 `rg` 搜索 `StatsCacheImpl|StatsCacheHandle|NewStatsCacheImpl|UpdateStatsCache|statsHealthyBucketIndex` 用于补足调用图未解析出的引用，并确认生产构造调用缺失。
- 已读 Go 对照：`pkg/statistics/handle/cache/statscache.go`；RustCodeGraph 显示该 Go 文件被 domain、session、executor、planner、DDL 等 14 个文件使用。
- 已读独立测试：`pkg/statistics/handle/cache/statscache_test.rs` 全部 855 行、`pkg/statistics/handle/cache/statscache_test.go` 全部 202 行；另由精确引用确认 `pkg/statistics/handle/cache/bench_test.rs` 覆盖更新可见性与性能路径。
- 已读相邻直接证据：`pkg/statistics/handle/cache/stats_table_row_cache.rs` 和 `pkg/statistics/handle/restricted_sql.rs` 的相关文件节点，用于辨别 RustCodeGraph 的文件级“used by”关系与本实现真实构造调用的差异。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核每项结论均能回指上述符号、配置、Go 对照或独立测试。
