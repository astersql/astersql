# `pkg/statistics/handle/cache/internal/inner.rs`

## 文件定位

本文件位于统计信息缓存的内部边界 crate `astersql-statistics-handle-cache-internal` 中。`pkg/statistics/handle/cache/internal/lib.rs` 将私有模块 `inner` 的内容公开再导出，因此上层以 `cache_internal::StatsCacheInner` 使用这里唯一的生产符号 `StatsCacheInner`。

`pkg/statistics/handle/cache/internal/Cargo.toml` 表明该 crate 只直接依赖 `astersql-statistics`，本文件因而只约束缓存容器如何持有 `statistics::Table`，不负责缓存算法、指标、配置或上层统计版本管理。具体实现位于相邻的 `internal/lfu/lfu_cache.rs` 与 `internal/mapcache/map_cache.rs`，统一使用者位于 `pkg/statistics/handle/cache/statscacheinner.rs`。

## 核心职责

`StatsCacheInner: Send` 是表统计缓存的可替换存储协议，按物理表 ID 提供查询、写入、删除、遍历、计数和内存成本读取，并统一暴露复制、容量调整、关闭以及异步淘汰同步操作。它让 `StatsCache` 无需知道当前使用 LFU 还是无淘汰的 MapCache；`NewStatsCacheWithCapacity` 根据 `quota` 构造 `Box<dyn StatsCacheInner>`。

本文件只声明协议，没有默认实现、字段、缓存算法或调度逻辑。命中指标和最大表统计版本由外层 `StatsCache` 维护；具体内存计量、淘汰和后台维护语义由实现承担。

## 主要符号

- `pub trait StatsCacheInner: Send`：对象安全的动态分派边界，可作为 `Box<dyn StatsCacheInner>` 跨线程所有权边界传递；它没有要求实现本身为 `Sync`。
- `Get(&self, tid: i64) -> Option<Arc<Table>>`：以物理表 ID 查询。`None` 表示未命中，命中时克隆并返回共享的表对象句柄。
- `Put(&mut self, tid: i64, table: Arc<Table>) -> bool`：插入或替换表统计；布尔值表示实现是否接受本次写入。外层 `StatsCache::Put` 在返回 `false` 时会等待 5 ms 后持续重试。
- `Del(&mut self, tid: i64)`、`Values(&self) -> Vec<Arc<Table>>`、`Len(&self) -> usize`：分别删除一项、取得当前值快照和取得条目数。接口不承诺 `Values` 的顺序。
- `Cost(&self) -> i64`：返回实现所跟踪的内存代价；具体采用 `TotalMemUsage` 还是 `TotalTrackingMemUsage` 由实现定义。
- `Copy(&self) -> Box<dyn StatsCacheInner>`：产生另一个 trait 对象。签名未表达深拷贝约束；MapCache 复制映射容器但共享各 `Arc<Table>`，LFU 的 `clone` 与 Go 的 `return s` 一样共享底层状态。
- `SetCapacity(&mut self, capacity: i64)`：调整容量。LFU 会据此重建/淘汰，MapCache 将它作为空操作。
- `Close(&mut self)`、`TriggerEvict(&mut self)`、`WaitForAsyncUpdates(&mut self)`：生命周期和异步维护钩子。LFU 提供实际行为，MapCache 均为空操作。

文件没有模块级常量、结构体、枚举、条件编译项或私有辅助函数。

## 执行流程

1. `pkg/statistics/handle/cache/statscacheinner.rs::NewStatsCacheWithCapacity` 根据是否启用统计缓存内存配额，构造 `LFU` 或 `MapCache`，装入 `Mutex<Box<dyn StatsCacheInner>>`。
2. 外层 `StatsCache::{Get,Put,Values,Cost,SetCapacity,Close}` 获取互斥锁后调用同名 trait 方法；`Get` 额外记录 hit/miss，`Put` 成功后推进 `maxTblStatsVer`。
3. 批量原地更新通过 `StatsCache::Update` 在一次持锁期间逐项调用 `Put`/`Del`。COW 路径 `CopyAndUpdate` 先调用 `Copy`，再向返回对象写入新增表并删除指定 ID。
4. 更外层 `StatsCacheImpl`（`pkg/statistics/handle/cache/statscache.rs`）通过可替换的 `Arc<StatsCache>` 暴露这些行为给统计加载与查询链，并转发 `TriggerEvict`、`WaitForAsyncUpdates` 和容量更新。
5. 对 LFU，写入和淘汰可能经过缓存库的待处理任务；调用者在后续读必须观察写入时调用 `WaitForAsyncUpdates`。MapCache 同步修改 `HashMap`，因此该方法为空操作。

