# `pkg/statistics/handle/cache/statscacheinner.rs`

## 文件定位

本文件属于 Cargo crate `astersql-statistics-handle-cache`（`pkg/statistics/handle/cache/Cargo.toml`），由同目录 `lib.rs` 作为私有模块 `statscacheinner` 装配并将其公开项重新导出。它位于上层缓存协调器 `StatsCacheImpl`（`statscache.rs`）和内层存储抽象 `cache_internal::StatsCacheInner`（`internal/inner.rs`）之间：上层只面对 `StatsCache`，本文件再把操作委派给 LFU 或普通 Map 实现。

该文件不是完整的统计信息加载器，也不决定表统计如何从系统表构造；这些流程在 `statscache.rs`。它负责的是单个内存缓存实例的存储策略选择、同步访问、最大统计版本水位和操作指标。

## 核心职责

- `NewStatsCache` 从全局配置 `performance.enable_stats_cache_mem_quota` 和动态变量 `vardef::StatsCacheMemQuota` 读取默认策略与容量。
- `NewStatsCacheWithCapacity` 在配额模式下构造 `cache_lfu::NewLFU(capacity)`，否则构造 `cache_map::NewMapCache()`，并统一装箱成 `Box<dyn StatsCacheInner>`。
- `StatsCache` 用 `Mutex` 串行化对内层 trait 对象的访问，用 `AtomicU64` 保存该缓存生命周期中见过的最大 `StatisticsTable::Version`。
- `Get`、`Put`、`Update` 和删除路径更新命中、未命中、更新、删除指标；`set_cost` 供 `statscache.rs::replace` 在替换缓存后更新全局 cost gauge。
- 同时支持原地批量更新 `Update` 和 Map 模式使用的复制更新 `CopyAndUpdate`，从而服务 `StatsCacheImpl::UpdateStatsCache` 的两条策略分支。

## 主要符号

- `pub struct StatsCache { inner, maxTblStatsVer }`：公开缓存门面。`inner` 是受互斥锁保护的动态存储实现；`maxTblStatsVer` 是只前进的版本水位。
- `NewStatsCache() -> Result<StatsCache, CacheError>`：生产默认构造入口，读取进程级配置后调用显式容量构造器。
- `NewStatsCacheWithCapacity(quota, capacity)`：可测试的策略选择入口。LFU 构造错误被转换为本 crate 的 `CacheError`；Map 构造不返回错误。
- `Len`、`Values`、`Cost`、`SetCapacity`、`Close`、`TriggerEvict`、`WaitForAsyncUpdates`：薄委派方法，分别对应 `StatsCacheInner` 的同名契约。
- `Get(id) -> (Option<Arc<StatisticsTable>>, bool)`：返回共享的只读表统计和显式命中标志，并记录 hit/miss。
- `Put(id, table)`：持续重试内层可能拒绝的写入；成功后以原子 `fetch_max` 推进版本水位。
- `Version() -> u64`：以 Acquire 顺序读取缓存生命周期最大版本，而不是当前所有驻留条目的最大值；删除或淘汰不会回退它。
- `CopyAndUpdate(tables, deleted) -> StatsCache`：复制内层快照，在副本上先写后删，返回独立门面；用于非配额 Map 模式的 COW 更新。
- `Update(tables, deleted, skip)`：持锁完成批量写删；`skip == false` 时才推进版本水位。
- 私有 `count(label)`：把固定标签映射到预先初始化的 Prometheus counter；未知标签落到 update。
- `pub(crate) set_cost(cost)`：更新 `CostGauge`，只在指标句柄已经初始化时生效。

## 执行流程

1. `StatsCacheImpl::new`、`Clear` 等入口调用 `NewStatsCache` 或 `NewStatsCacheWithCapacity`。构造器根据 `quota` 选择 LFU/Map，并将版本初始化为 0。
2. 单项读取经 `StatsCacheImpl::Get -> StatsCache::Get -> StatsCacheInner::Get`。门面持锁取得 `Option<Arc<_>>` 后释放锁，根据结果增加 hit/miss，并同时返回 `bool`。
3. 单项写入经 `StatsCacheImpl::Put -> StatsCache::Put`。每次尝试先增加 update；若内层 `Put` 返回 `false`，记录警告、睡眠 5 ms 后无限重试。成功后用 `fetch_max` 合并表版本并返回。
4. `StatsCacheImpl::UpdateStatsCache` 根据配额模式分流：配额开启时在当前实例上调用 `Update`；关闭时调用 `CopyAndUpdate`，再由 `replace` 原子替换上层 `Arc<StatsCache>` 并关闭旧缓存。
5. `Update` 在一次互斥锁临界区内依次写入 `tables`、删除 `deleted`，为每项记录指标；离开临界区后，除非 `skip` 为真，再逐表推进版本。
6. `CopyAndUpdate` 先在锁内调用内层 `Copy`，然后在副本上应用写入和删除。新实例的初始版本取旧水位和所有新增表版本的最大值。
7. 容量调整、主动淘汰和异步屏障只是转发；真实 LFU 淘汰与异步维护位于 `internal/lfu/lfu_cache.rs`，Map 实现中的淘汰与等待是空操作。

