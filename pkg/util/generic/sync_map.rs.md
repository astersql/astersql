# `pkg/util/generic/sync_map.rs`

## 文件定位

本文件属于 `astersql-util-generic` crate（`pkg/util/generic/Cargo.toml`），实现一个以 `std::sync::RwLock` 保护 `std::collections::HashMap` 的泛型并发映射。crate 入口 `pkg/util/generic/lib.rs` 通过 `pub mod sync_map` 声明模块，并用 `pub use sync_map::*` 将 `SyncMap` 与 `NewSyncMap` 提升到 crate 根路径。

它是 `pkg/util/generic/sync_map.go` 的 Rust 移植。当前仓库的 Rust 文本引用只出现在模块入口和独立测试 `pkg/util/generic/sync_map_test.rs`、`pkg/util/generic/migration_aster_unit_test.rs` 中，尚未发现生产 Rust 调用者。因此，它目前是一个已公开、已测试但尚未接入 Rust 应用主链的基础容器，不能把 Go 侧的生产使用场景直接描述成 Rust 侧已经接线。

## 核心职责

- `SyncMap<K, V>` 把键值表及其同步机制封装为一个对象，避免调用者分别维护 `HashMap` 和锁。
- `NewSyncMap` 接受容量提示并预分配哈希表；容量只影响初始分配，不是元素上限。
- `Store` 提供插入与覆盖，`Load` 提供克隆值的只读查询，`Delete` 原子地移除并返回旧值，`Keys` 返回调用时刻锁内收集到的键快照。
- API 保留 Go 版本的 PascalCase 名称和 `(value, exist)` 风格的存在标记；Rust 用 `Option<V>` 取代 Go 的类型零值。

本文件不负责淘汰、容量限制、遍历回调、原子复合更新或异步调度，也没有后台任务和外部 I/O。

## 主要符号

- `pub struct SyncMap<K, V> where K: Eq + Hash`：唯一的数据类型。私有字段 `item: RwLock<HashMap<K, V>>` 同时限定键必须可判等、可哈希，并阻止调用者绕过锁直接访问底层表。
- `pub fn NewSyncMap<K, V>(capacity: isize) -> SyncMap<K, V>`：唯一的自由函数。负容量执行 `panic!("capacity cannot be negative")`；非负容量转换为 `usize` 后交给 `HashMap::with_capacity`。
- `pub fn Store(&self, key: K, value: V)`：取得写锁并调用 `HashMap::insert`。同键已有值时旧值被丢弃，方法不返回它。
- `pub fn Load(&self, key: &K) -> (Option<V>, bool) where V: Clone`：取得读锁，通过 `HashMap::get` 查找；命中时克隆值并返回 `(Some(value), true)`，未命中时返回 `(None, false)`。
- `pub fn Delete(&self, key: &K) -> (Option<V>, bool)`：取得写锁并调用 `HashMap::remove`；命中时把所有权移出映射并返回旧值，未命中时返回 `(None, false)`。
- `pub fn Keys(&self) -> Vec<K> where K: Clone`：取得读锁，按当前长度预分配 `Vec`，克隆全部键后返回。哈希表遍历顺序没有稳定保证。

文件没有模块级常量、trait、枚举、条件编译项或私有辅助函数。除 `item` 字段外，以上类型和操作均为公开 API。

## 执行流程

1. 调用者以 `NewSyncMap(capacity)` 建表。构造函数先拒绝负值，再创建带容量提示的空 `HashMap` 并放入新的 `RwLock`。
2. 写入时，`Store` 独占写锁；`insert` 在同一临界区内完成新增或覆盖；guard 离开函数时自动解锁。
3. 查询时，`Load` 持有共享读锁完成查找和克隆。返回后不再持锁，所以调用者得到的是值快照，而不是映射内部值的借用。
4. 删除时，`Delete` 持有独占写锁完成查找和移除。`remove` 使“判断是否存在”和“删除并取得旧值”发生在同一临界区。
5. 枚举键时，`Keys` 在共享读锁下收集全部键，释放锁后返回独立向量。后续写入不会修改已返回向量，但该向量也不会反映后续变化。

各公开操作只获取一次锁，不嵌套调用其他 `SyncMap` 方法，因此本实现内部不存在重入锁顺序；复合业务操作（例如“若不存在则插入”）若由调用者用 `Load` 加 `Store` 拼接，则两个调用之间可能被其他线程修改，不具备原子性。

## 数据与状态

