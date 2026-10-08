# `pkg/store/mockstore/unistore/tikv/mock_region.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-store-mockstore-unistore-tikv`，由同目录 `lib.rs` 以 `pub mod mock_region` 暴露。它是进程内 mock TiKV/PD 的元数据控制面：保存 Store、Peer、Region 路由和纪元，提供 Region 分裂与扫描，同时实现 Mock PD 所需的 ID/TSO、外部时间戳和按 keyspace 隔离的 GC safe point 状态。真正的 KV/MVCC 数据不存放在这里；`server/server.rs::new_mock` 创建本管理器并把它经 `MockRegionAdapter` 接到请求路径，数据引擎则由 server 侧的 `EngineBundle` 持有。

同路径 `Cargo.toml` 指定 `lib.rs` 为 crate 根，并用 `package.metadata.porting.go-package` 指向 `pkg/store/mockstore/unistore/tikv`。当前依赖表主要是 Windows 目标下的相邻 AsterSQL crate 和通用 `fail` 依赖；本文件自身只使用标准库集合、同步原语和系统时间。

## 核心职责

1. `MockRegionManager` 用一份加锁的 `RegionState` 维护 Region ID 表、有序 start-key 索引、Store 表和 MPP 任务占位表，并用原子计数器分配全局 ID。
2. `bootstrap` 建立首个 Store/Region；`get_region_by_key`、`get_region_by_end_key`、`scan_regions` 提供 PD 风格的路由查询；`validate_context` 在请求进入 Region 执行上下文前检查 Store、Region 和客户端纪元。
3. `split`、`split_keys` 和 `calculate_split_keys` 分别实现单点分裂、多点分裂和按候选数据键近似均匀选择分裂点。
4. `MockPd` 给上层提供 Region 查询、Store 枚举、ID/TSO 与外部时间戳门面。
5. `GcStatesManager` 模拟 PD 的 GC 状态 API：维护 txn/gc safe point，使用最小 GC barrier 限制 txn safe point，并阻止 safe point 回退或 GC 超越事务水位。

当前实现是测试用的内存模型，不是完整 PD：Region/Store 状态不会写入 Badger，`close` 只记录原子标志而没有影响后续操作，`register_mpp_task` 只保存 `String`，没有 Go 版 `MPPTaskHandler` 的行为。

## 主要符号

- `StoreLabel`、`Store`、`Peer`、`RegionEpoch`、`Buckets`、`Region`：简化的 TiKV 元数据值类型。`Region` 的键范围遵循 `[start_key, end_key)`，空 `end_key` 表示正无穷。
- `RegionCtx`：把 `Region` 与可选 leader、bucket 信息、down peers 组合为一次路由查询的快照。
- `RegionError`：覆盖未引导、重复引导、Store 不匹配、Region 不存在、纪元过期和分裂键错误。当前代码不会产生 `AlreadyBootstrapped`，因为 `bootstrap` 对重复调用直接返回成功。
- `RegionState`：`regions: HashMap<u64, RegionCtx>` 是主表，`by_start: BTreeMap<Vec<u8>, u64>` 是有序路由索引；另含 `stores`、`primary_store` 和按 Store 分组的 `mpp_tasks`。
- `MockRegionManager`：`state: RwLock<RegionState>` 保护复合元数据；`id`、`closed` 分别是原子 ID 水位和关闭标志；`cluster_id`、`region_size` 是只读配置。
- `MockRegionManager::{bootstrap, validate_context, split, split_keys, calculate_split_keys, scan_regions}`：Region 生命周期和请求校验的主要入口。
- `contains`：半开区间包含判断，是按键路由的最终边界守卫。
- `MockPd`、`ExternalTimestampError`、`get_ts`：PD 门面、外部时间戳的两类校验错误，以及进程全局的单调 `(physical_ms, logical)` 时间戳生成器。
- `GcBarrier`、`GcState`、`AdvanceResult`、`GcError`：GC 状态 API 的输入输出模型与错误集合。
- `GcStatesManager`：用 `Mutex<HashMap<u32, InternalGcState>>` 隔离各 keyspace 的 txn/gc safe point 和 barrier 表。

## 执行流程

服务初始化从 `server/server.rs::new_mock` 开始：调用本文件 `get_ts` 生成引擎状态时间戳，构造 `MockRegionManager`，连续分配 Store/Region/Peer 三个 ID，通过 `bootstrap` 写入首个 Region，随后创建 `MockPd` 和 `MockRegionAdapter`。`bootstrap` 在写锁内检查是否已有 Region，登记 Store 与 MPP 表，抬高 ID 水位，把 Region 纪元重置为 `(conf_ver=1, version=1)`，选择第一个 Peer 为 leader，并同时写入 `regions` 与 `by_start`。

