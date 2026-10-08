# `pkg/statistics/handle/cache/internal/mapcache/map_cache.rs`

## 文件定位

本说明对应真实源文件 [`map_cache.rs`](./map_cache.rs)。该文件实现统计信息缓存的无淘汰后端 `MapCache`，位于独立 crate `astersql-statistics-handle-cache-internal-mapcache` 中；该 crate 的入口 [`lib.rs`](./lib.rs) 通过 `pub use map_cache::*` 导出本文件的公开项。[`Cargo.toml`](./Cargo.toml) 只直接依赖内部缓存抽象 crate `cache-internal` 和统计信息 crate `statistics`，分别提供 `StatsCacheInner` trait 与 `Table` 类型。

在完整缓存链路中，`pkg/statistics/handle/cache/statscacheinner.rs::NewStatsCacheWithCapacity` 根据 `quota` 选择后端：启用内存配额时构造 LFU，关闭配额时调用本文件的 `NewMapCache`。之后上层 `StatsCache` 只通过 `Box<dyn StatsCacheInner>` 使用该实现，因此本文件是“保留所有表统计、不按容量淘汰”的具体策略，而不是对外缓存门面。

## 核心职责

- 用 `HashMap<i64, CacheItem>` 保存“物理表 ID → 表统计”映射；键和值的对应关系见 `MapCache::tables`、`StatsCacheInner::Get` 和 `Put`。
- 在写入时读取 `Table::MemoryUsage().TotalMemUsage`，将成本快照保存在 `CacheItem::cost`，并通过 `MapCache::memUsage` 维护所有当前条目的累计成本。
- 实现 `StatsCacheInner` 要求的查询、更新、删除、枚举、长度、复制及生命周期方法，使上层可以在 map 与 LFU 后端之间切换。
- 明确提供无淘汰语义：`SetCapacity`、`TriggerEvict`、`WaitForAsyncUpdates` 和 `Close` 均为空操作；容量、后台准入、异步淘汰和资源关闭不属于本实现的职责。

## 主要符号

- `CacheItem { value: Arc<Table>, cost: i64 }`：私有缓存条目。`value` 共享统计表对象，`cost` 固化该对象写入时报告的内存成本。它派生 `Clone`，供 `MapCache::Copy` 复制映射。
- `MapCache { tables, memUsage }`：公开缓存类型，派生 `Default`。字段均为私有；`tables` 保存条目，`memUsage` 保存累计成本。
- `NewMapCache() -> MapCache`：公开构造函数，返回默认的空映射与零成本缓存。名称沿用 Go API；crate 根的 `#![allow(non_snake_case)]` 允许该命名。
- `MapCache::Keys(&self) -> Vec<i64>`：公开固有方法，复制并返回全部键。由于底层是 `HashMap`，返回顺序没有稳定保证。
- `StatsCacheInner::Get`：命中时克隆条目中的 `Arc<Table>`，未命中返回 `None`。
- `StatsCacheInner::Put`：计算新值成本，插入或替换条目，按“新成本减旧成本”修正累计值，并始终返回 `true`。
- `StatsCacheInner::Del`：仅在键存在时删除，并减去条目保存的成本；不存在的键是无操作。
- `Cost`、`Values`、`Len`：分别暴露成本、共享值快照和条目数。`Values` 与 `Keys` 一样不承诺顺序。
- `Copy`：克隆 `HashMap`、`CacheItem` 和 `Arc`，并复制 `memUsage`，返回新的 `Box<dyn StatsCacheInner>`。
- `SetCapacity`、`Close`、`TriggerEvict`、`WaitForAsyncUpdates`：为满足统一 trait 而实现的同步空操作。

## 执行流程

构造路径从 `NewStatsCacheWithCapacity(false, capacity)` 开始：上层忽略 map 后端不使用的 `capacity`，调用 `NewMapCache`，再将结果装入 `StatsCache.inner` 的 `Mutex<Box<dyn StatsCacheInner>>`。

写入路径为：`StatsCache::Put` 或 `StatsCache::Update` 取得外层互斥锁，调用 `MapCache::Put`；后者先从 `value.MemoryUsage().TotalMemUsage` 取得新成本，再执行 `HashMap::insert`。若键已存在，插入返回旧 `CacheItem`，`memUsage` 增加 `cost - previous.cost`；若键不存在，则增加完整 `cost`。函数返回 `true`，所以上层 `StatsCache::Put` 的失败重试循环在 map 后端第一次调用即结束。