## 数据与状态

`inner` 的键是物理表 ID（`i64`），值是 `Arc<StatisticsTable>`。`Arc` 让缓存快照和调用方共享表对象所有权；`internal/inner_test.rs::cache_contract_is_object_safe_and_keeps_shared_tables` 验证 `Get` 保留同一指针身份。调用方应把返回表视为只读；若产生新版本，应重新 `Put`，否则内存追踪与版本水位无法同步。

`maxTblStatsVer` 是生命周期水位而非派生值。`Put`、非跳过的 `Update`、`CopyAndUpdate` 可以使其增加；`Del`、驱逐、容量缩减不会使其降低。`StatsCacheImpl::GetNextCheckVersionWithOffset` 以该值计算下一轮 `mysql.stats_meta` 增量加载起点，因此水位不得倒退。

`CopyAndUpdate` 复制的是内层容器结构，而表值仍是 `Arc`。因此旧、新缓存可共享未变更表对象，但后续键集合变化彼此隔离；`statscache_test.rs::replace_shares_cache_and_map_copy_preserves_prior_snapshot` 覆盖了这一点。

## 依赖与调用关系

上游直接证据主要来自 `statscache.rs`：`StatsCacheImpl::new` 和 `Clear` 构造缓存；`Load` 返回当前 `Arc`；`Get`/`Put`/容量与淘汰方法转发到本文件；`UpdateStatsCache` 选择 `Update` 或 `CopyAndUpdate`；`replace` 调用旧实例 `Close` 并以 `set_cost(new.Cost())` 刷新指标。

下游边界由 `internal/inner.rs::StatsCacheInner` 定义。具体实现来自 Cargo 依赖 `cache-lfu` 和 `cache-map`；统计表类型由 `statistics` crate 通过本 crate 的 `StatisticsTable` 再导出提供；配置来自 `config` 与 `vardef`；指标句柄来自 `cache-metrics`；警告使用 `log`。

RustCodeGraph 对 `CopyAndUpdate` 给出的调用边包括 `StatsCacheInner::Copy`、`Put`、`Del` 和本文件 `Version`，并把本文件标记为被 `statscache.rs` 等统计模块使用。由于同名方法较多，精确上游位置以 `rg` 对 `pkg/statistics/handle/cache/*.rs` 的引用核验为准。

## 错误处理与边界

- 只有构造 LFU 会显式失败；错误文本被包装成 `CacheError`。`statscache_test.rs::lease_is_read_dynamically_and_tso_underflow_is_zero` 验证负容量构造返回错误。
- `Put` 对 `false` 没有超时、取消或最大次数，缓存持续拒绝时调用线程会永久每 5 ms 重试。它在每次失败时告警，并在每十次失败处额外告警。
- `Update` 与 `CopyAndUpdate` 忽略内层 `Put` 的布尔结果，不执行 `Put` 的重试逻辑；这与 Go 对照文件当前行为一致，但意味着批量路径依赖具体内层实现接受写入或自行处理拒绝。
- 所有 `Mutex::lock()` 都直接 `unwrap()`；若持锁线程 panic 导致锁中毒，后续调用也会 panic，而不是返回 `CacheError`。
- 指标尚未初始化时 `count` 与 `set_cost` 安静跳过；指标初始化次序由 `metrics/metrics.rs::init` 负责。`count` 的字符串分派只为当前固定调用点设计。
- 本文件不检查 `id == table.PhysicalID`，也未移植 Go `putCache` 中对 `ColAndIdxExistenceMap != nil` 的测试态断言；调用者必须维持键和值的物理 ID 一致性和表结构完整性。

## 并发与资源生命周期

`StatsCache` 可在线程间共享：trait 要求内层实现 `Send`，而 `Mutex<Box<dyn StatsCacheInner>>` 为访问提供互斥。与 Go 直接调用并发安全的内层对象不同，Rust 门面把读取、写入、容量调整、关闭及等待都串行化；长时间的 `Close` 或 `WaitForAsyncUpdates` 会阻塞同一实例上的其他操作。

版本水位独立于内层锁：写成功后用 `Ordering::AcqRel` 的 `fetch_max` 发布，读取用 `Ordering::Acquire`。这保证并发写不会让较小版本覆盖较大版本。批量 `Update` 先完成内层变更并释放互斥锁，再更新原子版本，所以并发观察者可能短暂看到新表已经可读而版本水位尚未前移。

