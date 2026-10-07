# `pkg/kv/cachedb.rs`

## 文件定位

`pkg/kv/cachedb.rs` 属于 `astersql-kv` crate。该 crate 由 `pkg/kv/Cargo.toml` 的 `[lib] path = "lib.rs"` 定义；`pkg/kv/lib.rs` 在 `cachedb` 模块中通过 `include!("cachedb.rs")` 装入本文件，并用 `pub use cachedb::*` 导出其公开项。文件没有条件编译项；只有配套测试由模块入口以 `#[cfg(test)]` 装入 `pkg/kv/cachedb_test.rs`。

它提供按 table ID 隔离的只读回填缓存：位于事务缓冲/调用方与 `Snapshot` 读取之间，命中时避免再次访问底层快照，未命中时经 `GetValue` 回源并写回。公开边界是 `MemManager` trait 和构造函数 `NewCacheDB`，具体类型 `cacheDB` 以及环形缓存后端均未公开。

需要注意当前接线状态：`Storage::GetMemCache` 在 `pkg/kv/kv.rs` 中以 `&dyn MemManager` 暴露缓存，但仓库内对 `NewCacheDB()` 的精确搜索只找到测试调用。TiKV 适配器目前在 `pkg/store/driver/kv_adapter.rs` 使用独立的 `AdapterMemManager`，mock storage 也在 `pkg/store/mockstore/mockstorage/canonical_storage.rs` 实现自己的 `MemManager`。因此，本文件是完整且已测试的缓存实现，但不能据现有直接证据声称生产 `TikvStore` 正在实例化它。

## 核心职责

- `cacheDB` 用 `RwLock<HashMap<i64, TableCache>>` 保存 table ID 到独立缓存的映射。每个首次出现的 table ID 都分配 `TABLE_CACHE_CAPACITY = 100 MiB`，不是所有表共享一个 100 MiB 配额。
- `MemManager::UnionGet` 实现“缓存优先、快照回源、成功后回填”的读取流程；`MemManager::Delete` 以 table ID 为粒度使整张表的缓存失效。
- `TableCache` 和 `CacheSegment` 在本文件内部移植 `coocood/freecache v1.2.1` 被此 API 使用的固定容量、256 分段、xxHash64 定位、环形写入、近似按访问时间淘汰及覆盖写行为，从而避免 Rust 版本只是一个无容量约束的 `HashMap`。
- 写入前保持 Go/freecache 的两项输入限制：key 不超过 `u16::MAX` 字节，且 key 与 value 总长不超过 `100 MiB / 1024 - 24 = 102376` 字节。
- 缓存只保存成功读取到的值，不缓存快照错误；零长度 value 虽会写入后端，但 `cacheDB::get` 将其折叠为未命中，因此后续 `UnionGet` 仍会回源。

## 主要符号

- `pub trait MemManager: Send + Sync`：存储层缓存契约。`UnionGet(&Context, tid, &dyn Snapshot, &Key) -> Result<Vec<u8>, Error>` 执行联合读取，`Delete(tableID)` 清除单表缓存。`Send + Sync` 使 trait object 可跨线程共享。
- `pub fn NewCacheDB() -> Box<dyn MemManager>`：创建空 `cacheDB`，用 trait object 隐藏具体实现与内部锁。
- `struct cacheDB`：表级容器，仅含 `memTables: RwLock<HashMap<i64, TableCache>>`。私有方法 `set` 负责按需建表、校验 entry 并写入，`get` 负责命中查询与值复制。
- `TABLE_CACHE_CAPACITY`、`FREECACHE_MAX_KEY_LENGTH`、`FREECACHE_ENTRY_HEADER_SIZE`、`FREECACHE_MAX_KEY_VALUE_LENGTH`：分别固定单表容量、key 上限、24 字节头部和单 entry 上限；这些值参与 Go 兼容边界。
- `struct TableCache`：包含 256 个 `Mutex<CacheSegment>`。`insert`/`get` 都以 `xxhash_rust::xxh64(key, 0)` 选中 `hash & 255` 对应的 segment，并传入当前秒级 `u32` 时间。
- `CacheEntryPtr`：slot 索引项，记录环形区偏移、16 位 hash 摘要和 key 长度；保留字段使布局意图与 freecache 的 16 字节指针项一致。
- `CacheHeader`：序列化在环形字节区中的 entry 元数据，包括访问时间、key/value 长度与容量、删除标记、slot；`size` 返回头、key 和预留 value 容量的总占用。
- `CacheSegment`：实现固定字节环、256 个按 `hash16` 排序的 slot、空间游标与访问时间统计。关键方法为 `read`/`write`（处理环回）、`header`/`write_header`、`lookup`、`remove`、`insert_ptr`、`evacuate`、`set` 和 `get`。