读取路径中，`StatsCache::Get` 在互斥锁内调用 `MapCache::Get`；命中时只增加 `Arc` 引用计数，不复制 `Table`。`Values` 同样为每个条目克隆 `Arc`。`Keys` 直接复制整数键。

删除路径中，`HashMap::remove` 同时取得被删条目；存在时从累计成本减去该条目的成本，不存在时保持状态不变。复制路径中，`StatsCache::CopyAndUpdate` 先调用 `MapCache::Copy` 得到独立映射，再在副本上执行 `Put`/`Del`，因此副本的键集合和成本记账可独立变化。

## 数据与状态

核心不变量是：只要条目变更都通过本文件的 `Put` 和 `Del`，`memUsage` 等于 `tables` 中所有 `CacheItem::cost` 的和。替换已有键不会改变 `Len`，但会用成本差值更新 `memUsage`；删除不存在的键不会改变长度或成本。

`CacheItem::cost` 是写入时的成本快照。文件不会在 `Get`、`Values` 或 `Cost` 时重新计算 `Table` 的内存占用，因此若共享 `Table` 的内部状态可在写入后改变，本缓存的成本不会自动刷新；要更新记账，需要再次 `Put` 对应表。

`Copy` 的“独立”限于缓存容器和成本字段：新旧 `HashMap` 可分别增删条目，但条目中的 `Arc<Table>` 指向同一统计表对象。这与直接深拷贝全部统计数据不同，也使复制成本主要来自映射与引用计数操作。

## 依赖与调用关系

上游直接调用边由 RustCodeGraph 确认：`pkg/statistics/handle/cache/statscacheinner.rs::NewStatsCacheWithCapacity` 在 `quota == false` 分支调用 `cache_map::NewMapCache`。同一上层文件的 `StatsCache::{Get, Put, Values, Cost, SetCapacity, CopyAndUpdate, Update, TriggerEvict, WaitForAsyncUpdates, Close}` 经 `StatsCacheInner` trait 对象间接进入本实现。

下游依赖包括：

- `cache_internal::StatsCacheInner`：定义本文件必须实现的方法集合，并要求实现类型满足 `Send`。
- `statistics::Table`：缓存的值类型；`Put` 调用其 `MemoryUsage` 获取 `TotalMemUsage`。
- `std::collections::HashMap`：负责键值存储、插入替换、删除和无序遍历。
- `std::sync::Arc`：让缓存、返回值及缓存副本共享 `Table` 的所有权。

`pkg/statistics/handle/cache/internal/mapcache/lib.rs` 是本 crate 的导出边界；它还通过 `#[cfg(test)]` 将独立文件 `map_cache_test.rs` 接入测试构建，测试逻辑没有内嵌在生产源文件中。

## 错误处理与边界

本文件没有自定义错误类型，也没有返回 `Result`。`Get` 用 `Option` 表示未命中；`Del` 对不存在的键静默无操作；`Put` 当前没有拒绝路径，恒定返回 `true`。因此上层针对可能拒绝写入的通用重试逻辑不会由 map 后端触发。

容量参数对本实现没有效果，调用 `SetCapacity` 不会限制内存，也不会删除条目。`TriggerEvict` 不触发淘汰，`WaitForAsyncUpdates` 无需等待，`Close` 不释放额外资源。调用者不能把统一 trait 中这些方法的存在解释为 map 后端支持容量治理或后台任务。

本文件不验证键是否等于 `Table` 内部的物理 ID，也不排序 `Keys`/`Values`，还不处理条目成本在写入后的变化；这些都是调用方或更高层契约需要维护的边界。

## 并发与资源生命周期

`MapCache` 自身不包含锁，所有方法都假定调用者提供适当的串行化：写方法要求 `&mut self`，读方法要求 `&self`。实际接线中，`StatsCache.inner` 用 `Mutex<Box<dyn StatsCacheInner>>` 包装后端，所有公开缓存操作先加锁，因此同一个已接线实例上的 `HashMap` 和 `memUsage` 更新是串行的。`StatsCacheInner: Send` 允许该对象被放进这一跨线程共享结构，但本文件没有独立的后台线程、通道或异步任务。