请求校验由 `server/server.rs::MockRegionAdapter::get_region_from_context` 调用 `validate_context`：若请求携带 Store ID，先确认 Store 存在；再按 Region ID 取上下文；最后只要客户端的 `conf_ver` 或 `version` 小于服务端值，就返回携带当前 Region 的 `EpochNotMatch`。校验成功后，adapter 把元数据转换并缓存为执行侧 `RegionContext`。

按键查询时，`get_region_by_key` 先在 `by_start` 中取 `start_key <= key` 的最右候选，再用 `contains` 检查半开区间；若索引候选未命中，会向后扫描以容忍索引中的空洞。`get_region_by_end_key` 使用严格的 `start_key < key <= end_key` 语义实现“前一个 Region”。`scan_regions` 则筛选所有与 `[start,end)` 相交的 Region，按 start key 排序，并在 `limit > 0` 时截断。

单点 `split` 要求分裂键严格位于原 Region 内且 `peer_ids.len()` 与原 Peer 数相同。左半保留原 ID、Peer 和 conf_ver，end key 改为分裂键并把 version 加一；右半使用新 Region/Peer ID、继承各 Peer 的 store_id，纪元从 `(1,1)` 开始，leader 为首个新 Peer。两半写回 `regions`，右半 start key 加入 `by_start`。`split_keys` 先排序去重，从左到右重新定位每个键，跳过恰好等于 Region start key 的键，为其余分裂分配 Region/Peer ID 并逐次调用 `split`。`calculate_split_keys` 先过滤 `[start,end)` 内候选，再按商和余数分组，余数优先分给前面的组，并选择每组之后的首键作为边界。

`get_ts` 通过进程级 `OnceLock<Mutex<(i64,i64)>>` 串行化时间戳生成：物理毫秒前进时逻辑值归零，同毫秒调用或系统时钟回拨时逻辑值递增。`MockPd::set_external_timestamp` 先拒绝大于当前打包 TSO（物理值左移 18 位再或逻辑值）的时间戳，再用 CAS 循环保证外部时间戳不下降；相同值可成功写回。

GC 流程以 keyspace 为隔离单元。`set_barrier` 拒绝空 ID、零时间戳和低于当前 txn safe point 的 barrier。`advance_txn_safe_point` 拒绝目标回退，然后寻找最小 barrier；若它低于目标，就把新水位截在该 barrier，且绝不让截断结果低于旧水位。`advance_gc_safe_point` 拒绝回退，也拒绝超过 txn safe point。`state` 可选择是否把 barrier 快照返回给调用者。

## 数据与状态

Region 主表与 `by_start` 必须同步：每个可路由 Region 都应由 ID 找到，并由其 start key 找到相同 ID。分裂不会删除原 start-key 项，因为左半仍沿用原 Region ID 和 start key；它只为右半增加新索引。所有对外查询都克隆值，调用者不能绕过锁直接修改内部元数据。

`id` 是 Store、Region 和 Peer 共用的单调水位。`bootstrap`、`add_store` 和 `split` 会用 `fetch_max` 吸收外部指定 ID，`alloc_id(s)` 使用原子加法生成后续 ID。该计数器只保证当前进程内不重复，不提供持久化或溢出处理。

`RegionEpoch.conf_ver` 在 `add_peer` 时递增，`version` 在左半 Region 分裂时递增；新右半从 `(1,1)` 开始。`validate_context` 接受相等或领先于服务端的客户端纪元，只拒绝任一分量落后，这与本文件 Go 对照的 stale 判断方向一致，但不是“必须完全相等”的校验。

每个 `InternalGcState` 独立记录 `txn_safe_point`、`gc_safe_point` 和 `HashMap<String,u64>` barriers。状态按需由 `entry(keyspace).or_default()` 创建，因此只读 `state` 和删除不存在 barrier 也会在内部表留下默认 keyspace 项。barrier 返回顺序来自 `HashMap`，没有稳定排序承诺。

## 依赖与调用关系

上游调用关系包括：

- `pkg/store/mockstore/unistore/server/server.rs::new_mock` 构造、引导并共享 `MockRegionManager`/`MockPd`；`MockRegionAdapter` 调用 `validate_context`、Store 查询和 `split_keys`，并在分裂后失效 RegionContext 缓存。
- `pkg/store/mockstore/unistore/cluster.rs::Cluster` 把编码后的用户键交给 `split`，并通过 `calculate_split_keys`/`split_keys` 实现近似均匀分裂；引导辅助函数使用 `alloc_id(s)` 和 `bootstrap`。
- `pkg/store/mockstore/unistore/pd.rs::MockPdClient` 把 TSO 请求转发到 `MockPd::get_ts`；该文件还独立实现了上层 PD client 的外部时间戳状态，因此不能把两个原子字段视为同一个共享存储。
- 同 crate 的 `region.rs`、`server.rs`、`server_batch.rs` 复用本文件的 Region 元数据和 `RegionError` 作为路由/响应模型。