唯一持久状态是 `item` 中的 `HashMap<K, V>`。键和值以所有权形式存储；`Store` 消耗传入的键和值，`Delete` 把被删除值的所有权交还调用者。`Load` 为避免返回跨越锁生命周期的引用而要求 `V: Clone`，`Keys` 同理要求 `K: Clone`。

`capacity` 是构造时的分配提示，表可以继续增长。`Keys` 的 `Vec::with_capacity(item.len())` 只用于避免快照收集时的常规扩容。文件不维护版本号、命中统计、最大容量或逻辑关闭状态。

返回值存在一个刻意的双层表示：`Option<V>` 表示是否有值，`bool` 对齐 Go API 的 `exist`。当前实现保证二者一致，即只有 `(Some(_), true)` 和 `(None, false)`；测试也依赖此不变量。

## 依赖与调用关系

下游依赖全部来自标准库：`HashMap` 提供存储，`Eq + Hash` 提供键约束，`RwLock` 提供同步。`pkg/util/generic/Cargo.toml` 没有声明第三方依赖或 feature，并用 `[lib] path = "lib.rs"` 指定 crate 入口。

上游 Rust 接线由 `pkg/util/generic/lib.rs` 完成：模块被公开并整体再导出。RustCodeGraph 将目标文件索引为 7 个符号，但对这些符号的精确 `callers/callees` 查询未解析出可用生产调用边；仓库级 Rust 搜索进一步确认，直接调用仅来自 `sync_map_test.rs` 和 `migration_aster_unit_test.rs`。虽然 `pkg/resourcegroup/runaway`、`pkg/ddl` 与 `pkg/ddl/ingest` 的 Cargo manifest 声明了该 crate 依赖，当前这些目录的 Rust 源码没有引用 `SyncMap`/`NewSyncMap`，不能据此声称它们已使用此实现。

Go 对照实现有明确生产调用者：`pkg/ddl/ddl.go` 用它登记 DDL job 完成通道，`pkg/ddl/ingest/engine.go` 用它缓存 engine writer，`pkg/resourcegroup/runaway/manager.go` 用它保存指标 counter，`pkg/util/extsort/disk_sorter.go` 用它记录并发压缩中已删除的文件编号。这些调用说明抽象的目标场景，但不是 Rust 调用边。

## 错误处理与边界

- `NewSyncMap` 对负容量主动 panic；`migration_aster_unit_test.rs::sync_map_negative_capacity_panics_like_go` 用 `catch_unwind` 固化该行为。超大非负容量仍可能因底层分配失败而终止，文件没有返回可恢复错误。
- 所有锁获取都使用 `expect`。若另一个线程在持写锁期间 panic 导致锁中毒，后续读操作以 `"SyncMap read lock poisoned"` panic，写/删操作以 `"SyncMap write lock poisoned"` panic；实现不会从 `PoisonError` 中恢复内部数据。
- 查找或删除不存在的键不是错误，均返回 `(None, false)`；删除存在键返回原值。`Store` 覆盖旧值也不是错误，但旧值不可观察。
- `Load` 的克隆可能有显著成本，也可能在用户定义的 `Clone` 实现中 panic；克隆发生在读锁持有期间，会延长写者等待时间。`Keys` 同样在读锁内克隆每个键。
- `Keys` 不承诺排序。测试在多键场景显式排序后断言；依赖自然遍历顺序会产生不稳定行为。
- 本 API 没有 `Default`、零值可用性或从现有 `HashMap` 构造的保证，调用者应通过 `NewSyncMap` 初始化。

## 并发与资源生命周期

`RwLock` 允许多个 `Load`/`Keys` 同时持读锁；`Store`/`Delete` 需要独占写锁，并与所有读写操作互斥。锁 guard 由 RAII 管理，正常返回或栈展开都会释放锁；但持锁期间 panic 会留下中毒标记。映射本身没有显式 `Close`，最后一个所有者被丢弃时，锁、哈希表及剩余键值一并释放。

类型的跨线程能力由标准库自动 trait 推导和具体 `K`、`V` 决定，本文件没有手写 `Send`/`Sync`。测试 `migration_aster_unit_test.rs::sync_map_supports_concurrent_access` 用 `Arc<SyncMap<usize, usize>>` 启动 4 个线程，各写入并立即读取 100 个互不重叠的键，线程汇合后检查共有 400 个键，证明了该类型参数组合的并发读写路径。

每次方法调用都具有锁保护下的单操作原子性，但返回快照后状态可能立刻变化。尤其是先 `Load` 后 `Store`、先 `Keys` 后逐项处理等跨调用流程没有事务隔离。标准库 `RwLock` 也不在此文件中提供公平性承诺；高竞争场景应评估克隆时间、哈希计算和长读快照对写延迟的影响。

