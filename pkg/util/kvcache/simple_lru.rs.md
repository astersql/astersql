# `pkg/util/kvcache/simple_lru.rs`

## 文件定位

`simple_lru.rs` 是 `astersql-util-kvcache` crate 的核心实现文件，提供按字节哈希寻址、按最近使用顺序淘汰的通用内存缓存。crate 入口 `pkg/util/kvcache/lib.rs` 通过 `mod simple_lru` 装入本文件，并用 `pub use simple_lru::*` 重导出其公开 API；`pkg/util/kvcache/Cargo.toml` 表明该 crate 的唯一直接依赖是同仓库的 `astersql-util-memory`，用于读取实例内存和创建全局内存 Tracker。

它位于 SQL 执行路径可复用的工具层，而不是一个自带锁或后台任务的缓存服务。已核实的 Rust 生产调用者 `pkg/executor/internal/applycache/apply_cache.rs` 用它缓存相关子查询的内层 `chunk::List`：`NewApplyCache` 构造 `SimpleLRUCache`，查询和写入分别调用 `Get`、`Put`，外层内存配额回收调用 `RemoveOldest`。该调用者用 `syncutil::Mutex` 串行化所有访问，正好对应本文件“非线程安全”的契约。

## 核心职责

- 以 `Key::Hash() -> Vec<u8>` 的结果作为唯一索引键，在 `HashMap<Vec<u8>, usize>` 中以平均常数时间定位缓存节点。
- 以 `front`（最近使用，MRU）到 `back`（最久未使用，LRU）的双向链表维护近因顺序；`Get` 和同键 `Put` 会刷新顺序，`Peek` 不会。
- 同时实施条目数上限 `capacity` 与可选的进程实例内存阈值。`quota == 0` 时完全跳过较昂贵的 `memory::InstanceMemUsed()`；否则 `Put` 会在实例内存超过 `quota * (1 - guard)` 或条目数超限时持续淘汰。
- 在自动淘汰路径上提供 `onEvict` 回调，并提供显式删除、清空、调整容量和弹出最旧项等管理 API。
- 用 `Arc` 对键和值做共享所有权和类型擦除，使调用方能存储任意 `Any + Send + Sync` 值，并通过下转恢复具体类型。

本实现不统计单条缓存项的精确大小。配额判断依据是整个进程的 `InstanceMemUsed`，而 `GlobalLRUMemUsageTracker` 在本文件中只负责惰性创建；本文件没有对它调用 `Consume`，不能把它解释为本缓存的逐项内存账本。

## 主要符号

- `pub trait Key: Send + Sync`：公开键协议。实现者必须返回稳定的字节哈希；缓存只比较哈希字节，不再比较原始键。
- `pub type KeyRef = Arc<dyn Key>`：公开键句柄，允许缓存及调用者共享键对象。
- `pub type Value = Arc<dyn Any + Send + Sync>`：公开的类型擦除值句柄；具体调用者负责保证写入和下转类型一致。
- `CacheEntry`：私有链表节点，保存 `key`、`value` 以及指向 `entries` 槽位的 `previous`/`next` 索引。
- `pub static GlobalLRUMemUsageTracker: OnceLock<Box<memory::Tracker>>`：进程级惰性单例。`init` 或首次构造缓存时以 `LabelForGlobalSimpleLRUCache` 和无限制额度 `-1` 创建。
- `pub const ProfileName`：保持 Go 堆分析命名的常量 `github.com/pingcap/tidb/pkg/util/kvcache.(*SimpleLRUCache).Put`。
- `pub struct SimpleLRUCache`：缓存主体。`capacity`、`size` 公开，其余索引、链表、回调和配额状态私有。
- `NewSimpleLRUCache(capacity, guard, quota)`：公开构造函数。容量小于 1 时断言失败；同时保证全局 Tracker 已初始化。
- 公开方法：`SetOnEvict`、`Get`、`Peek`、`Put`、`Delete`、`DeleteAll`、`Size`、`Values`、`Keys`、`SetCapacity`、`RemoveOldest`。
- 私有链表方法：`entry`/`entry_mut` 读取活动槽位，`insert_front` 插入 MRU，`move_to_front` 刷新近因性，`remove` 摘链并回收槽位，`evict_oldest` 删除 LRU 并按参数决定是否回调。
- `go_float64_to_uint64`：模拟 Go/amd64 的 `float64 -> uint64` 阈值转换；负数先转 `i64` 再转 `u64`，保留补码结果。