## 数据与状态

接口的键固定为 `i64` 物理表 ID，值固定为 `Arc<statistics::Table>`。`Arc` 使缓存、调用者以及复制后的容器共享不可变句柄；trait 本身不定义表内容的复制策略，也不拥有版本水位，版本由外层 `StatsCache::maxTblStatsVer` 单独维护。

`Cost`、`Len` 和 `Values` 是实现状态的三个观察面。MapCache 维护 `HashMap<i64, CacheItem>` 与累计 `memUsage`，替换时按新旧 cost 差更新；LFU 维护主缓存、结果键集合、频率、原子 cost/容量和关闭状态。因此不能从 `Len` 推导 `Cost`，也不能假定不同实现的成本口径完全相同。

`Copy` 尤其需要按实现理解：MapCache 新建映射并复制记账值，后续增删与原容器隔离；LFU 返回共享内部状态的克隆，后续操作可影响同一缓存状态。这与对应 Go 实现的差异化语义一致，接口注释中的“深拷贝”不能作为跨实现保证。

## 依赖与调用关系

- crate 边界：`internal/Cargo.toml` 将 `statistics` 映射到 `pkg/statistics`；`internal/lib.rs` 再导出本 trait。
- 实现方：`internal/mapcache/map_cache.rs::impl StatsCacheInner for MapCache` 和 `internal/lfu/lfu_cache.rs::impl StatsCacheInner for LFU`。两个实现 crate 的 Cargo 清单都以路径依赖引用本 internal crate。
- 直接使用方：`statscacheinner.rs::StatsCache` 的 `inner: Mutex<Box<dyn StatsCacheInner>>`，以及 `NewStatsCacheWithCapacity`、`Get`、`Put`、`CopyAndUpdate`、`Update`、`SetCapacity`、`Close`、`TriggerEvict`、`WaitForAsyncUpdates`。
- 上层门面：`statscache.rs::StatsCacheImpl` 转发查询、写入、成本、容量和异步同步操作，使接口进入统计信息加载、更新和查询路径。
- 测试使用方：`inner_test.rs::TestCache` 提供最小实现；MapCache/LFU 各自的独立测试验证具体算法。RustCodeGraph 能定位 trait、实现文件和门面文件，但未为 trait 方法的动态分派生成 callers/callees 边，因此调用关系由上述字段类型和调用点直接核验。

## 错误处理与边界

trait 不返回 `Result`，也没有统一错误类型。查询失败以 `Option::None` 表示；删除缺失键没有反馈；关闭、容量调整和等待操作也不报告错误。`Put == false` 的处理策略留给调用方，目前外层选择无限重试并记录警告，因此新实现必须避免永久拒绝而无恢复路径。

接口没有规定负容量、成本溢出、关闭后调用、重复关闭、重复删除或锁中毒的统一行为。现有 LFU 会在容量调整失败时直接返回，锁中毒处使用 `expect`；MapCache 忽略容量和生命周期钩子。新增实现必须明确这些边界，并保持外层“不接收错误”的既有约束，或在改变签名前同步修改全部实现与门面。

`Values` 返回快照集合但不保证顺序；`Copy` 不保证隔离层级；`WaitForAsyncUpdates` 只承诺由具体实现定义的待处理更新已排空，不能泛化为整个统计系统的事务屏障。

## 并发与资源生命周期

`Send` 允许 trait 对象随所有权在线程间移动；未声明 `Sync`，而可变操作要求 `&mut self`。生产门面以 `Mutex<Box<dyn StatsCacheInner>>` 串行化所有 trait 调用，使具体实现无需通过本接口直接暴露共享可变性。返回的表以 `Arc<Table>` 管理共享生命周期，删除缓存项不会强制销毁仍被调用者持有的表。

LFU 内部还使用读写锁、互斥锁和原子量，并有异步维护任务：`WaitForAsyncUpdates` 排空待处理任务，`TriggerEvict` 在成本超限时等待淘汰，`Close` 设置关闭标志并清理主缓存、结果集合和频率表。MapCache 没有后台资源，三个生命周期方法为空操作。调用方不应因为方法存在就假定所有实现都有线程或异步队列。