## 执行流程

1. 调用方取得 `dyn MemManager` 后调用 `UnionGet(ctx, tid, snapshot, key)`。
2. `cacheDB::get` 获取表映射读锁，按 `tid` 找到 `TableCache`；后者计算 xxHash64，锁定一个 segment，并用 slot 内的 `hash16`、key 长度和实际 key 字节确认命中。命中会更新时间统计并复制 value 返回。
3. 非空 value 命中时直接返回，不访问 `snapshot`。表不存在、key 不存在、entry 已被淘汰或 value 长度为零都视为未命中。
4. 未命中时，`UnionGet` 调用 `pkg/kv/kv.rs::GetValue(ctx, snapshot, key.clone())`。它通过 `Snapshot` 继承的 `Getter` 接口读取单键；错误使用 `?` 原样向上传播，且不回填。
5. 回源成功后，`cacheDB::set` 取得表映射写锁并先用 `entry(tableID).or_insert_with(TableCache::new)` 建立单表 100 MiB 缓存。这个建表动作发生在大小校验之前，所以被拒绝的首个写入仍会留下空表缓存。
6. 输入通过长度限制后，`TableCache::insert` 锁定目标 segment。`CacheSegment::set` 若找到旧 entry 且原 `value_cap` 足够，就原位改写；否则标记旧 entry 删除，按倍增规则计算新容量，并为新 entry 腾出环形空间。
7. `evacuate` 从最旧环形位置前进：已删除 entry 直接回收；活 entry 若访问时间不高于平均水平，或连续搬迁超过 5 次，则淘汰，否则复制到环尾并更新 slot 指针。之后插入排序指针、头、key、value并更新 `end`、`vacuum`、`total_time`、`total_count`。
8. `Delete(tableID)` 取得表映射写锁并移除整个 `TableCache`。不存在的 table ID 是幂等空操作；移除后由 Rust drop 释放所有 segment、索引和环形缓冲。

## 数据与状态

状态分三层。最外层 `memTables` 定义 table ID 隔离边界；中间层 `TableCache::segments` 按 hash 低 8 位把键分到 256 个 segment；内层 `CacheSegment` 同时维护固定长度字节环 `data`、逻辑写尾 `end`、可用空间 `vacuum`、访问统计以及每个 slot 的有序指针区。

单表 100 MiB 平均分为 256 个 409600 字节 segment。最大 entry 限制是单 segment 容量的四分之一减 24 字节头，即 key 与 value 合计最多 102376 字节。容量压力发生在单个 segment 内：即使整张表其他 segment 为空，碰撞到同一 segment 的 entry 仍可能被淘汰。

entry 的 value 容量和当前 value 长度分开记录。缩小覆盖可复用原空间；增长覆盖会留下带 `deleted` 标记的旧环形记录，直到 `evacuate` 走到它才回收并扣减统计。slot 数组以每 slot 相同的 `slot_cap` 连续布局，任一 slot 满时整体将容量翻倍并搬移 256 个 slot 的指针。

时间由 `SystemTime` 相对 Unix epoch 的秒数截断为 `u32`。`get` 使用 `now.wrapping_sub(old_access_time)` 对齐 Go 的 `uint32` 回绕减法；`evacuate` 用访问时间与全体平均值比较做近似淘汰。这里没有 TTL API，序列化头的 expire-at 字节始终为零，对应 Go 调用 `freecache.Set(key, value, 0)`。

## 依赖与调用关系

上游公开关系如下：`pkg/kv/lib.rs` 装入并重导出本模块；`pkg/kv/kv.rs::Storage::GetMemCache` 返回 `&dyn MemManager`，因此上层通过 trait 使用缓存而不依赖 `cacheDB`。RustCodeGraph 报告 `cachedb.rs` 被 24 个文件使用，但对 `NewCacheDB` 返回的 trait object 以及 `UnionGet` 动态分派没有建立 callers/callees 边；精确搜索确认 `NewCacheDB` 当前直接调用位于 `pkg/kv/cachedb_test.rs` 和 `pkg/kv/assertion_1_aster_unit_test.rs`。

直接下游包括：

- `pkg/kv/kv.rs::GetValue`、`Snapshot`、`Key`、`Error` 与 `context::Context`，均通过模块入口的 `use crate::*` 进入作用域。
- 标准库 `HashMap`、`RwLock`、每 segment 的 `Mutex`、`SystemTime` 和 `Vec`。
- `xxhash-rust` 的 `xxh64` feature；`pkg/kv/Cargo.toml` 明确声明 `xxhash-rust = { version = "0.8", features = ["xxh64"] }`。
- `errors::New`，用于把表锁中毒和 freecache 兼容校验错误转换为 crate 的共享错误类型。