本文件没有条件编译项；测试条件编译位于 `lib.rs`，测试逻辑分别保存在独立的 `simple_lru_test.rs` 和 `migration_aster_unit_test.rs` 中。

## 执行流程

1. 构造：`NewSimpleLRUCache` 校验 `capacity >= 1`，调用 `init` 以 `OnceLock::get_or_init` 创建全局 Tracker，然后建立空的哈希表、槽位表、空闲表和空链表。
2. 插入新键：`Put` 计算一次 `key.Hash()`；若哈希已存在，仅替换值并用 `move_to_front` 标为 MRU，`size` 不变，也不执行配额淘汰。若不存在，则通过 `insert_front` 优先复用 `free` 槽位，再写入 `elements` 并增加 `size`。
3. 淘汰：零配额分支只在 `size > capacity` 时调用一次 `evict_oldest(true)`。有配额分支先读取 `InstanceMemUsed`，再在“实例内存超过阈值”或“条目数超过容量”期间循环删除 `back`；只有仍超内存阈值时才重新读取实例内存。
4. 查询：`Get` 命中后先 `move_to_front`，再克隆值的 `Arc`；`Peek` 只克隆值，不改变链表。两者未命中都返回 `(None, false)`。
5. 显式移除：`Delete` 按哈希移除特定键；`DeleteAll` 从 `back` 反复摘除；`RemoveOldest` 摘除并返回 LRU 键值；`SetCapacity` 校验新容量后从 LRU 端收缩到目标大小。这四条路径都不调用 `onEvict`。
6. 枚举：`Keys` 和 `Values` 从 `front` 沿 `next` 遍历，因此结果顺序固定为 MRU 到 LRU。

链表操作维持以下不变量：空缓存时 `front`/`back` 都为 `None`；非空时 `front.previous == None`、`back.next == None`；每个 `elements` 下标指向 `entries` 中的活动节点；`size` 等于活动节点数；被删除的槽位为 `None` 且下标进入 `free`，之后可由 `insert_front` 复用。

## 数据与状态

`elements` 保存“哈希字节到节点槽位”的主索引，`entries` 保存稳定下标的节点，`free` 保存已释放槽位。这种设计避免链表重排时移动节点，也避免每次淘汰都重新分配节点存储。代价是显式删除或 `DeleteAll` 只释放节点内持有的 `Arc`，不会缩小 `entries`/`free` 的容量；缓存对象存活期间，槽位数组的峰值分配会被保留供后续复用。

`capacity` 和 `size` 是 `usize`，与 64 位 Go 平台的 `uint` 宽度对齐；`Size` 为兼容 Go 风格 API 返回 `isize`。`quota` 是字节上限，0 明确表示禁用进程内存检查。有效阈值为 `go_float64_to_uint64(quota as f64 * (1.0 - guard))`：`guard == 1` 得到 0；`guard > 1` 产生负浮点值并按 Go 兼容规则转成较大的无符号值，而不是饱和为 0。代码没有限制 `guard` 范围，调用方需理解这一语义。

键和值都由 `Arc` 持有。返回 `Keys`/`Values` 或命中查询只增加引用计数；淘汰或删除只移除缓存持有的引用，其他调用方仍可继续持有对象。`Value` 的动态类型没有运行时 schema，类型错误通常在调用方 `Arc::downcast` 时暴露，例如 `ApplyCache::Get` 期望值恒为 `chunk::List`。

## 依赖与调用关系

向下依赖如下：

- 标准库 `HashMap` 负责哈希索引，`Arc` 负责共享所有权，`Any` 负责值类型擦除，`OnceLock` 负责全局 Tracker 的单次初始化。
- `crate::memory::InstanceMemUsed` 提供进程实例内存；`NewTracker` 与 `LabelForGlobalSimpleLRUCache` 创建全局 Tracker。这些符号由 `pkg/util/kvcache/lib.rs` 从 `astersql-util-memory` 重导出。
- `Put -> insert_front/move_to_front/evict_oldest`，`Get -> move_to_front`，`Delete/DeleteAll/RemoveOldest -> remove`，`SetCapacity -> evict_oldest(false)`，`evict_oldest -> remove` 是文件内的关键调用边。

向上已核实的 Rust 使用链为：`pkg/executor/internal/applycache/apply_cache.rs::NewApplyCache -> NewSimpleLRUCache`；其私有 `get/put/removeOldest` 分别调用 `Get/Put/RemoveOldest`，并由 `ApplyCache::Get/Set` 在相关子查询结果缓存路径使用。`pkg/executor/internal/applycache/lib.rs` 通过 `kvcache_dependency` 重导出本 crate，并明确用互斥锁补足线程安全边界。

