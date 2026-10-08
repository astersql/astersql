# `pkg/resourcemanager/util/shard_pool_map.rs`

## 文件定位

本文件实现资源管理器使用的、按名称分片的池容器映射。它位于 `pkg/resourcemanager/util`，在仓库中有两种装配方式：`pkg/resourcemanager/util/lib.rs` 将其声明为公开模块并再导出全部公开符号，对应独立 crate `astersql-resourcemanager-util`；同时 `pkg/resourcemanager/lib.rs` 通过 `#[path = "util/shard_pool_map.rs"] mod shard_pool_map` 将同一实现直接纳入 `astersql-resourcemanager`，再从 `crate::util` 暴露 `NewShardPoolMap`、`ShardPoolMap` 和 `PoolMapError`。

在完整资源管理流程中，`pkg/resourcemanager/rm.rs` 的 `ResourceManagerInner::poolMap` 持有 `Arc<ShardPoolMap>`。`ResourceManager::Register`/`Unregister` 分别落到本文件的 `Add`/`Del`，`Reset` 用 `NewShardPoolMap` 替换整张表；后台调度入口 `pkg/resourcemanager/schedule.rs::ResourceManager::schedule` 则调用 `Iter`，把每个 `PoolContainer` 交给调度决策与执行逻辑。

## 核心职责

- 以固定的 8 个 `PoolMap` 分片保存“池名称 -> `PoolContainer`”关系；`hash` 仅用 key 的首字节选取分片。
- 用每个分片独立的 `RwLock<HashMap<...>>` 隔离锁竞争，使落在不同分片的注册、注销和遍历不必共享同一把映射锁。
- 提供注册、幂等删除和全量遍历三个操作，并在重复注册时返回稳定的 `PoolMapError`。
- 用 `Arc<PoolContainer>` 保持表内对象地址与共享所有权稳定，同时向遍历回调只暴露借用 `&PoolContainer`，不开放映射内部结构。

本文件只管理注册表，不负责池的创建、调容、停止或释放。`PoolContainer` 内的 `GoroutinePool` 行为由 `pkg/resourcemanager/util/util.rs` 定义，调度策略位于 `pkg/resourcemanager/schedule.rs`。

## 主要符号

- `const SHARD: usize = 8`：固定分片数，与 Go 的 `shard = 8` 一致；数组长度和取模基数共同依赖该常量。
- `pub fn hash(key: &str) -> usize`：读取 UTF-8 字节序列的第一个字节，计算 `byte % SHARD`。这是公开函数，但前置条件是 key 非空。
- `pub struct ShardPoolMap`：公开外壳，内部字段 `pools: [PoolMap; SHARD]` 私有，调用者不能绕过锁操作分片。
- `pub fn NewShardPoolMap() -> ShardPoolMap`：通过 `std::array::from_fn` 构造 8 个相互独立的空 `PoolMap`。名称保留 Go 风格，因此文件启用了 `non_snake_case`。
- `ShardPoolMap::Add(&self, key: String, pool: PoolContainer) -> Result<(), PoolMapError>`：选择一个分片并转交内部 `PoolMap::Add`；key 与容器所有权都移入映射。
- `ShardPoolMap::Del<K: AsRef<str>>(&self, key: K)`：接受 `String`、`&str` 等字符串形态，选择分片并删除；无返回值。
- `ShardPoolMap::Iter<F: FnMut(&PoolContainer)>(&self, callback: F)`：按分片数组顺序逐个遍历，把每个容器的共享借用传给可变回调。
- `pub struct PoolMapError`：无字段错误类型，实现 `Debug`、`Eq`、`PartialEq`、`Display` 和 `std::error::Error`；显示文本固定为 `pool is already exist`。
- 私有 `PoolMap`：一个分片的实现，字段为 `RwLock<HashMap<String, Arc<PoolContainer>>>`；其 `new`、`Add`、`Del`、`Iter` 承担实际锁定与容器操作。

本文件没有 trait、枚举、条件编译项或模块级可变状态。

## 执行流程

1. `NewShardPoolMap` 调用 8 次 `PoolMap::new`，每次创建独立的空 `HashMap` 和 `RwLock`。
2. 注册时，公开 `Add` 先调用 `hash(&key)`。私有 `PoolMap::Add` 获取目标分片写锁，在同一临界区内检查 `contains_key` 并插入，因此同一 key 的查重与写入不会被并发注册穿透。若 key 已存在，保持原值并返回 `Err(PoolMapError)`；否则把容器包进 `Arc` 后返回 `Ok(())`。
3. 注销时，`Del` 用同样的散列规则定位唯一分片，获取写锁并调用 `HashMap::remove`。不存在的 key 不产生错误，所以重复注销是幂等的。
4. 遍历时，`Iter` 按数组下标从 0 到 7 顺序处理分片；每个私有 `PoolMap::Iter` 获取该分片读锁，遍历 `HashMap::values()`，并在锁仍被持有时逐项调用回调。完成一个分片后释放其读锁，再进入下一分片。
5. 应用侧 `ResourceManager::schedule` 克隆当前映射的 `Arc` 后调用 `Iter`；回调会跳过 `DistTask`，对其余容器运行 `schedulePool` 和 `Exec`。因此映射替换与一次已经开始的调度遍历互不破坏：调度持有的是替换前或替换后的一个完整 `Arc` 快照。