外层在 `StatsCache::Put` 重试时每次只在单次 `Put` 调用期间持锁，睡眠发生在锁守卫释放后；批量 `Update` 则在整个增删循环期间持锁。扩展实现时应避免在这些方法中加入无界阻塞，以免放大统计缓存的全局串行区。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/cache/internal/inner.go`。Rust trait 保留 Go `StatsCacheInner` 的 11 个方法及总体职责：`Get`、`Put`、`Del`、`Cost`、`Values`、`Len`、`Copy`、`SetCapacity`、`Close`、`TriggerEvict`、`WaitForAsyncUpdates`。

类型映射为：Go `*statistics.Table` 对应 Rust `Arc<Table>`；Go `Get(...)(*Table, bool)` 对应 Rust `Option<Arc<Table>>`；Go `int` 长度对应 Rust `usize`；Go 接口返回值对应 `Box<dyn StatsCacheInner>`。Rust 通过 `&self`/`&mut self` 显式区分只读与可变操作，并增加 `Send` 上界。

现有实现语义也有直接对应：Go MapCache 的 `Copy` 新建 map，Rust MapCache 新建 `HashMap`；Go LFU 的 `Copy` 直接返回 `s`，Rust LFU 通过 `clone` 共享其 `Arc` 状态；两边 MapCache 的容量、关闭、淘汰和等待钩子都是空操作。Go 接口注释保留了将缓存泛型化、不再直接感知 `statistics.Table` 的 TODO，Rust 当前尚未实现该泛化，扩展时不能把 TODO 当成现状。

## 扩展指南

新增缓存算法时，应在独立实现文件中实现全部 11 个方法，并由构造路径显式选择它；不要把算法或测试实现写进本 trait 文件。至少需要说明 `Put` 何时返回 `false`、成本口径、`Values` 的快照语义、`Copy` 是容器隔离还是共享状态、容量为零/负数时的行为、关闭幂等性，以及何时必须调用 `WaitForAsyncUpdates`。

修改接口签名时必须同步：`statscacheinner.rs` 的动态对象与转发、LFU/MapCache 两个实现、`internal/inner_test.rs::TestCache`，以及实现各自的独立测试。涉及异步准入或淘汰时，应在 `internal/lfu/lfu_cache_test.rs` 覆盖写后等待、容量收缩、淘汰和关闭；涉及同步映射与成本记账时，应在 `internal/mapcache/map_cache_test.rs` 覆盖替换/删除/复制；接口对象安全与共享句柄继续放在独立的 `internal/inner_test.rs`，不要嵌入生产源文件。

兼容性风险集中在动态对象安全、Go/Rust 行为偏离和 `Copy` 语义；性能风险集中在外层互斥锁内的阻塞、`Values` 全量分配、表级 `Arc` 克隆以及异步维护等待。若要将值泛型化，应作为跨 crate 的接口迁移处理，而不是只修改本文件。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`inner.rs` 被索引为 49 行、15 个符号，其中 `StatsCacheInner` 及其 11 个方法均可精确查询。
- RustCodeGraph 已读源码：`internal/inner.rs`、`internal/mapcache/map_cache.rs`、`internal/lfu/lfu_cache.rs`、`statscacheinner.rs`、`statscache.rs`。trait 方法的 `callers` 查询为空，动态分派关系改由 `Box<dyn StatsCacheInner>` 字段、两个 `impl` 和门面调用点交叉核验。
- crate/模块证据：`pkg/statistics/handle/cache/internal/Cargo.toml`、`internal/lib.rs`、`internal/lfu/Cargo.toml`、`internal/mapcache/Cargo.toml`、`pkg/statistics/handle/cache/Cargo.toml`。
- Go 对照：`internal/inner.go`、`internal/lfu/lfu_cache.go`、`internal/mapcache/map_cache.go`、`statscacheinner.go`。
- 独立 Rust 测试：`internal/inner_test.rs::cache_contract_is_object_safe_and_keeps_shared_tables` 验证 trait 对象、Put/Get 的同一 `Arc` 与 Copy 后长度；`internal/lfu/lfu_cache_test.rs` 覆盖异步等待、容量与复制；`internal/mapcache/map_cache_test.rs` 覆盖映射实现；`statscache_test.rs::quota_eviction_retains_table_metadata_like_go_lfu` 覆盖门面层触发淘汰并等待后的可见行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅运行任务指定的 11 章节结构检查，并人工复核本文没有把实现差异提升为 trait 保证。