RustCodeGraph 对通用名称 `Put`、`Get` 等存在跨语言同名歧义，因此文档只采用能由目标文件、精确文件限定图查询或依赖声明共同确认的调用边；没有把查询输出中的无关同名函数计为本实现调用者。

## 错误处理与边界

- 构造时容量为 0 会 `panic`，消息为 `capacity of LRU Cache should be at least 1.`；动态调整容量为 0 则返回 `Err(String)`，不会修改原容量。
- `Get`/`Peek` 未命中返回 `(None, false)`；`Delete` 删除不存在的键是无操作；空缓存 `RemoveOldest` 返回 `(None, None, false)`。
- 有配额的 `Put` 若第一次或后续 `InstanceMemUsed` 查询失败，会调用 `DeleteAll` 并返回；这是清空降级，不向调用者传播错误，也不触发 `onEvict`。
- `entry`、`entry_mut` 和 `remove` 用 `expect("live LRU entry")` 保护内部槽位不变量。若索引、链表与槽位表失配，会直接 panic，说明这是内部一致性缺陷而非可恢复输入错误。
- 哈希字节是键身份的全部依据。两个逻辑不同但 `Hash()` 返回相同字节的键会被当作同一项；可变键若插入后改变哈希，删除时可能无法清理原索引。实现 `Key` 时必须保证哈希稳定且碰撞策略符合业务需要。
- 同键 `Put` 只替换 `value`，保留最初插入的 `CacheEntry.key`。若不同键对象共享哈希，后来的键对象不会替换节点内原键，回调和 `Keys` 仍观察原键。
- `onEvict` 只表示由 `Put` 的容量/配额控制触发的自动驱逐；它不覆盖 `Delete`、`DeleteAll`、`SetCapacity` 或 `RemoveOldest`。依赖回调做资源记账时必须区分这些路径。

## 并发与资源生命周期

`SimpleLRUCache` 没有内部锁，所有修改方法要求 `&mut self`，但这不等于面向共享业务状态的自动同步。需要跨线程共享时，调用方应像 `ApplyCache` 一样在外层使用互斥锁，并把一次复合操作所需的读取、淘汰和记账放在同一串行化协议内。`Key` 和 `Value` 要求 `Send + Sync`，`onEvict` 要求 `Send`，这些约束允许对象跨线程转移，但不提供缓存级原子性。

全局 Tracker 由 `OnceLock` 管理，首次 `init`/构造后存活到进程结束，重复初始化保留同一实例。每个缓存自身没有线程、通道、定时器、文件句柄或自定义 `Drop`；缓存销毁时，`HashMap`、槽位和仍持有的 `Arc` 按 Rust RAII 自动释放。显式淘汰先把节点从链表和索引中删除、递减 `size`，再调用 `onEvict`，因此回调运行时缓存内部已经处于删除后的状态。

配额循环观察的是进程级内存，释放一个缓存项未必立即降低观测值（其他 `Arc` 可能仍持有值，分配器也可能保留内存），所以循环可能继续淘汰直至缓存为空。代码通过 `back.is_none()` 终止空缓存场景，避免无限循环。

## 与 Go 版本的对应关系

Rust 文件逐项对照 `pkg/util/kvcache/simple_lru.go`：Go 的 `map[string]*list.Element` 对应 Rust 的 `HashMap<Vec<u8>, usize> + entries`；Go `container/list` 对应索引双向链表；Go `Key`/`any` 对应 Rust `Key`/`Arc<dyn Any + Send + Sync>`；公开方法、回调触发范围、MRU→LRU 枚举顺序、容量错误消息、quota/guard 淘汰条件及 `ProfileName` 均保持同一意图。

需要注意的实现层差异：

- Go 包 `init()` 在包加载时建立 Tracker；Rust 没有同等包初始化机制，因此 `NewSimpleLRUCache` 显式调用 `init`，也允许调用方提前调用公开 `init`。
- Go 用字符串承载任意哈希字节；Rust 直接用 `Vec<u8>`，避免文本编码转换，仍保留按完整字节序列比较的语义。
- Rust 用 `usize` 保留 64 位平台 Go `uint` 的容量范围；迁移测试专门覆盖超过 `u32::MAX` 的容量。
- Rust 的 `go_float64_to_uint64` 专门覆盖 `guard > 1` 时的负阈值转换；不能用 Rust 默认的负浮点到无符号饱和转换替代。
- Go 值可以为任意接口值；Rust 要求值 `Send + Sync + 'static` 并放入 `Arc`。这是线程边界与所有权模型带来的更强约束。

