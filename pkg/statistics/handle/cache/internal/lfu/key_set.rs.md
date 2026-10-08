# `pkg/statistics/handle/cache/internal/lfu/key_set.rs`

## 文件定位

本文件实现 LFU 统计缓存内部的单分片二级键集合 `keySet`。它属于 Cargo crate `astersql-statistics-handle-cache-internal-lfu`（见同目录 `Cargo.toml`），由 `lib.rs` 以私有模块 `mod key_set` 装配，不是 crate 的公共 API。直接上游是 `key_set_shard.rs` 的 `keySetShard`：后者建立 256 个 `keySet`，再供 `lfu_cache.rs` 的 `LFUState::resultKeySet` 使用。

这里保存的不是 TinyLFU/Moka 主缓存本身，而是物理表 ID 到 `Arc<statistics::Table>` 的并发映射。`LFU::Put` 在主缓存准入前先向它发布表；主缓存拒绝或淘汰完整统计数据后，它仍可保存精简后的“壳表”。因此 `LFU::Get` 可以在主缓存未命中时回落到该集合，`LFU::Values` 和 `LFU::Len` 也以该集合为准。

## 核心职责

- 以 `i64` 物理表 ID 为键，持有共享所有权的 `Arc<Table>`。
- 用一把 `RwLock` 串行化同一分片内的修改，并允许互不冲突的读取并发进行。
- 提供插入/覆盖、查询、删除、键快照、长度和整体清空六种基础操作，供分片层统一转发。
- 删除时返回被删表的 `MemoryUsage().TotalTrackingMemUsage()`，让需要成本结算的上层可以使用；当前 `keySetShard::Remove` 与 `LFU::Del` 会忽略该返回值，实际成本调整由 LFU 的准入/淘汰路径负责。

该文件不负责分片路由、TinyLFU 准入、频率计数、淘汰壳表生成、容量控制或指标更新；这些职责分别位于 `key_set_shard.rs` 和 `lfu_cache.rs`。

## 主要符号

- `pub(crate) struct keySet`：crate 内可见的容器，只有字段 `set: RwLock<HashMap<i64, Arc<Table>>>`。`#[derive(Default)]` 创建空 `HashMap` 和未加锁的 `RwLock`。
- `Remove(&self, key: i64) -> i64`：取得写锁并删除键；命中时读取被删表的跟踪内存，未命中返回 `0`。
- `Keys(&self) -> Vec<i64>`：在读锁内复制当前全部键。结果是快照，顺序沿用 `HashMap` 的未定义迭代顺序。
- `Len(&self) -> usize`：在读锁内读取当前条目数。
- `AddKeyValue(&self, key: i64, value: Arc<Table>)`：在写锁内插入；已有键会被覆盖，并按 `Arc` 引用计数规则释放集合持有的旧引用。
- `Get(&self, key: i64) -> Option<Arc<Table>>`：在读锁内查找并克隆 `Arc`；锁释放后返回值仍拥有表的有效共享引用。
- `Clear(&self)`：在写锁内以新的空 `HashMap` 替换旧映射，原映射及其中的 `Arc` 在替换完成后释放。

文件没有模块级常量、trait、条件编译项或公开到 crate 外的符号。

## 执行流程

1. 构造阶段：`keySetShard::newKeySetShard` 用 `std::array::from_fn` 调用 `keySet::default()`，为每个分片创建独立映射和读写锁。
2. 写入阶段：`LFU::Put` 先克隆传入的 `Arc<Table>`，经 `keySetShard::AddKeyValue` 按表 ID 路由，最终由本文件的 `AddKeyValue` 在写锁内发布或覆盖。这样即使后续主缓存准入拒绝，读取仍可命中二级集合。
3. 读取阶段：`LFU::Get` 先查询 Moka 主缓存，未命中时经分片层调用本文件的 `Get`；`Arc::clone` 把对象生命周期从锁保护范围中解耦。
4. 淘汰阶段：`LFUState::retainEvictedShell` 生成丢弃重统计数据的表副本，再用 `AddKeyValue` 覆盖二级集合中的完整表。键继续存在，但值可能从完整统计表变为壳表。
5. 枚举阶段：`LFU::Values` 先从所有分片收集 `Keys`，再逐键 `Get`。两步之间允许并发删除或清空，因此 `filter_map` 会跳过已经消失的键；该接口提供弱一致枚举而非事务快照。
6. 删除和清理阶段：`LFU::Del` 经分片层调用 `Remove`；`LFU::Clear` 与 `LFU::Close` 经分片层逐个调用 `Clear`。单个 `keySet::Clear` 是锁内整体替换，但 256 个分片之间不存在全局原子切换。

