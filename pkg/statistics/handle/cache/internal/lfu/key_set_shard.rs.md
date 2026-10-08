# `pkg/statistics/handle/cache/internal/lfu/key_set_shard.rs`

## 文件定位

本文件实现 LFU 统计缓存内部的分片二级键集合。crate 入口 `pkg/statistics/handle/cache/internal/lfu/lib.rs` 将它声明为私有模块 `mod key_set_shard`，因此文件中的 `pub` 类型和方法只服务于该 crate 的实现与测试，并不是 crate 对外再导出的公共 API。直接使用者是 `lfu_cache.rs` 的 `SharedState::resultKeySet`；主缓存未命中、条目被拒绝或淘汰时，该集合继续保存表统计对象或降级后的“壳表”。

crate 由 `pkg/statistics/handle/cache/internal/lfu/Cargo.toml` 定义，包名为 `astersql-statistics-handle-cache-internal-lfu`。本文件自身只直接使用标准库的 `Arc`、同 crate 的 `key_set::keySet`，以及 `astersql-statistics` 依赖提供的 `statistics::Table`。

## 核心职责

- 用固定的 256 个 `keySet` 分散表 ID 的访问，避免所有二级集合读写竞争同一把锁；分片数由 `keySetCnt` 固定。
- 让 `Get`、`AddKeyValue` 和 `Remove` 只锁定单个分片，并让 `Keys`、`Len`、`Clear` 逐分片汇总或操作。
- 保持与 Go `keySetShard` 相同的取模路由和无序枚举语义，同时用 `Arc<Table>` 表达 Go 指针的共享所有权。
- 为 `lfu_cache.rs` 提供主缓存之外的完整键视图：`LFU::Get` 回落查询它，`LFU::Values` 和 `LFU::Len` 从它枚举，删除与清空路径也同步更新它。

它不负责 TinyLFU 准入、容量核算、表统计降级或指标更新；这些行为位于 `lfu_cache.rs`。它也不维护跨分片的全局锁或一致性快照。

## 主要符号

- `pub const keySetCnt: usize = 256`：固定分片数量，也是 `resultKeySet` 数组的编译期长度和取模基数。
- `pub struct keySetShard { resultKeySet: [keySet; keySetCnt] }`：分片容器。字段私有，每个 `keySet` 内部拥有独立的 `RwLock<HashMap<i64, Arc<Table>>>`（见 `key_set.rs::keySet`）。
- `newKeySetShard() -> Self`：使用 `std::array::from_fn` 为 256 个槽位分别调用 `keySet::default()`，保证每个分片有独立空映射和独立锁。
- `shardIndex(key: i64) -> usize`：内部路由函数，计算 `(key % 256) as usize`。
- `Get(key) -> Option<Arc<Table>>`：路由后调用 `keySet::Get`；命中时底层克隆 `Arc`。
- `AddKeyValue(key, table)`：路由后调用 `keySet::AddKeyValue`，同键写入会覆盖旧值。
- `Remove(key)`：路由后删除键，刻意忽略 `keySet::Remove` 返回的内存成本。
- `Keys() -> Vec<i64>`：依次复制每个分片当前的键并拼接。
- `Len() -> usize`：对 256 个分片的当前长度求和。
- `Clear()`：依次调用每个分片的 `Clear`，保留数组和锁对象，只替换分片内部映射。

没有 trait、枚举、条件编译项或模块级可变状态。

## 执行流程

1. `NewLFU` 在建立 `SharedState` 时调用 `keySetShard::newKeySetShard()`，一次性初始化固定数组（`lfu_cache.rs::NewLFU`）。
2. 单键操作先经 `shardIndex` 做 `key % keySetCnt` 路由，再只进入一个 `keySet`：读操作取得该分片读锁，写入与删除取得该分片写锁（`key_set.rs::{Get, AddKeyValue, Remove}`）。
3. `LFU::Put` 在主缓存准入完成前先调用 `AddKeyValue` 发布表对象，因此超容量拒绝路径仍可由 `LFU::Get` 回落命中；淘汰路径 `SharedState::retainEvictedShell` 也以相同方法写回降级表。
4. `LFU::Get` 先查询 Moka 主缓存，未命中时调用本文件的 `Get`；返回的 `Arc` 允许调用方在释放分片读锁后继续持有表。
5. `LFU::Del` 调用 `Remove`；`LFU::Values` 先取 `Keys`，再逐键 `Get`；`LFU::Len` 直接调用 `Len`。
6. `LFU::Close` 与 `LFU::Clear` 在处理主缓存后调用本文件的 `Clear`，逐个清空二级分片。