## 数据与状态

持久状态仅存在于各分片的 `HashMap<String, Arc<PoolContainer>>`。字符串 key 是池的注册名称；值中的 `PoolContainer` 绑定 `Arc<dyn GoroutinePool>` 与所属 `Component`。外层 `ShardPoolMap` 自身不保存元素数、迭代游标或散列种子。

同一个 key 永远映射到同一个分片，因为注册、删除都调用相同的 `hash`。散列只观察首字节：具有相同首字节的名称必然竞争同一把锁，后续字节不参与分片选择；非 ASCII 名称按 UTF-8 编码的首字节分片，而不是按 Unicode 字符值分片。分片内采用标准 `HashMap`，所以单个分片内的回调顺序不稳定；唯一稳定的外层顺序是先处理低编号分片。

映射把传入的 `PoolContainer` 包装为新的 `Arc`。`Iter` 从该 `Arc` 解引用得到临时借用，并不把容器所有权或 `Arc` 克隆交给调用者；回调不能把该引用保留到调用结束之后。

## 依赖与调用关系

直接标准库依赖为 `HashMap`、`Error`、`fmt`、`Arc` 和 `RwLock`，没有第三方 crate 依赖。业务类型依赖 `crate::util::PoolContainer`；在独立 util crate 中它来自同目录 `util.rs`，在主资源管理 crate 中则由 `pkg/resourcemanager/lib.rs` 的 `util` 门面从 `scheduler_dependency::util` 再导出。

已核实的上游边如下：

- `pkg/resourcemanager/rm.rs::ResourceManager::new_with_schedulers` 和 `Reset` 调用 `NewShardPoolMap`。
- `pkg/resourcemanager/rm.rs::registerPool` 调用 `ShardPoolMap::Add`，公开 `Register` 为其构造 `PoolContainer`。
- `pkg/resourcemanager/rm.rs::Unregister` 调用 `ShardPoolMap::Del`。
- `pkg/resourcemanager/schedule.rs::ResourceManager::schedule` 调用 `ShardPoolMap::Iter`，并在回调中继续调用调度与调容逻辑。
- `pkg/resourcemanager/util/shard_pool_map_test.rs::TestShardPoolMap` 直接覆盖构造、添加、重复添加、遍历、删除和重复删除。

Cargo 边界由 `pkg/resourcemanager/util/Cargo.toml` 明确为无额外依赖的库 crate，入口是 `lib.rs`；工作区根 `Cargo.toml` 以 `facade_resourcemanager_util` 引用它。生产资源管理器本身由 `pkg/resourcemanager/Cargo.toml` 定义，并通过源码路径装配本文件，而不是依赖该 facade crate。

## 错误处理与边界

- 空 key 会在 `hash` 的 `key.as_bytes()[0]` 处 panic；所有 `Add`、`Del` 调用者都必须保证名称非空。该行为与 Go 对空字符串执行 `key[0]` 的越界失败一致。
- 重复 key 的 `Add` 返回 `PoolMapError`，不会覆盖已注册容器。错误不携带 key 或分片信息，调用者若要提供上下文需在边界外补充。
- 删除不存在的 key 是成功的空操作；Rust 独立测试最后一次 `pm.Del("0")` 专门覆盖该语义。
- `RwLock::read()` 和 `write()` 均直接 `unwrap()`；若某个持锁线程 panic 导致锁中毒，后续操作也会 panic，而不是返回可恢复错误。
- 回调自身返回 `()`，因此 `Iter` 没有错误传播或提前终止协议。回调 panic 会向上传播，并使当时持有的分片读锁中毒。
- 分片数、首字节散列和错误文案都是与现有 Go 行为相关的兼容面；单独修改任一项可能改变竞争分布、删除定位或上层错误断言。

## 并发与资源生命周期

`Add` 与 `Del` 对目标分片取独占写锁；`Iter` 对当前分片取共享读锁。不同分片可并发读写，同一分片的写操作会等待该分片的读者和写者。查重与插入在同一写锁临界区内，保证重复注册判定的原子性。

`Iter` 不会一次锁住全部 8 个分片，所以遍历不是整张映射的原子快照：并发更新可能使回调看到各分片处于不同时间点的状态。另一方面，回调执行期间当前分片的读锁一直保留；回调若同步调用会写入同一分片的 `Add` 或 `Del`，可能发生自阻塞。耗时回调也会延长该分片写操作的等待时间，扩展时不应在回调中执行无界阻塞工作。