## 与 Go 版本的对应关系

`pkg/util/generic/sync_map.go` 使用 `map[K]V` 加 `sync.RWMutex`，Rust 用一个 `RwLock<HashMap<K, V>>` 表达相同的受保护状态。`Store` 对应 `Lock` 后赋值，`Load` 和 `Keys` 对应 `RLock` 下读取，`Delete` 对应写锁下先取旧值再删除。

关键语言差异如下：

- Go 的键约束是 `comparable`，Rust 明确要求 `Eq + Hash`。
- Go `Load`/`Delete` 在未命中时返回 `V` 的零值与 `false`；Rust 没有通用零值，返回 `None` 与 `false`。Rust 命中 `Load` 还要求 `V: Clone`，而 Go 返回值复制语义由具体类型决定。
- Go 方法接收键值 `K`，Rust 的 `Load`/`Delete` 接收 `&K`，避免为查询转移键所有权；`Store` 仍取得键值所有权。
- Go 显式解锁，Rust 由 guard 生命周期自动解锁；Rust 锁还具有中毒语义，Go `RWMutex` 没有对应的 poisoning 状态。
- 两版 `Keys` 都返回无序键快照。Rust 在获得读锁后读取长度并收集；Go 当前源码在加读锁前以 `len(m.item)` 预分配容量，因此 Rust 的整个读取过程具有更完整的锁保护。
- 两版构造器都把容量当预分配提示；负容量分别由 Go `make(map, capacity)` 与 Rust 的显式检查触发 panic。

`pkg/util/generic/sync_map_test.rs::TestSyncMap` 按 Go 测试顺序覆盖基本增删查、覆盖和无序键集合；迁移补充测试还验证了 `Delete` 返回旧值、并发访问和负容量 panic。

## 扩展指南

新增操作应优先放在 `impl<K, V> SyncMap<K, V>` 中，并选择满足语义的最窄锁：只观察状态用读锁，修改状态或要求“检查并修改”不可分割时用单个写锁。不要通过连续调用现有公开方法实现需要原子性的复合操作，因为每个方法返回时锁已经释放；例如 `LoadOrStore` 应在同一个写锁 guard 下检查并插入。

设计新 API 时需要明确所有权与成本：返回拥有值可能要求 `Clone`，返回引用则需要把 guard 生命周期暴露给调用者，批量回调若在锁内执行还会引入重入、长临界区和用户 panic 风险。若增加迭代、清空、长度或条件更新，应写清快照/线性化语义以及锁中毒策略，并保持 Go 对照行为，除非有证据记录刻意差异。

测试必须继续放在独立文件，优先扩展 `pkg/util/generic/sync_map_test.rs` 的 Go 对齐用例；Rust 特有的并发、panic 或所有权边界可扩展 `pkg/util/generic/migration_aster_unit_test.rs`。若 Go API 同步变化，还应核对 `pkg/util/generic/sync_map.go` 与 `sync_map_test.go`。性能敏感改动应特别验证高竞争读写、昂贵 `Clone`、大量键快照及哈希冲突场景，兼容性上则注意现有 PascalCase 再导出路径和 `(Option<V>, bool)` 返回契约。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引可用（目标目录 10 个已索引文件）；`files --filter pkg/util/generic` 确认目标、模块入口及 Rust/Go 测试均入图；`node --file pkg/util/generic/sync_map.rs --offset 1 --limit 240` 读取了 108 行完整实现，并确认文件含 7 个符号；`query SyncMap`、`query NewSyncMap --kind function` 定位了类型与构造器。精确 `callers/callees` 查询未产出可用边，因此调用关系另以仓库文本搜索核验，未把缺失图边解释为生产调用。
- 实现与 crate 边界：`pkg/util/generic/sync_map.rs`、`pkg/util/generic/lib.rs`、`pkg/util/generic/Cargo.toml`。
- Go 对照及生产场景：`pkg/util/generic/sync_map.go`、`pkg/ddl/ddl.go`、`pkg/ddl/ingest/engine.go`、`pkg/resourcegroup/runaway/manager.go`、`pkg/util/extsort/disk_sorter.go`。
- 独立测试：`pkg/util/generic/sync_map_test.rs`、`pkg/util/generic/migration_aster_unit_test.rs`、`pkg/util/generic/sync_map_test.go`。覆盖基本增删查、覆盖、键集合、旧值返回、并发访问和负容量 panic；没有直接覆盖锁中毒、公平性、昂贵克隆或高竞争性能。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复核唯一新增生产物、源码引用和事实/限制边界。