批量方法不是单一原子操作：`Keys`、`Len` 和 `Clear` 在遍历时分别获取并释放各分片锁；并发写入可能夹在两个分片的观察点之间。

## 数据与状态

核心状态是长度固定为 256 的 `[keySet; keySetCnt]`。同一个 `i64` 表 ID 总由确定性的余数映射到同一分片；非负键的下标范围是 `0..256`。每个底层 `keySet` 保存 `HashMap<i64, Arc<Table>>`，所以：

- 键是物理表 ID，值是共享所有权的统计表；读取只增加 `Arc` 引用计数，不复制 `Table`。
- 同键覆盖由 `HashMap::insert` 完成，本层不累计历史值。
- `Keys` 的结果顺序先受分片编号影响，分片内又受 `HashMap` 迭代顺序影响，不能用于稳定排序。
- `Remove` 不利用底层返回的跟踪内存；内存成本由 `lfu_cache.rs` 的主缓存生命周期和 `SharedState::addCost/dropMemory` 管理。
- `Clear` 释放集合持有的 `Arc`，但调用方已克隆的 `Arc<Table>` 可继续存活。

## 依赖与调用关系

上游调用边由 `lfu_cache.rs` 的直接引用确认：

- `SharedState` 持有 `keySetShard`，`NewLFU` 负责构造。
- `SharedState::retainEvictedShell` 和 `LFU::Put` 调用 `AddKeyValue`。
- `LFU::Get` 调用 `Get` 作为主缓存未命中的回落路径。
- `LFU::Del` 调用 `Remove`。
- `LFU::Values` 调用 `Keys` 后再逐键调用 `Get`；`LFU::Len` 调用 `Len`。
- `LFU::Close`、`LFU::Clear` 调用 `Clear`。

下游依赖全部集中到 `key_set.rs::keySet`：本文件将单键和批量操作转发给其 `Get`、`AddKeyValue`、`Remove`、`Keys`、`Len`、`Clear`。数据类型来自 `statistics::Table`，共享生命周期由 `std::sync::Arc` 提供。Cargo 清单还声明 Moka、缓存接口、指标和 `sysinfo` 等 crate 级依赖，但它们不被本文件直接引用；Moka 主缓存与容量逻辑由 `lfu_cache.rs` 使用。

RustCodeGraph 能确认目标文件的符号集合、`key_set_shard_test.rs`/`lfu_cache_test.rs` 对文件的使用关系，以及 `shardIndex -> keySetCnt` 等引用；由于 Go/Rust 同名和方法名高度重复，方法级图查询无法可靠区分全部调用边，所以上述上游边同时由精确源码引用核验。

## 错误处理与边界

- API 不返回业务错误。底层 `keySet` 对 `RwLock` 使用 `unwrap()`；若锁因持锁线程 panic 而中毒，后续访问会继续 panic，而不是恢复或返回错误。
- 负键是显式保留的兼容边界：Rust 的负余数转换成 `usize` 后会成为巨大下标，数组索引随即 panic。`key_set_shard_test.rs::negative_key_panics_like_go_array_indexing` 用 `#[should_panic]` 固化了这一行为，与 Go 负数组下标 panic 对应。因此调用者必须提供非负表 ID，不应在本层静默归一化负数。
- `Remove` 删除不存在的键是无操作；`Get` 返回 `None`；空集合的 `Keys` 为空、`Len` 为 0。
- `Keys` 后再 `Get` 存在竞态：键可能在两步之间被并发删除，因此 `LFU::Values` 使用 `filter_map` 接受缺失；不能把二者组合视为快照。
- `Clear` 在 256 个分片之间不具备全局原子性，并发读写可能观察到部分分片已清空、部分尚未清空的过渡状态。

## 并发与资源生命周期

分片容器本身没有额外锁；并发保证来自每个 `keySet` 独立的 `RwLock`。不同余数的键通常落在不同锁上，可并行读写；同一分片内允许并发读，但写入、删除和清空互斥。`lfu_cache_test.rs::TestLFUCachePutGetWithManyConcurrency` 以 32 个线程写读 1000 个键并检查 `Len`/`Values` 完整性，`TestLFUCachePutGetWithManyConcurrency2` 以多组读写线程覆盖并发路径。