## 数据与状态

唯一可变状态是 `HashMap<i64, Arc<Table>>`，其所有访问均经过 `RwLock`。键与上层 `LFU`、`keySetShard` 一致，语义上是物理表 ID。值使用 `Arc`，所以集合删除、覆盖或清空只会释放集合持有的那一份强引用；仍由调用方或主缓存持有的表不会提前销毁。

`keySet` 不保存容量、频率或累计成本。`Remove` 计算的是删除瞬间该表的 `TotalTrackingMemUsage()`，不是集合自身开销，也不会修改 `LFUState::cost`。`Keys` 返回独立 `Vec<i64>`，调用方不能借此修改内部映射。`Default` 保证 Rust 映射从一开始就是可写的空映射（底层容量仍可延迟分配）；不存在 Go 零值 `nil map` 的写入限制。

## 依赖与调用关系

- 标准库：`HashMap` 提供键值存储，`RwLock` 提供同步，`Arc` 提供跨锁、跨线程的共享所有权。
- 工作区依赖：`statistics::Table` 来自同一工作区的 `astersql-statistics` crate；`Cargo.toml` 以路径 `../../../../` 声明该依赖。
- 直接上游：`key_set_shard.rs` 导入 `super::key_set::keySet`，构造 256 个实例，并逐一转发 `Get`、`AddKeyValue`、`Remove`、`Keys`、`Len`、`Clear`。
- 应用上游：`lfu_cache.rs` 在 `LFUState::retainEvictedShell`、`LFU::Put`、`LFU::Get`、`LFU::Del`、`LFU::Values`、`LFU::Len`、`LFU::Clear` 和 `LFU::Close` 中使用分片集合。
- crate 边界：`lib.rs` 不重新导出 `keySet`；对外只 `pub use lfu_cache::*`，因此外部调用者通过 `LFU`/`StatsCacheInner` 接口间接触发本文件逻辑。

RustCodeGraph 的精确节点查询确认了本文件六个方法的定义，并确认 `key_set_shard.rs` 是 `keySet` 的导入方；对 `key_set_shard.rs::Get` 的调用轨迹还连接到并发/淘汰相关 Rust 测试。由于方法名 `Get`、`Remove`、`Keys` 等高度重复，图的通用 callers/callees 查询存在同名消歧噪声，因此上述完整调用边同时以同目录直接引用核验。

## 错误处理与边界

- 六个方法都不返回 `Result`。锁获取使用 `unwrap()`；若持锁线程 panic 导致锁中毒，后续访问也会 panic，而不是恢复或传播结构化错误。
- `Remove` 对不存在的键返回 `0`，不区分“键不存在”和“存在但跟踪内存为 0”。
- `AddKeyValue` 覆盖同键时不返回旧值或旧成本；调用方若要结算替换成本，必须在 LFU 层完成。
- `Get` 用 `None` 表示未命中。Rust 类型不允许集合中保存空表指针，因此没有 Go 版本“键存在但值为 `nil`”的状态。
- `Keys` 无排序保证；不得把输出顺序用于稳定序列化、测试顺序或淘汰优先级。
- 单文件自身接受任意 `i64` 键，包括负数；负数边界由分片层的数组索引暴露。`key_set_shard_test.rs::negative_key_panics_like_go_array_indexing` 验证负键在路由时 panic。
- `Keys` 后再 `Get` 不是原子操作，调用方必须像 `LFU::Values` 一样容忍键在两次调用之间消失。

## 并发与资源生命周期

读操作 `Get`、`Keys`、`Len` 共享读锁；`AddKeyValue`、`Remove`、`Clear` 独占写锁。同一方法不会把锁守卫返回给调用方：`Keys` 复制键，`Get` 克隆 `Arc`，`Len` 复制数值，所以锁只覆盖最小的映射访问区间。

`keySetShard` 通过 256 把独立锁降低不同表 ID 间的竞争，但单个热点分片仍由本文件的一把锁协调。跨分片的 `Keys`、`Len`、`Clear` 是逐分片执行的弱一致操作；并发写入时，它们可能观察到不同时刻的分片状态。`Arc<Table>` 只保证值的所有权生命周期安全，不自动保证 `Table` 内部任意可变字段的同步；本文件仅替换共享指针，不原地修改表内容。

覆盖、删除和清空会递减对应 `Arc` 的强引用计数，最后一个强引用离开作用域时才释放表。`Clear` 用新映射替换旧映射，避免逐键删除时长期持锁，但析构大量旧值仍与该调用相关。源码没有异步任务、通道、事务或 I/O。

## 与 Go 版本的对应关系