下游仅是标准库：`HashMap`/`BTreeMap` 提供主表和有序索引，`RwLock`/`Mutex` 保护复合状态，`AtomicU64`/`AtomicBool` 提供计数器和标志，`SystemTime` 提供物理毫秒。没有文件 I/O、网络 I/O 或外部 crate 调用。

RustCodeGraph 的 `query MockRegionManager --kind struct` 与 `query GcStatesManager --kind struct` 将符号定位到本文件，并同时找到 Go 对照；索引内直接调用点再由上述相邻 Rust 文件核验。此次 `explore` 和 `node --file` 没有返回正文，因此源码细节与调用点使用直接文件读取和 `rg` 补足。

## 错误处理与边界

Region API 以 `RegionError` 返回可预期业务错误：未知 Store 为 `StoreNotMatch`，未知 Region 为 `RegionNotFound(id)`，客户端纪元落后为 `EpochNotMatch(current_region)`，分裂键碰到/越过任一边界为 `SplitKeyOutOfRange`，Peer 数不匹配或多键分裂找不到 Region 为 `InvalidSplitKeys`。`bootstrap` 在 Store 列表为空时返回 `NotBootstrapped`，但重复引导幂等成功。

键范围均按字节序比较。空 end key 表示无上界；start key 可以为空。`calculate_split_keys(count <= 1)` 和 `split_keys(empty)` 返回空列表。候选键数量少于分组数时算法仍会终止，但可能产生少于 `count - 1` 个分裂点。

外部时间戳错误区分“超过当前全局 TSO”和“试图下降”。`SystemTime` 早于 Unix epoch 时 `get_ts` 使用默认零时长而不报错；锁中毒则通过 `expect` panic。Region/GC 的全部标准锁也采用相同的中毒即 panic 策略。

GC API 拒绝非法 barrier、safe point 回退及 GC 超越 txn 水位。删除不存在的 barrier 返回 `None`。它没有 Go 版本的 TTL、全局 barrier、全部 keyspace 枚举及错误码/日志包装；调用者不能依赖这些未移植行为。

## 并发与资源生命周期

`MockRegionManager.state` 的读操作共享 `RwLock` 读锁，bootstrap、分裂和 Store/Peer/MPP 修改持有写锁，因此每个方法内部的复合状态变更是原子的。`split_keys` 不在整个批次持有同一写锁：它逐键查询、分配 ID、再调用 `split`，并发修改可能在步骤间改变目标 Region；当前 mock 的典型测试用法是串行配置拓扑，若扩展到并发拓扑变更，需要增加批次级互斥或重试协议。

ID 和 external timestamp 使用 Acquire/Release 或 AcqRel 原子顺序。external timestamp 的 CAS 循环在线程竞争时重新读取并再次校验，保证成功序列单调不降。`get_ts` 通过全局互斥锁保证所有 `MockPd` 实例共享时间水位，避免同一毫秒返回重复的 `(physical, logical)` 对。

GC 的全部 keyspace 共用一把 `Mutex`，单次 barrier/safe-point 操作原子化，但不同 keyspace 也相互串行。所有状态随 `MockRegionManager`/`MockPd` 进程内对象销毁而消失，没有后台任务、通道或显式清理。`MockRegionManager::close` 当前只把 `closed` 设为 true；没有读取该标志的路径，所以它不阻止查询、分裂或后续状态变更。

## 与 Go 版本的对应关系

直接对照 `pkg/store/mockstore/unistore/tikv/mock_region.go`：Rust 的 `MockRegionManager`、按键/末键查询、bootstrap、ID 分配、分裂、Store/Peer 操作、`MockPD`、外部时间戳、全局 TSO 和 GC 状态机，都有同名或等价的 Go 来源。半开区间判断、分裂时左半 version 加一/右半纪元重置、候选键的商余分配、外部时间戳 CAS、最小 barrier 阻塞 txn safe point，以及 GC 不得越过 txn safe point等关键规则保持一致。

Rust 版本有以下明确差异和迁移边界：

