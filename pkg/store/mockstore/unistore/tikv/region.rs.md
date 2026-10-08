# `pkg/store/mockstore/unistore/tikv/region.rs`

## 文件定位

本文件是 unistore 内嵌 mock TiKV crate 的 Region 运行时与管理接口实现，源码见 [`region.rs`](./region.rs)。crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod region` 暴露它；[`Cargo.toml`](./Cargo.toml) 将该 crate 定义为 `astersql-store-mockstore-unistore-tikv`，并记录其 Go 对照包为 `pkg/store/mockstore/unistore/tikv`。当前 crate 只直接声明 `fail` 依赖，Region、Peer、Store、Epoch 和 Region 错误类型来自同 crate 的 [`mock_region.rs`](./mock_region.rs)，并非直接使用 kvproto 类型。

它位于 mock TiKV RPC 服务的 Region 边界上：[`server.rs`](./server.rs) 把 `Arc<dyn RegionManager>` 注入 `Server`，每次请求先通过 `get_region_from_context` 校验 Store、Region ID 与 Epoch，写路径再借助返回的 `RegionContext` 获取键 latch。它不负责 MVCC 数据读写，也不是多节点 Raft/PD 的真实实现。

## 核心职责

1. 定义内部元数据键：`INTERNAL_KEY_PREFIX`、`INTERNAL_REGION_META_PREFIX`、`INTERNAL_STORE_META_KEY`、`INTERNAL_SAFE_POINT_KEY`，并由 `internal_region_meta_key` 生成十进制 Region ID 后缀。
2. 通过 `Latches` 为相同键指纹提供进程内互斥；256 个槽以哈希高 8 位分片，降低无关键竞争同一互斥锁的概率。
3. 通过 `RegionContext` 聚合 Region 元数据、原始键范围、近似大小/增量和共享 latch，并提供 Peer 变更时的 `conf_ver` 更新。
4. 通过 `RegionManager` trait 隔离 Server 与具体 Region 管理实现，统一请求上下文校验、Store 地址映射、Region 分裂和关闭入口。
5. 通过 `StandAloneRegionManager` 提供单 Store、纯内存的实现：维护 Region 表、分配本地 ID、按键定位 Region、按采样大小触发分裂。
6. 通过 `router_region` 把运行时上下文投影成路由元数据，约定第一个 Peer 为 leader，且 buckets/down peers 为空。

## 主要符号

- `internal_region_meta_key(region_id: u64) -> Vec<u8>`：拼接 `b"\xffregion"` 与 Region ID 的十进制文本。该格式与 Go `InternalRegionMetaKey` 一致，但当前 Rust 管理器没有持久化这些键。
- `LatchWaiter`：一个 acquisition ticket，`released: Mutex<bool>` 保存释放状态，`Condvar` 唤醒等待同一 ticket 的线程。
- `Latches::{acquire, release}`：对一组完整 `u64` 哈希加锁/解锁。`acquire` 返回发生等待的次数；`release` 删除映射后仅对该组键共享的 ticket 做一次 `notify_all`。
- `RegionContext`：`meta` 使用 `RwLock<Region>`；`approximate_size` 与 `diff` 使用 `AtomicI64`；`raw_start/raw_end` 构造后不再改变；`latches` 是跨 RegionContext 共享的 `Arc<Latches>`。
- `RegionContext::new`：复制元数据边界；空 `end_key` 转为 `INTERNAL_KEY_PREFIX`，使无界用户键范围在扫描/比较时停在内部元数据空间之前。
- `RegionContext::add_peer`：追加 Peer 并将 Epoch 的 `conf_ver` 加一，不改变 `version`。
- `RegionOptions`：包含 Store 地址、PD 地址和目标 Region 大小。当前 `StandAloneRegionManager::new` 只读取 `region_size`，另外两个字段尚未接线。
- `RequestContext`：RPC 侧的最小路由上下文；`store_id`、`epoch` 可缺省，`region_id` 必填。
- `RegionManager`：供 `Server` 使用的对象安全 trait；要求实现 `Send + Sync`。
- `StandAloneRegionManager`：持有 `regions`、单个 `store`、共享 `latches`、本地 `next_id`、`closed` 标记与 `region_size`。
- `split_check_region`：对调用方提供的 `(key, size)` 样本求总大小；小于阈值时只更新近似大小，达到阈值时选择累计大小首次达到一半的键并调用 `split_region`。
- `split_region`：排序、去重并过滤切分键，创建连续子区间；最右子 Region 保留旧 ID 和 `conf_ver`，仅把 `version` 加一，其他子 Region 使用新 ID 和 `(1, 1)` Epoch；所有子 Region 复制旧 Peer 列表。
- `router_region`：生成 `mock_region::RegionCtx`，leader 取 `peers.first()`，因此空 Peer 列表产生 `None`，不会 panic。

## 执行流程

初始化时，`StandAloneRegionManager::new` 创建一份共享 `Latches`，把 root Region 包装成 `RegionContext` 放入 `regions`，并将 `next_id` 初始化为 root Region ID、Store ID、所有 Peer ID 的最大值。后续 `alloc_ids(count)` 用一次原子加法预留连续区间，返回严格递增且大于初始最大 ID 的 ID。

RPC 请求进入 [`server.rs`](./server.rs) 后，`Server::request_region` 调用 `RegionManager::get_region_from_context`：先校验可选 `store_id`，再按 `region_id` 查询内存表，最后对可选 Epoch 的 `conf_ver/version` 做精确相等校验。错误分别映射为 `StoreNotMatch`、`RegionNotFound(id)` 和携带当前 Region 的 `EpochNotMatch`。读取请求只需此定位步骤；`Server::with_latches` 的写请求还会调用 [`util.rs`](./util.rs) 的 `keys_to_hash_values`，把键转换为已排序且去重的哈希，再按“获取 latch—执行 MVCC 操作—释放 latch”的顺序运行。

显式分裂从 `split_region(region_id, keys)` 开始。输入键先按字节序排序、去重，再剔除 `<= old.start_key` 或 `>= old.end_key`（非空上界）的键；没有有效键则返回 `InvalidSplitKeys`。实现用“旧起点 + 有效切分键 + 旧终点”构造连续半开区间，先完整创建结果，再在 `regions` 写锁下删除旧项并插入新项。返回顺序与键序一致。

大小驱动分裂从 `split_check_region` 开始。它信任调用方传入的样本顺序，累加 size；总量小于 `region_size` 时更新 `approximate_size` 并返回 `Ok(None)`。否则选取累计值首次达到 `total / 2` 的样本键，缺少样本则返回 `InvalidSplitKeys`，找到后委托 `split_region`。当前源码中没有生产调用者，因此它是可用但尚未接入后台扫描器的入口。

关闭时 `close` 仅以 Release 顺序把 `closed` 原子标志设为 true；当前没有读取该标志的 API，也没有线程、通道或持久资源需要等待。

## 数据与状态

- Region 键区间遵循 `[start_key, end_key)`；空 `end_key` 表示用户键空间无界。`region_for_key` 和 `split_region` 都保持这一约定。
- `RegionContext::raw_start/raw_end` 是构造时快照，而 `meta` 可通过 `add_peer` 修改。当前可变操作只改 Peer/Epoch，不改边界，因此快照仍一致；若以后允许在线改边界，必须同步重建上下文或调整字段设计。
- `approximate_size` 和 `diff` 可无锁读写。`split_check_region` 在“不分裂”分支更新 `approximate_size`，但成功分裂时新上下文的两个计数器都从 0 开始；这与 Go 版为左右 Region 保存估算大小不同。
- 所有 `RegionContext` 共用管理器的同一个 `Arc<Latches>`，因此即使 Region 分裂，相同哈希仍在全管理器范围互斥。
- `regions` 是以 Region ID 为键的 `HashMap`。`region_for_key` 遍历 values，复杂度为 O(Region 数)，若区间意外重叠则返回项受 HashMap 遍历顺序影响；正确性依赖区间互斥不变量。
- `next_id` 使用 `AtomicU64`，`alloc_ids(0)` 返回空集合；整数接近 `u64::MAX` 时源码没有显式溢出处理。
- `closed` 只记录关闭动作，不阻止关闭后的查询、分裂或 ID 分配。

## 依赖与调用关系

上游直接调用以 [`server.rs`](./server.rs) 为主：`Server::new` 接收 `Arc<dyn RegionManager>`；`request_region` 调用 `get_region_from_context`；地址查询方法转发到 Store 映射接口；`stop` 调用 `close`；`with_latches` 使用 `RegionContext::{acquire_latches, release_latches}`。[`main_test.rs`](./main_test.rs) 和 [`server_test.rs`](./server_test.rs) 构造 `StandAloneRegionManager` 并注入 Server。

下游依赖集中在标准库同步原语与 [`mock_region.rs`](./mock_region.rs) 的数据模型：`Arc` 负责共享所有权，`Mutex/Condvar` 实现 latch 等待，`RwLock` 保护 Region/Store 表，原子类型管理计数、ID 与关闭标记；`Peer`、`Region`、`RegionEpoch`、`RegionError`、`Store` 定义传输语义。`router_region` 返回的 `RegionMetadata` 也来自 `mock_region.rs`。

RustCodeGraph 对目标文件记录了 51 个符号，并确认 `server.rs` 使用 `RegionContext/RegionManager/RequestContext`。精确符号查询能定位 `region.rs::StandAloneRegionManager`、`region.rs::RegionManager::split_region` 和 `region.rs::router_region`；但当前索引没有给该 impl 的精确 callers/callees 输出，因此直接调用点用 `rg` 复核。仓库范围搜索没有发现 `router_region`、`split_check_region`、`region_for_key` 等入口在目标文件外的生产调用。

## 错误处理与边界

- 可恢复的业务错误使用 `RegionError` 返回：Store 不匹配、Region 不存在、Epoch 不匹配、没有有效切分键。
- 缺省 `store_id` 或 `epoch` 表示跳过对应校验；这便于 mock 请求，但调用者若需要真实路由一致性必须显式携带它们。
- Epoch 校验要求两个字段都完全相等，任一字段不同均返回当前 Region 元数据，供客户端刷新路由。独立测试 `epoch_must_match_exactly_like_go` 覆盖该合同。
- 切分键等于边界、落在区间外或重复都会被过滤；若过滤后为空则不修改 Region 表。
- `split_check_region` 未拒绝负 size 或未排序样本；负值会影响总量和中点选择，因此调用方应提供按键顺序排列的非负大小样本。
- 所有标准库锁都以 `expect("... poisoned")` 处理 poisoning；持锁线程 panic 后，后续访问会继续 panic，而不是返回 `RegionError`。
- `Latches::acquire` 自身不排序/去重。重复哈希会等待同一个 acquisition 自己释放而死锁，不同线程以不同顺序获取多个哈希也可能形成循环等待；生产调用链依赖 `keys_to_hash_values` 的排序去重保证。新增直接调用者必须维持同一前置条件。
- `Server::with_latches` 没有 RAII guard；若传入的 operation panic，`release_latches` 不会执行。正常 `Result` 错误路径会释放。
- `router_region` 对空 peers 安全返回无 leader；Go 版心跳代码直接索引首个 Peer 的位置则要求 Peer 非空，两者使用场景不同。

## 并发与资源生命周期

`StandAloneRegionManager` 满足 `Send + Sync`：Region 表与 Store 使用 `RwLock`，高频尺寸数据和 ID/关闭状态使用原子类型。`get_region_from_context` 在取得 Store 读锁并校验后显式释放，再取得 Region 表读锁，避免同时持有两把管理器级锁。`split_region` 先在读锁下克隆旧元数据，离锁构建结果，最后一次持有写锁替换映射，缩短临界区。

Latch 的生命周期以一次 `acquire(hashes)` 创建的共享 ticket 为单位。同一请求的各哈希都指向该 ticket；竞争者克隆前任 `Arc`，在条件变量循环中处理伪唤醒。释放方先从各槽删除哈希映射，再把 ticket 标为 released 并广播，等待者醒来后重新竞争槽。Release/Acquire 原子顺序分别用于近似大小、增量、ID 和关闭标记的跨线程可见性。

分裂替换 Region 表时，已取得的 `Arc<RegionContext>` 仍可存活并继续操作旧上下文；实现没有 generation barrier。调用者通过 Epoch 校验和重新请求来转向新 Region。管理器没有后台线程、PD 客户端、数据库句柄或析构逻辑；`close` 因而不是 Go 版那种“关闭通道并等待 worker”的资源屏障。

## 与 Go 版本的对应关系

同路径 [`region.go`](./region.go) 是语义对照来源。Rust 保留了以下核心合同：内部键前缀格式；256 槽 latch 与完整哈希排队；空结束键避开内部元数据空间；Store/Region/Epoch 校验；Peer 变更增加 `conf_ver`；分裂后右侧保留旧 Region ID 且 version 加一、左侧使用新 ID 和 `(1,1)` Epoch并继承 peers。Rust 测试 `split_keeps_old_id_on_right_and_preserves_peers_like_go` 直接固定了最后一项。

两者并非功能等价实现。Go `regionCtx` 会对 Region 边界做 codec 解码，持久化/反序列化近似大小与元数据；Rust 当前直接复制 `Region.start_key/end_key`，因此上游模型必须已经使用它期望的原始键表示。Go `StandAloneRegionManager` 从 Badger 加载 Store/Region，向 PD bootstrap/注册/汇报心跳，由 PD 分配 ID，运行 Store heartbeat 与定时 split worker，并把分裂结果写回 Badger；Rust 只维护内存 HashMap、本地原子 ID 和布尔关闭标志。

Go 的采样器最多保留 64 个逐步稀疏的样本，并在约三分之二大小处切分；Rust `split_check_region` 接收外部完整样本并在约二分之一处切分。Go 的公开 `SplitRegion` RPC 当前也是空响应，但内部 `splitRegion` 是持久化/PD 汇报链的一部分；Rust trait 的 `split_region` 则直接提供可工作的内存多键切分。这些差异应视为明确的迁移范围，而不是已完成的等价移植。

## 扩展指南

- 新增请求侧 Region 校验时，优先扩展 `RequestContext` 和 `RegionManager::get_region_from_context`，并同步 [`region_test.rs`](./region_test.rs) 的错误分支；保持 `Server` 只依赖 trait。
- 新增边界或 Epoch 变更操作时，必须同时维护 `RegionContext.meta` 与不可变 `raw_start/raw_end` 的一致性，并覆盖旧 `Arc` 存活、客户端持有旧 Epoch 的行为。
- 接入后台大小检查时，应明确样本排序、size 非负、空样本和阈值为零/负数的合同；若追求 Go 等价，需要恢复 64 项采样、三分之二切点、近似大小迁移与持久化，而不能仅启动定时器调用现有简化方法。
- 扩展 latch 时宜引入 RAII guard，确保 panic/提前返回也能释放；任何直接调用 `acquire` 的入口都必须先排序去重。测试逻辑应继续放在独立 [`region_test.rs`](./region_test.rs) 或调用侧独立测试文件中，不要内嵌进生产源文件。
- 支持多 Store/PD 时，需要重新定义 Store 映射、ID 分配、leader 选择、心跳和关闭生命周期；`RegionOptions::store_address/pd_address` 当前只是占位字段，不能作为已接线依据。
- 若 Region 数量增大，应把 `region_for_key` 的 HashMap 全扫描替换为有序范围索引，并用测试固定半开区间、空上界、相邻边界和不重叠不变量。
- 修改分裂必须保留排序去重、区间过滤、右侧复用旧 ID/Epoch 递增及 peers 继承合同；相关兼容风险集中在路由刷新和旧上下文并存，性能风险集中在持有 Region 表写锁的替换阶段。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件，目标目录被索引；`node --file pkg/store/mockstore/unistore/tikv/region.rs --offset 1 --limit 500` 读取了 425 行全文件；`query` 精确定位了 `StandAloneRegionManager`、`RegionManager::split_region`、`router_region`。自然语言 `explore` 因 `Region` 同名符号较多产生噪声，精确 callers/callees 对 impl 无输出，故调用边另以仓库搜索核验。
- 生产源码：[`region.rs`](./region.rs)（全部常量、类型、trait、impl 与函数）；[`server.rs`](./server.rs)（`Server::new`、`stop`、`request_region`、`with_latches`）；[`util.rs`](./util.rs)（`sort_and_dedup_hash_values`、`keys_to_hash_values`）；[`mock_region.rs`](./mock_region.rs)（共享数据模型与错误）；[`lib.rs`](./lib.rs)（模块装配）。目标包没有 `doc.go`。
- crate/config：[`Cargo.toml`](./Cargo.toml) 的 package 名、`lib.rs` 入口、Go 包迁移元数据、平台依赖与 `fail` 依赖。
- Go 对照：[`region.go`](./region.go) 的 `RegionCtx`/`regionCtx`、`latches`、`regionManager`、`StandAloneRegionManager`、`runSplitWorker`、`splitCheckRegion`、`splitRegion`、`Close`。
- 独立测试：[`region_test.rs`](./region_test.rs) 覆盖精确 Epoch 校验以及分裂 ID/Epoch/Peer 合同；[`main_test.rs`](./main_test.rs) 覆盖 Server 注入、关闭幂等、Store 与 Epoch 校验；[`server_test.rs`](./server_test.rs) 证明管理器被实际 Server 夹具使用。同路径不存在 `region_test.go`，Go 行为证据来自生产实现和目录内其他 Go 测试对该管理器的使用搜索。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证：目标文件存在，且固定的十一个二级标题各出现一次。