删除条目会释放映射持有的 `Arc<PoolContainer>`；若没有其他强引用，容器随之析构，但本文件不会调用 `GoroutinePool::ReleaseAndWait`。池的停机必须由更高层生命周期显式负责。主资源管理器外层的 `RwLock<Arc<ShardPoolMap>>` 与本文件分片锁是两级同步：`schedule` 先克隆 `Arc` 再遍历，从而不在整个回调期间持有外层锁。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/resourcemanager/util/shard_pool_map.go`。Rust 保留了 Go 的 8 分片、首字节取模、逐分片遍历、读写锁、删除不存在 key 的空操作，以及错误文本 `pool is already exist`。Go 的 `*PoolContainer` 对应 Rust 映射中的 `Arc<PoolContainer>`；Go 构造函数返回指针，Rust 构造函数返回值，实际主资源管理器随后再用 `Arc` 共享外层映射。

最重要的当前语义差异在重复注册：Go `poolMap.Add` 仅在 `contain && !intest.InTest` 时返回错误，在 `intest.InTest` 模式下会覆盖原值；Rust 没有 `intest.InTest` 分支，任何环境中只要 key 已存在就返回 `PoolMapError` 并保留原值。对应测试也反映了该差异：Go 测试仅在 `!intest.InTest` 时断言重复添加失败，Rust 测试无条件断言失败。扩展或对齐时不能把两者描述为完全相同。

另一个实现差异是锁失败处理：Go 的 `sync.RWMutex` 没有中毒概念，Rust 的 `RwLock` 在中毒后会因 `unwrap` panic。两端都在遍历回调期间持有当前分片读锁，也都不承诺 `map`/`HashMap` 内部顺序。

## 扩展指南

- 若增加按 key 查询、条件删除或更新操作，应继续通过 `hash` 选择唯一分片，并把“检查 + 修改”放在同一个锁临界区；不要先读后写制造竞态。对应测试应放在独立文件 `pkg/resourcemanager/util/shard_pool_map_test.rs`，不要内嵌到生产源文件。
- 若要支持空 key，必须同时修改 `hash`、Go 对照实现和注册 API 的名称约束，并添加空 key 回归测试；仅在 Rust 侧选择默认分片会造成移植语义分叉。
- 若要改变散列算法或 `SHARD`，需评估热点名称的锁竞争、已有 key 的增删定位一致性，并同步 Go 实现。当前映射没有在线重分片机制，运行中改变算法不能迁移已经存在的条目。
- 若要让遍历可提前停止或返回错误，应调整 `Iter` 的回调/返回类型，并同步 `ResourceManager::schedule` 调用点与独立测试。若目标是缩短持锁时间，可考虑在锁内克隆 `Arc` 列表、锁外回调，但这会改变删除后的对象可见生命周期和内存开销，必须明确验证。
- 若要对齐 Go 的 `intest.InTest` 覆盖行为，应先决定 Rust 测试模式的权威开关，并分别测试生产重复注册拒绝与测试模式覆盖；不能简单删除重复检查。
- 若加入显式清理，不能只在 `Del` 中无条件调用 `ReleaseAndWait`，因为值可能仍被其他所有者引用且锁内阻塞会放大竞争。应由拥有池生命周期的上层定义释放时机，并增加并发与阻塞边界测试。

## 验证依据

- RustCodeGraph `status`：索引包含 `pkg/resourcemanager/util` 的 Rust/Go 文件；`files --filter pkg/resourcemanager/util` 显示目标、模块入口、Go 对照和两端测试均在图中。
- RustCodeGraph `query ShardPoolMap --json`、`query NewShardPoolMap --json`、`query PoolMapError --json`：确认公开结构、构造函数、错误类型及 Go 对照符号的位置；目标文件节点列出 14 个符号。
- RustCodeGraph `node --file pkg/resourcemanager/util/shard_pool_map.rs`：核实 `SHARD`、`hash`、公开 API、私有 `PoolMap`、锁范围和错误文本；`node NewShardPoolMap` 进一步确认其下游为 `std::array::from_fn` 与 `PoolMap::new`。
- RustCodeGraph 文件节点：读取并核对 `pkg/resourcemanager/rm.rs`、`pkg/resourcemanager/schedule.rs`、`pkg/resourcemanager/lib.rs`、`pkg/resourcemanager/util/lib.rs` 与 `pkg/resourcemanager/util/util.rs`，确认装配方式、上游注册/注销/调度调用和 `PoolContainer` 数据定义。
- Go 与测试证据：`pkg/resourcemanager/util/shard_pool_map.go`、`pkg/resourcemanager/util/shard_pool_map_test.go`、`pkg/resourcemanager/util/shard_pool_map_test.rs`，用于核对重复键、删除幂等、计数遍历、锁粒度与 Go/Rust 差异。
- Cargo 证据：`pkg/resourcemanager/util/Cargo.toml`、`pkg/resourcemanager/Cargo.toml` 和工作区根 `Cargo.toml`，用于核对独立 facade crate 与生产 crate 的双重装配边界。
- 本任务是只读代码分析与文档新增，按计划不运行 Cargo；交付前使用任务规定的命令验证本文恰有 11 个固定二级章节。