当前生产适配证据也很重要：`pkg/store/driver/kv_adapter.rs::AdapterMemManager` 与 `pkg/store/mockstore/mockstorage/canonical_storage.rs` 各自实现相同 trait。修改 trait 签名会同时影响这些实现；修改本文件私有环形后端则只影响实际构造 `NewCacheDB` 的路径。

## 错误处理与边界

- `cacheDB::set` 获取 `memTables.write()` 失败时，以 `errors::New(err.to_string())` 返回锁中毒错误。之后的 key/value 限制也返回与 Go freecache 一致的固定文本。
- `cacheDB::get` 对表映射读锁中毒使用 `.ok()?`，把它折叠为缓存未命中；`UnionGet` 随后会尝试回源。`Delete` 同样在写锁失败时静默不删除。这与 `set` 的显式报错不对称，扩展时不应未经兼容评估改变。
- 每个 `CacheSegment` 的 `Mutex` 使用 `lock().unwrap()`：segment 锁中毒会 panic，而不是转成 `Error`。内部 `try_into().unwrap()` 和“live ring entry must have a slot pointer”的 `expect` 依赖数据结构不变量，损坏时也会 panic。
- `SystemTime` 早于 Unix epoch 时 `unwrap_or_default()` 使用零时间；秒数转 `u32` 会截断。测试明确覆盖时钟倒退时的 wrapping 语义，但不提供外部可注入时钟。
- 空 value 不构成可复用命中。它会占用缓存 entry，下一次仍回源并覆盖；这与 Go 中 `nil`/零长度切片在该路径的行为保持一致，但不适合作为“负缓存”。
- 快照读取成功、缓存写入失败时，整个 `UnionGet` 返回写缓存错误而不是已取得的 value。调用者必须把缓存容量错误视为读取失败。
- `Delete` 与不存在的 table ID、重复删除均安全；淘汰只代表缓存丢失，后续读取会回源，不改变底层快照。

## 并发与资源生命周期

`MemManager: Send + Sync` 加上锁保护使同一个 trait object 可跨线程共享。表映射读锁覆盖完整的 `TableCache::get`，写锁覆盖建表与完整的 `TableCache::insert`，所以同一时刻删除不能与任何表的本文件 get/set 交错；代价是即使访问不同 table ID，写入也由全局表映射写锁串行化，读取期间也会阻塞所有建表、写入和删除。

进入 `TableCache` 后，每次操作只锁定一个 hash segment。该 segment 内环形数据、slot 索引和统计由同一 `Mutex` 原子更新；不同 segment 理论上可并行，但外层 `cacheDB` 锁的持有范围会进一步限制并发，特别是所有 set 都先持有外层写锁。