`newKeySetShard` 创建后数组与锁的数量终身不变；`Clear` 只用新 `HashMap` 替换每个锁内的映射。值以 `Arc<Table>` 管理：集合写入取得一份所有权，`Get` 克隆引用，覆盖、删除或清空只释放集合自己的引用。分片遍历不同时持有所有锁，减少长时间全局阻塞，但代价是批量结果只代表逐分片观察的组合状态。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/statistics/handle/cache/internal/lfu/key_set_shard.go`。常量、数组布局、构造、单键路由以及 `Get/AddKeyValue/Remove/Keys/Len/Clear` 的职责逐项对应：两端均以 `key % 256` 选择分片，均逐分片汇总键和长度，且均逐分片清空。

主要语言映射如下：Go 的 `*statistics.Table` 对应 Rust 的 `Arc<Table>`；Go 的 `(*Table, bool)` 对应 Rust 的 `Option<Arc<Table>>`；Go 构造函数返回指针，Rust 返回拥有值并由 `SharedState` 持有；Go 每个 `keySet` 显式初始化 `map` 和 `sync.RWMutex`，Rust 通过 `Default` 初始化 `RwLock<HashMap<...>>`。Go `Remove` 也忽略底层内存成本返回值。

两端对负键都不会提供正常结果，但触发机制不同：Go 的负余数直接作为数组下标 panic，Rust 先将负余数转换为 `usize`，再因越界索引 panic。Go 侧当前没有专门的 `keySetShard` 单元测试；Rust 独立测试补充了该兼容边界。并发与缓存集成语义主要由两端各自的 `lfu_cache_test` 覆盖。

## 扩展指南

- 改变分片数或路由算法时，应同时修改 `keySetCnt`、`shardIndex` 与 Go 对照实现，并评估已有键在运行期路由变化、锁竞争、固定数组内存以及负键行为；若需稳定迁移，不能只改取模常量。
- 新增单键操作时，优先在 `key_set.rs` 实现锁内原子动作，再由 `keySetShard` 只负责路由，避免在 `Get` 与写操作之间拼接非原子的读改写流程。
- 新增全量操作时，需要明确是一致快照还是弱一致遍历。若要求全局原子性，必须设计固定加锁顺序并评估一次持有 256 把锁的死锁与延迟风险，不能默认复用当前逐分片模式。
- 改动返回值或内存成本语义时，要同步检查 `lfu_cache.rs::{Put, Del, Values, Len, Close, Clear}` 及 `SharedState::{retainEvictedShell, dropMemory}`，避免二级集合状态与成本指标脱节。
- 测试必须保留在独立文件：直接边界测试同步更新 `key_set_shard_test.rs`，缓存生命周期和并发行为同步更新 `lfu_cache_test.rs`；Go 语义发生变化时还应同步 `key_set_shard.go` 与 `lfu_cache_test.go`。重点回归非负边界（0、255、256）、同分片冲突键、覆盖/删除不存在键、并发清空，以及负键 panic。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标目录列出 `key_set_shard.rs`、`key_set.rs`、`lfu_cache.rs` 及对应 Go/测试文件。
- RustCodeGraph `node --file pkg/statistics/handle/cache/internal/lfu/key_set_shard.rs`：核对 93 行目标源码、14 个符号，以及测试文件使用关系。
- RustCodeGraph `query KeySetShard` / `query key_set_shard` 与 callers/callees 查询：核对 Rust/Go 对照符号、`shardIndex -> keySetCnt` 引用；同时发现同名方法消歧限制，未将含混图结果作为调用结论。
- `pkg/statistics/handle/cache/internal/lfu/key_set.rs`：核对每分片的 `RwLock<HashMap<i64, Arc<Table>>>`、锁范围、`Arc` 克隆和 `Remove` 成本返回值。
- `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`：核对 `SharedState` 持有关系及 `NewLFU/Get/Put/Del/Values/Len/Close/Clear`、淘汰壳表路径的直接调用。
- `pkg/statistics/handle/cache/internal/lfu/Cargo.toml` 与 `lib.rs`：核对 crate 边界、依赖、私有模块声明和独立测试模块接线。
- `pkg/statistics/handle/cache/internal/lfu/key_set_shard.go` 与 `key_set.go`：核对 Go 的分片布局、取模、锁、返回值和批量操作语义。
- `pkg/statistics/handle/cache/internal/lfu/key_set_shard_test.rs`：核对负键 panic；`lfu_cache_test.rs` 的 `TestLFUPutTooBig`、`TestCacheLen`、`TestLFUCachePutGetWithManyConcurrency`、`TestLFUCachePutGetWithManyConcurrency2`：核对回落可见性、长度与并发完整性。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令确认目标文件存在且恰有 11 个固定二级章节。