直接对照文件为同目录 `key_set.go`，结构和六个方法一一对应：Go 的 `map[int64]*statistics.Table + sync.RWMutex` 对应 Rust 的 `HashMap<i64, Arc<Table>> + RwLock`；`Remove`、`Keys`、`Len`、`AddKeyValue`、`Get`、`Clear` 保留相同的锁粒度与基本语义。

主要语言差异如下：

- Go `Get` 返回 `(*Table, bool)`；Rust 合并为 `Option<Arc<Table>>`。
- Go 映射可保存 `nil`，所以 `Remove` 在计算成本前检查 `table != nil`；Rust 的 `Arc<Table>` 非空，命中后可直接计算成本。
- Go 通过显式 `Lock/Unlock` 管理锁；Rust 守卫在作用域结束时自动释放。若锁中毒，Rust 的 `unwrap()` 会 panic，Go 的 `RWMutex` 没有对应的中毒状态。
- Go `Clear` 创建新 map，Rust 同样整体替换为 `HashMap::new()`；Rust `Default` 还能保证初始映射已分配为可写空映射。
- Go 返回原始表指针，Rust 返回克隆的 `Arc`，明确延长共享值生命周期。
- 两边的 `Keys` 都复制 map 键且不承诺顺序；两边的分片层都逐分片汇总，因此都不是全局一致快照。

相关 Go 回归位于 `lfu_cache_test.go`。它通过 `TestLFUPutGetDel`、`TestCacheLen`、两个并发 Put/Get 测试和小容量淘汰测试验证二级集合的删除、长度、枚举与壳表保留语义。Rust 的 `lfu_cache_test.rs` 保留了相应意图。

## 扩展指南

- 若新增单键原子操作，应首先在 `keySet` 内一次持锁完成，再在 `keySetShard` 增加同语义转发；不要在上层用 `Get` 后 `AddKeyValue` 拼接读改写，否则会产生竞态。
- 若需要一致的全量快照、原子全局清空或精确并发长度，必须同时设计跨 256 个分片的协调方案；仅修改本文件不能提供跨分片原子性，并可能显著增加热点锁竞争。
- 若调整值类型或所有权模型，需要同步检查 `LFU::Put/Get/Values`、淘汰监听与 `retainEvictedShell`，确保主缓存和二级集合之间不会出现悬垂引用或意外深拷贝。
- 若使用 `Remove` 返回成本进行结算，应先明确它与 `LFUState::addCost`、替换监听和异步淘汰之间的唯一责任，避免重复扣减；当前分片层有意忽略返回值。
- 若改变缺失键、覆盖、排序或锁中毒行为，需要同步 Go 对照语义，或在文档和兼容测试中明确差异。
- 测试必须放在独立文件。单分片行为可新增同目录 `key_set_test.rs` 并由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 接入；涉及分片路由的测试应扩展 `key_set_shard_test.rs`，涉及对外缓存语义和并发的测试应扩展 `lfu_cache_test.rs`，并参考 `lfu_cache_test.go` 的同名场景。

性能风险集中在锁持有时间、`Keys` 的 O(n) 分配、`Clear` 的批量析构和热点键所在分片的竞争；兼容风险集中在 Go/Rust 空值差异、无序枚举和弱一致快照语义。

## 验证依据

- 目标源码：`pkg/statistics/handle/cache/internal/lfu/key_set.rs`，共 74 行；核对了 `keySet` 字段和六个方法的完整实现。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/cache/internal/lfu` 找到本文件及相邻 Go/Rust 实现和测试；精确 `node` 查询覆盖 `keySet`、`Remove`、`Keys`、`Len`、`AddKeyValue`、`Get`、`Clear`，并查询了 `key_set_shard.rs::Get/AddKeyValue` 与 `lfu_cache.rs::Put/Values`。
- crate 与模块：`pkg/statistics/handle/cache/internal/lfu/Cargo.toml`、`lib.rs`。
- 直接调用链：`key_set_shard.rs`、`lfu_cache.rs`；通过精确符号查询和 `rg` 直接引用交叉验证。
- Go 对照：`key_set.go`、`key_set_shard.go`、`lfu_cache.go`。
- 独立测试：`key_set_shard_test.rs`、`lfu_cache_test.rs`、`lfu_cache_test.go`。当前没有同名 `key_set_test.rs`，所以单分片逻辑由分片层和 LFU 对外测试间接覆盖。
- 人工复核结论：本文件存在的原因、写入/回落/淘汰/枚举/清理流程、锁与 `Arc` 生命周期、Go 差异及安全扩展位置均可由上述符号和文件反查；没有把未接线能力描述为已支持。