`StatsCacheImpl` 通过 `RwLock<Arc<StatsCache>>` 管理实例替换。Map/COW 路径产生新实例、交换 `Arc`、关闭旧实例；已克隆旧 `Arc` 的操作仍持有旧门面。LFU 的异步维护需要调用 `WaitForAsyncUpdates` 作为可见性/淘汰屏障，Map 对应方法为空。`statscache_test.rs::quota_eviction_retains_table_metadata_like_go_lfu` 验证容量缩减、主动淘汰和等待后的可见行为。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `statscacheinner.go` 的 `StatsCache`：构造时选择 LFU/Map，公开 Len/Get/Put/Values/Cost/SetCapacity/Close/Version，提供 COW 与原地更新，并转发淘汰/等待方法。`Arc<StatisticsTable>` 对应 Go 的 `*statistics.Table`，`Box<dyn StatsCacheInner>` 对应 Go interface，`AtomicU64` 对应 `atomic.Uint64`。

已确认的差异如下：

- Rust 增加 `NewStatsCacheWithCapacity`，把策略与容量显式化，供测试和 `StatsCacheImpl` 的保存设置使用；Go 默认构造器直接读取全局值。
- Rust 用外层 `Mutex` 保护整个 trait 对象；Go 不在该门面加锁，由具体实现承担并发语义。
- Rust `fetch_max` 能在竞争中稳定保留最大版本；Go `Put` 使用 CAS 循环，Go `Update` 只做一次 CompareAndSwap。最终目标都是让水位单调前进，但竞争细节不完全相同。
- Go `putCache` 含 `intest.Assert(ColAndIdxExistenceMap != nil)`；Rust 当前没有对应断言。
- Rust 指标句柄是 `Option`，未初始化时跳过；Go 直接调用已绑定的包级指标。
- Rust `CopyAndUpdate` 在循环中直接计算版本最大值；Go 先复制和修改，再单独遍历新增表推进版本，结果等价。

## 扩展指南

新增缓存后端时，应先在独立的 `internal` 子 crate 实现 `StatsCacheInner` 全部方法，再在 `NewStatsCacheWithCapacity` 增加明确的策略选择；不要把后端细节泄露给 `StatsCacheImpl`。需要同时补充独立测试文件，至少覆盖构造失败、Put 拒绝策略、Copy 键集合隔离、Close 幂等性、容量/淘汰及异步可见性。

修改版本语义时，应同步检查 `Put`、`Update(skip)`、`CopyAndUpdate` 和 `Version`，并验证 `statscache.rs::GetNextCheckVersionWithOffset` 的增量读取行为。尤其不能因删除或淘汰而降低水位，否则可能重复或漏读统计元数据。

修改并发策略时，要评估外层 `Mutex` 的临界区长度、锁中毒处理，以及 `Update` 中“数据先可见、版本后发布”的窗口。若让 `Update` 或 `CopyAndUpdate` 处理 `Put == false`，必须明确批量操作的重试、部分成功、取消和指标语义，不能直接复用无限重试而引入无法终止的批次。

测试应继续放在独立文件：门面和上层行为扩展 `statscache_test.rs`，trait 契约扩展 `internal/inner_test.rs`，LFU/Map 特性分别扩展各自的 `*_test.rs`；性能变化使用 `bench_test.rs` 的 Put/Get/CopyAndUpdate 基准。还应对照更新 `statscacheinner.go` 及其测试意图，避免 Rust 行为无依据偏离 Go。

## 验证依据

- 源文件与符号：`statscacheinner.rs` 的 `StatsCache`、两个构造器、全部 impl 方法、`count`、`set_cost`；RustCodeGraph `node --file ... --offset 1 --limit 260` 显示文件完整 144 行。
- 图查询：RustCodeGraph `status` 显示索引含该文件；`query StatsCacheInner`、`query CopyAndUpdate`、`callees CopyAndUpdate` 核对 trait、Go/Rust 对照符号及 Copy/Put/Del/Version 调用边。泛型同名方法的 callers 结果不足以唯一定位时，以源码引用搜索补证。
- crate 与装配：`pkg/statistics/handle/cache/Cargo.toml`、`pkg/statistics/handle/cache/lib.rs`、`pkg/statistics/handle/cache/internal/inner.rs`、两种后端的 Cargo 依赖。
- 上游流程：`pkg/statistics/handle/cache/statscache.rs` 中 `StatsCacheImpl::new`、`replace`、`UpdateStatsCache`、`Get`、`Put`、`Clear`、容量和淘汰转发。
- Go 对照：`pkg/statistics/handle/cache/statscacheinner.go`、`pkg/statistics/handle/cache/internal/inner.go`。
- 测试证据：`pkg/statistics/handle/cache/statscache_test.rs` 的 `quota_eviction_retains_table_metadata_like_go_lfu`、`replace_shares_cache_and_map_copy_preserves_prior_snapshot`、`lease_is_read_dynamically_and_tso_underflow_is_zero`；`pkg/statistics/handle/cache/internal/inner_test.rs::cache_contract_is_object_safe_and_keeps_shared_tables`；性能入口位于 `bench_test.rs`。Go 的 `statscache_test.go` 主要提供 mock 接口覆盖，未发现直接覆盖本门面全部方法的同名独立单元测试。
- 本任务为纯文档分析，按计划未运行 Cargo；最终以固定 11 章节结构检查和人工事实复核验收。