`pkg/util/kvcache/simple_lru_test.rs` 对齐主要 Go 测试；`migration_aster_unit_test.rs` 额外锁定二进制哈希、同键更新、显式删除/收缩/弹出、Tracker 初始化、原生字宽以及 `guard > 1` 等移植边界。

## 扩展指南

- 新增命中统计、淘汰原因或观测指标时，优先在 `Get`、`Peek`、`Put` 和 `evict_oldest` 接入；必须明确显式删除是否计入淘汰，并在独立的 `simple_lru_test.rs` 增加各路径断言。
- 改变回调语义时，要同时检查 `Put` 的零配额与有配额分支、内存查询失败后的 `DeleteAll`、`SetCapacity`、`RemoveOldest` 和 `Delete`，避免只修改一种删除入口。回调目前在自动淘汰且内部状态更新后执行，这一时序属于兼容性契约。
- 新增按项内存记账不能只复用 `InstanceMemUsed`；应设计键值大小来源、更新同键时的差额、外部 `Arc` 持有导致的实际释放延迟，以及与 `GlobalLRUMemUsageTracker` 的 `Consume`/父子挂接关系。
- 修改链表结构时必须同时维护 `elements`、`entries/free`、`front/back` 和 `size` 五组状态，并覆盖删除头、尾、中间节点、唯一节点、槽位复用及 `Get` 提升顺序。测试逻辑继续放在独立测试文件，不内嵌进生产文件。
- 修改键协议前先评估哈希碰撞和可变哈希风险；若要支持“哈希相同但逻辑键不同”，需要同时更改索引结构和 Go 兼容语义，不能只在单个查询方法补比较。
- 调整 `guard` 合法范围或浮点转换时，应同步 `simple_lru.go` 的行为与 `migration_aster_unit_test.rs::guard_above_one_preserves_go_unsigned_threshold_conversion`，并评估已有调用方传入边界值的兼容风险。
- 扩展线程安全能力前先检查 `ApplyCache` 等现有外层锁，避免形成重复锁或改变复合记账操作的锁粒度；更合适的默认仍是让通用缓存保持轻量，由调用者选择同步策略。

## 验证依据

- 生产实现：`pkg/util/kvcache/simple_lru.rs`，核对了全部 353 行以及 `Key`、`CacheEntry`、`SimpleLRUCache`、构造函数、12 个公开方法/函数和 6 个私有辅助函数；文件无条件编译分支。
- crate 边界：`pkg/util/kvcache/Cargo.toml`、`pkg/util/kvcache/lib.rs`；确认 crate 名、唯一直接依赖、内存符号桥接、公开重导出和独立测试装配。
- Go 对照：`pkg/util/kvcache/simple_lru.go`、`pkg/util/kvcache/simple_lru_test.go`；核对公开 API、链表顺序、回调范围、容量和 OOM 守卫语义。
- Rust 测试：`pkg/util/kvcache/simple_lru_test.rs`、`pkg/util/kvcache/migration_aster_unit_test.rs`；覆盖容量淘汰、零配额、guard 阈值、Get/Peek 顺序、更新不增容、删除/清空/弹出、错误容量、二进制哈希、Tracker 初始化、原生字宽和 profile 名称。
- 直接调用方：`pkg/executor/internal/applycache/apply_cache.rs`、`pkg/executor/internal/applycache/lib.rs`；确认外层互斥、构造、查询、写入和最旧项淘汰调用链。
- 内存标签：`pkg/util/memory/tracker.rs::LabelForGlobalSimpleLRUCache`，确认标签值由 memory crate 提供。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/kvcache` 确认目标、模块入口和两份 Rust 测试均被索引；`node --file pkg/util/kvcache/simple_lru.rs --offset 1 --limit 360` 读取全文件；文件限定的 `callers/callees` 核实 `Put -> evict_oldest`、`SetCapacity -> evict_oldest` 等内部边。对同名符号产生的跨语言噪声未作为结论。
- 人工复核：本文能回答文件存在原因、构造/查询/淘汰流程、状态不变量、错误与回调边界、并发责任、Go 对应关系以及安全扩展位置；未将预期架构写成当前事实。