- Go 构造器从 `mvcc.DBBundle` 加载 Region，并在 bootstrap/分裂时把元数据写入 Badger；Rust 本文件只有内存表，重启不恢复。
- Go 通过 protobuf 元数据、gRPC PD 接口、B-tree 和具体 `MPPTaskHandler` 工作；Rust 用本地值类型、`BTreeMap` 和字符串占位，API 面更窄。
- Go `Split`/`SplitRaw` 区分编码键与原始键；Rust 本文件 `split` 只接收已经选定的存储键，用户键编码由 `cluster.rs` 上游完成。
- Go 的 `calculateSplitKeys` 自行扫描 Badger 有序键；Rust `calculate_split_keys` 接收调用方提供的候选切片且不排序。`cluster.rs::split_keys` 会先排序数据键后再执行同一商余算法，因此直接调用本文件方法时，调用者必须保证输入已按存储键顺序排列。
- Go GC barrier 校验 TTL，并公开更多兼容接口；Rust 只实现永久 barrier 所需的核心状态转换，未表示 TTL 与创建时间。
- Go `saveRegions` 在 `closed` 后跳过持久化；Rust 没有持久化路径，因此 `closed` 当前是未消费状态。

## 扩展指南

新增 Region 路由或拓扑变更时，应优先修改 `MockRegionManager`，并保持 `regions` 与 `by_start` 的双索引不变量、ID 水位、纪元递增规则和 `[start,end)` 边界一致。若变更会替换/删除 Region，必须同时更新旧 start-key 索引；若上层缓存了 `RegionContext`，还要同步检查 `server/server.rs::MockRegionAdapter` 的缓存失效逻辑。

扩展分裂行为时，需要明确输入是用户原始键还是编码后的存储键，并在 `cluster.rs` 边界完成编码；不要在本文件内悄然双重编码。`calculate_split_keys` 若允许无序输入，应显式排序并补充性能评估，而不是依赖测试数据碰巧有序。

新增 GC 能力时，应在 `InternalGcState`/`GcStatesManager` 内集中维护不变量。实现 TTL 需要引入可测试时钟、过期清理语义和返回模型；实现全局 barrier 或跨 keyspace 查询时，必须定义锁粒度和确定性排序。新增 PD API 还应检查 `pd.rs` 客户端门面是否需要转发。

测试逻辑必须继续放在独立文件：分裂点与 external timestamp 回归扩展 `mock_region_test.rs`；GC barrier/safe-point 扩展 `mock_pd_test.rs`；请求上下文和纪元兼容性可扩展 `region_test.rs` 或 server 侧独立测试。需要同步对照 Go 的 `mock_region.go`、`mock_pd_test.go`，但不要把 Go 的持久化/完整接口能力当成 Rust 当前事实。

兼容性风险主要是键编码、纪元比较方向、原 Region ID 落在哪一半、错误映射和 GC 单调性；性能风险主要是 `scan_regions` 对 HashMap 全表扫描、`split_keys` 的逐键加锁以及所有 keyspace 共用一把 GC mutex。

## 验证依据

- 源码全量读取：`pkg/store/mockstore/unistore/tikv/mock_region.rs`，覆盖其中 82 个索引符号及所有 783 行；主要事实落在 `MockRegionManager`、`MockPd`、`get_ts`、`GcStatesManager` 及其方法。
- crate 与模块边界：`pkg/store/mockstore/unistore/tikv/Cargo.toml`、`pkg/store/mockstore/unistore/tikv/lib.rs`。
- RustCodeGraph：`status` 显示目标目录已索引；`files --filter pkg/store/mockstore/unistore/tikv` 列出目标 Rust/Go/测试文件；`query MockRegionManager --kind struct`、`query GcStatesManager --kind struct` 同时定位 Rust 与 Go 对照。`explore`/`node --file` 本次无正文输出，调用点改用 `rg` 和直接读取核验。
- Rust 上游：`pkg/store/mockstore/unistore/cluster.rs`、`pkg/store/mockstore/unistore/server/server.rs`、`pkg/store/mockstore/unistore/pd.rs`；同 crate 类型使用处还包括 `region.rs`、`server.rs`、`server_batch.rs`。
- Go 对照：`pkg/store/mockstore/unistore/tikv/mock_region.go`，重点核对构造/持久化、路由、bootstrap、分裂、外部时间戳、TSO 和 GC 状态机。
- 独立测试：`pkg/store/mockstore/unistore/tikv/mock_region_test.rs` 验证商余分裂点和 external timestamp；`mock_pd_test.rs` 验证三个 keyspace、safe-point 单调性、最小 barrier 与错误；`region_test.rs` 提供纪元/分裂兼容证据。Go 的 `mock_pd_test.go` 提供 GC API 原始语义对照。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前另以指定 shell 命令验证目标文档存在且恰有十一个固定二级标题，并人工复核没有把 Go 独有能力写成 Rust 已支持。