`Arc<Table>` 管理统计表生命周期：从缓存删除条目只释放缓存持有的一个强引用；调用者或副本仍持有引用时，`Table` 继续存活。`Copy` 增加强引用计数，随后两个缓存容器独立销毁。`Close` 是空操作，因为本实现没有需要停止或回收的后台资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/cache/internal/mapcache/map_cache.go`。两版都以表 ID 为键、保存表指针/共享引用与写入成本、在替换时按新旧成本差更新总成本、删除时扣减成本，并将容量及异步生命周期方法实现为空操作。

Rust `Arc<Table>` 对应 Go `*statistics.Table` 的共享对象语义。Go `cacheItem` 额外保存 `key`，但读取、删除和枚举都以 map 键为准；Rust 删除了这个未参与行为的冗余字段。Go `cacheItem.copy` 逐字段复制，Rust 用派生的 `Clone` 完成等价容器复制；两者都共享底层表对象，不深拷贝表统计。

接口形态存在语言差异：Go `Get` 返回 `(*Table, bool)`，Rust 返回 `Option<Arc<Table>>`；Go `NewMapCache` 返回指针，Rust 返回拥有值后再由上层装箱；Go `Len` 返回 `int`，Rust 返回 `usize`。这些差异不改变命中、替换、删除、复制和无淘汰语义。

同目录没有 `map_cache_test.go`。当前直接回归证据来自 Rust 独立测试 `map_cache_test.rs::put_replace_copy_and_delete_match_the_go_map_cache`，它以 Go 行为为契约，覆盖同键替换、长度不增长、返回替换后的同一 `Arc`、副本长度、删除后未命中以及成本归零。

## 扩展指南

若新增会改变条目或成本的行为，应优先修改 `Put`、`Del` 与 `Copy`，并始终维护“`memUsage` 等于条目成本之和”的不变量。若改变成本计算时机，需要明确处理共享 `Table` 在写入后的变化，并在独立测试文件中加入替换前后成本不同、删除不存在键及复制后分别修改的用例。

若要引入容量限制、淘汰或异步更新，不应只填写当前空方法：还需设计淘汰策略、同步方式、后台资源关闭协议以及 `Put` 返回 `false` 时与 `StatsCache::Put` 重试循环的交互。此类变化应与 LFU 实现和 Go `StatsCacheInner` 契约共同核对，避免让相同 trait 方法在两个后端产生意外不兼容语义。

若扩展公开 API，需要同步检查 `lib.rs` 的导出、`Cargo.toml` 的最小依赖，以及上层 `statscacheinner.rs` 的 trait-object 接线。测试应继续放在 `pkg/statistics/handle/cache/internal/mapcache/map_cache_test.rs`，不要嵌入生产文件；涉及 Go 对齐时还应检查 `map_cache.go` 和上层 `statscacheinner.go`。

性能方面，`Get` 是平均常数时间并只克隆 `Arc`；`Keys`、`Values` 和 `Copy` 都遍历全部条目，其中 `Copy` 还分配新的映射。扩展时不应在热路径上无意增加全表扫描、深拷贝或持锁时间。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 11,467 个文件，目标目录的 `map_cache.rs`、`lib.rs`、`map_cache_test.rs` 及 Go 对照文件均已索引；`map_cache.rs` 识别出 16 个符号。
- RustCodeGraph `node --file pkg/statistics/handle/cache/internal/mapcache/map_cache.rs`：核对 `CacheItem`、`MapCache`、构造函数、全部 trait 方法及其完整实现。
- RustCodeGraph `explore "NewMapCache NewStatsCacheWithCapacity map_cache.rs stats cache"`：确认直接调用边 `NewStatsCacheWithCapacity → NewMapCache`。
- RustCodeGraph 对 `lib.rs`、`internal/inner.rs`、`statscacheinner.rs` 和 `map_cache_test.rs` 的文件节点：核对 crate 导出、trait 边界、外层互斥与调用方式，以及独立测试覆盖。
- `pkg/statistics/handle/cache/internal/mapcache/Cargo.toml`：核对 crate 名、`lib.rs` 入口、两个路径依赖和 Go 包迁移元数据。
- `pkg/statistics/handle/cache/internal/mapcache/map_cache.go`、`pkg/statistics/handle/cache/internal/inner.go`、`pkg/statistics/handle/cache/statscacheinner.go`：核对 Go 实现、接口契约与上层选择路径。仓库搜索确认同目录没有 Go 专属测试文件。
- 人工复核结论：本文件存在的理由是为关闭统计缓存内存配额的场景提供简单、同步、无淘汰后端；安全扩展的关键是保持成本记账、容器复制与外层锁契约一致，并在独立测试文件覆盖新边界。