缓存没有后台线程、异步任务、通道或显式 close。`NewCacheDB` 创建空映射；某个 table 首次 `set` 时一次性为它分配约 100 MiB 环形字节区及索引；`Delete` 或整个 manager drop 时同步释放。大量 table ID 即使只写入一个小值，也会各自承担固定 100 MiB 分配，调用方应在表不再需要时及时 `Delete`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/cachedb.go`。公开契约一一对应：Go `MemManager` 的 `UnionGet`/`Delete` 对应 Rust 同名 trait 方法，Go `NewCacheDB() MemManager` 对应 Rust `NewCacheDB() -> Box<dyn MemManager>`，Go 的 `map[int64]*freecache.Cache` 与 `sync.RWMutex` 对应 Rust 的 `HashMap<i64, TableCache>` 与 `RwLock`。

Go 在首次 set 时调用 `freecache.NewCache(100 * 1024 * 1024)`，然后 `Set(key, value, 0)`；Rust 在本文件内移植所需的 freecache v1.2.1 后端，保持 100 MiB、256 segment、xxHash64、零 TTL、输入上限、覆盖容量、环形搬迁和淘汰语义。`pkg/kv/cachedb_test.rs::freecache_v1_2_1_differential_ring_traces` 用相同操作流与 Go freecache oracle 的四组 digest 校验内部状态，而不只比较最终命中数。

可观察差异主要来自语言错误模型。Go `sync.RWMutex` 不会返回中毒状态，Rust 外层锁可能被映射为错误、未命中或静默删除失败；Rust segment `Mutex` 中毒则 panic。Go `Delete` 先 `Clear()` 再从 map 删除，Rust 因没有逸出的 `TableCache` 引用而直接 `remove` 并 drop，最终资源效果等价。Go 依赖外部 `github.com/coocood/freecache`，Rust Cargo 只声明 `xxhash-rust`，缓存算法代码在本文件内并保留其 MIT 许可说明。

仓库没有 `pkg/kv/cachedb_test.go`；Go 对照行为来自生产 `cachedb.go` 与 Rust 测试内可重放的 freecache v1.2.1 oracle。不能据此声称已运行本任务中的测试，因为本次按纯文档任务要求不运行 Cargo。

## 扩展指南

- 若改变缓存命中/回源/错误传播语义，首先修改 `MemManager::UnionGet` 或 `cacheDB::{get,set}`，并同步扩展独立文件 `pkg/kv/cachedb_test.rs`；不要把 Rust 测试嵌回生产源文件。还应检查 `pkg/kv/assertion_1_aster_unit_test.rs::cache_db_preserves_freecache_entry_limits`。
- 若改变 trait 方法或返回类型，必须同时检查 `pkg/store/driver/kv_adapter.rs::AdapterMemManager`、`pkg/store/mockstore/mockstorage/canonical_storage.rs` 的实现以及 `pkg/kv/kv.rs::Storage::GetMemCache`，否则 crate 级接口会断裂。
- 若要让生产 TiKV 路径实际采用本实现，应在存储适配器的 `GetMemCache` 构造/持有点接线并补集成级证据；仅修改 `NewCacheDB` 不会自动替换当前静态 `AdapterMemManager`。
- 若调整容量、segment 数、hash 切分、entry 头布局、淘汰、覆盖增长或时间统计，必须把它当作 Go/freecache 兼容变更：更新边界测试、segment 压力测试、覆盖 tombstone 测试和差分 digest，并确认错误文本是否仍需兼容。
- 若优化并发，最敏感处是外层 `RwLock` 当前覆盖内部 segment 操作的锁顺序。缩小锁范围需要为 `TableCache` 引入可安全共享的所有权（例如 `Arc`），同时证明 `Delete` 与进行中的 get/set 的失效语义；避免形成外层锁与 segment 锁的反向获取。
- 若增加负缓存或允许空值命中，必须明确区分“不存在”和“存在但为空”，同时核对 `GetValue`/`ValueEntry` 契约，不能仅删除当前的 `filter(|value| !value.is_empty())`。
- 性能与资源风险包括每表固定 100 MiB、同 segment 热点、全局写锁串行化、读操作复制整个 value，以及 slot 扩容一次搬移所有 256 个 slot。新增指标或可配置容量时需保持默认 Go 行为，并验证删除后资源释放。

## 验证依据

- RustCodeGraph `status`：索引包含 11467 个文件、307296 个节点和 1848419 条边；目标源码可由 `node --file pkg/kv/cachedb.rs` 完整读取，并报告被 24 个文件使用。
- RustCodeGraph 查询：`query NewCacheDB --kind function` 定位 Rust `cachedb.rs::NewCacheDB` 与 Go `pkg/kv/cachedb.go::NewCacheDB`；`query cacheDB --kind struct` 定位 `cacheDB`、`TableCache`、`CacheEntryPtr`、`CacheHeader`、`CacheSegment`；`query MemManager --kind trait` 定位本文件 trait；`query GetValue --kind function` 定位 `pkg/kv/kv.rs::GetValue`。对 Rust `NewCacheDB` 和 trait `UnionGet` 的 callers/callees 查询返回空，故调用接线另以精确文本搜索核验并在本文明确标注图限制。
- 已读生产与装配文件：`pkg/kv/cachedb.rs`、`pkg/kv/lib.rs`、`pkg/kv/kv.rs`、`pkg/kv/Cargo.toml`、`pkg/store/driver/kv_adapter.rs`、`pkg/store/mockstore/mockstorage/canonical_storage.rs`。
- 已读 Go 对照：`pkg/kv/cachedb.go`。仓库中不存在同名 `pkg/kv/cachedb_test.go`。
- 已读独立 Rust 测试：`pkg/kv/cachedb_test.rs`，覆盖拒绝边界、复制隔离、覆盖、幂等删除、空值/错误、`Send + Sync`、segment 压力淘汰、freecache 差分轨迹、并发读写删除和 tombstone/容量增长；另读 `pkg/kv/assertion_1_aster_unit_test.rs::cache_db_preserves_freecache_entry_limits`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查本文存在且恰有 11 个固定二级标题，并人工复核所有“当前已接线”陈述均有上述源码或搜索证据。
