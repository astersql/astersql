# `pkg/store/mockstore/unistore/cluster.rs`

## 文件定位

本文件位于 `astersql-store-mockstore-unistore` crate 的集群控制层。crate 入口 `pkg/store/mockstore/unistore/lib.rs` 以 `pub mod cluster` 声明本模块并通过 `pub use cluster::*` 再导出其公开符号；`pkg/store/mockstore/unistore/Cargo.toml` 则把该 crate 映射到 Go 包 `pkg/store/mockstore/unistore`，并依赖实际定义 Region 类型及管理器的 `astersql-store-mockstore-unistore-tikv` 子 crate。

在完整的嵌入式 UniStore 启动链中，`pkg/store/mockstore/unistore/mock.rs::New` 从 `server::new_mock` 取得共享的 `MockRegionManager`，构造 `Arc<Cluster>`，再把同一个 `Cluster` 交给 `RPCClient` 并返回给测试调用方。因此本文件不是数据读写引擎：它是 Region/Store 元数据管理器的薄控制门面，同时承载用于事务并发测试的一次性 RPC 延迟表。最近目录没有 `doc.go`；本说明以模块入口、Rust 实现、同路径 Go 实现和独立测试为事实来源。

## 核心职责

- `Cluster` 持有共享的 `MockRegionManager`，把引导、单点分裂和多点分裂等拓扑操作委托给它；真实 Region 状态、Store 表、leader 选择及 ID 水位均由 `pkg/store/mockstore/unistore/tikv/mock_region.rs::MockRegionManager` 维护。
- `schedule_delay` 与 `handle_delay` 以 `(start_ts, region_id)` 为键注册、消费一次性 `Duration`。`pkg/store/mockstore/unistore/rpc.rs::RPCClient::dispatch` 在 Prewrite、PessimisticLock 和 Flush 请求进入 server 前调用 `handle_delay`，从而为测试提供确定性的请求时序控制。
- `split_raw`、`split` 和 `split_keys` 将用户/原始键转成 TiDB memcomparable 字节编码后交给 Region 管理器，维持 Region 边界使用编码键的约定。
- 三个 `BootstrapWith*` 辅助函数构造单 Store、单 Region/多 Store、单 Store/多 Region 的初始拓扑，供 mock 环境和测试快速建立可预测的 ID 与 Region 布局。
- `encode_bytes` 在本模块内复刻 TiDB `codec.EncodeBytes` 的 8 字节分组编码，避免把未编码用户键直接写入 Region 边界。

## 主要符号

- `DelayKey { start_ts, region_id }`：私有、可哈希的延迟键。两个字段共同定位“某事务在某 Region 上”的一次性事件，避免同一事务跨 Region 时互相干扰。
- `pub struct Cluster`：包含 `Arc<MockRegionManager>` 和 `Mutex<HashMap<DelayKey, Duration>>`。前者支持与 server、PD、RPC 门面共享拓扑，后者只保护延迟事件表。
- `Cluster::new`、`Cluster::region_manager`：分别构造包装器和克隆内部 `Arc`；克隆不会复制管理器状态。
- `Cluster::schedule_delay`、`Cluster::handle_delay`：公开注册延迟、在 crate 内 RPC 路径消费延迟。锁中毒时二者都会通过 `expect("delay-event lock poisoned")` panic。
- `Cluster::split_raw` 与 `Cluster::split`：都先调用 `encode_bytes`，再调用 `MockRegionManager::split`。两者当前行为相同；参数 `_leader_peer_id` 为兼容 Go/上层接口而保留，但 Rust 实现不读取它。
- `Cluster::split_keys(start, end, count, data_keys)`：从调用方提供的数据键快照中筛选 `[start, end)`、排序并按商/余分配选出切分点，编码后交给 `MockRegionManager::split_keys`；返回实际新建的右侧 Region。
- `Cluster::close`：进程内 mock 的空操作，不负责关闭 manager、server 或删除目录。
- `newCluster`：返回 `Arc<Cluster>` 的兼容工厂。仓库内 Rust 启动路径当前直接使用 `Cluster::new`；该符号没有检索到 Rust 调用者。
- `BootstrapWithSingleStore`：分配 store、region、peer 三个 ID，创建地址为 `store{id}` 的单 Store 与 epoch `(conf_ver=1, version=1)` 的单 Region。
- `BootstrapWithMultiStores`：为一个 Region 创建 `count` 个 Store/Peer，并把第一个 peer ID 作为返回的 leader ID；`count == 0` 时返回 `RegionError::NotBootstrapped`。
- `BootstrapWithMultiRegions`：先执行单 Store 引导，再预分配额外 Region/Peer ID，按给定键从左向右逐次调用 `Cluster::split`。
- `encode_bytes`：每 8 字节数据后附加 `0xff - padding` 标记；输入长度正好为 8 的倍数（包括空输入）时，再追加一组八个零和标记 `0xf7`，以保持前缀可比较编码。

## 执行流程

1. `mock.rs::New` 调用 `server::new_mock` 创建 server、`Arc<MockRegionManager>` 和 mock PD，然后以该 manager 创建 `Arc<Cluster>`；`RPCClient::new` 保存同一个 Cluster，调用方也获得其克隆。
2. 引导单 Store 时，`BootstrapWithSingleStore` 依次调用 `alloc_id` 得到 store、region、peer ID，组装元数据后调用 `MockRegionManager::bootstrap`。多 Store 版本批量分配连续的 store/peer ID，并把相同顺序的 Store 与 Peer 配对；多 Region 版本先完成单 Store 引导，再逐个分裂当前最右 Region。
3. 单点分裂时，`split_raw`/`split` 对调用方的裸键执行一次 `encode_bytes`。`MockRegionManager::split` 验证原 Region、键范围及 peer 数量，缩短左 Region、递增其 version，创建右 Region并更新起始键索引。
4. 均匀分裂时，`split_keys` 在裸 `data_keys` 中保留 `key >= start` 且 `end` 为空或 `key < end` 的键，并排序以模拟 Go Badger 有序迭代器。设 `q = len / count`、`r = len % count`，前 `r` 个分段各取 `q + 1` 个条目，其余取 `q` 个；每个仍位于数据范围内的累计边界成为切分点。切分点编码后由 manager 排序、去重并逐点分裂。
5. 延迟路径中，测试或外部调用方先调用 `schedule_delay` 覆盖对应键的时长。RPC 分派 Prewrite、PessimisticLock 或 Flush 时调用 `handle_delay`；函数在互斥锁内 `remove` 事件，释放锁后才 `thread::sleep`，所以相同键只命中一次，休眠也不会阻塞其他键的注册或消费。

## 数据与状态

`Cluster` 自身只拥有延迟表，Region 元数据通过 `Arc<MockRegionManager>` 共享。`region_manager()` 返回新的 `Arc` 强引用，调用方看到的是同一套 `RwLock<RegionState>` 和 `AtomicU64` ID 生成器，而不是快照。Region 初始范围依赖 `Region::default()`（引导辅助函数没有显式设置 start/end），初始 epoch 在辅助函数和 manager 的 `bootstrap` 中均设为 `1/1`。

延迟表允许同一 `DelayKey` 最多保存一个时长：重复 `schedule_delay` 使用 `HashMap::insert` 覆盖旧值；`handle_delay` 用 `remove` 原子地取得所有权，未命中时立即返回。它没有清理定时器、过期时间或后台线程，未被对应 RPC 消费的事件会保留到 `Cluster` 被释放。

`split_keys` 的 `start`、`end` 和 `data_keys` 都是未编码用户键；传给 manager 的切分点才是编码键。`count <= 1`、范围内没有数据键，或数据量不足以形成内部边界时返回空列表。该函数选择的是现有数据键作为新 Region 的起始边界，不复制或移动数据；数据归属随 manager 中的 Region 元数据边界改变。

## 依赖与调用关系

上游主链是 `mock.rs::New → Cluster::new → RPCClient::new`。运行期直接调用边为 `rpc.rs::RPCClient::dispatch → Cluster::handle_delay`，涵盖 Prewrite、PessimisticLock、Flush；`DebugGetRegionProperties` 也经 `Cluster::region_manager` 查询 Region。`cluster_test.rs` 和 `main_test.rs` 直接构造 Cluster、引导并验证分裂行为。

下游调用集中在 `tikv/mock_region.rs`：构造与查询使用 `MockRegionManager`，引导使用 `alloc_id`/`alloc_ids`/`bootstrap`，分裂使用 `split`/`split_keys`。`MockRegionManager::split` 再负责持有写锁、验证边界、建立左右 Region 与 leader；因此修改本文件的分裂参数会直接影响 manager 的不变量。标准库依赖仅有 `HashMap`、`Arc`、`Mutex`、线程休眠与 `Duration`。

`lib.rs` 的 glob 再导出使这些公开函数可通过 crate 根访问。仓库搜索确认本 crate 的 Rust 测试使用 `BootstrapWithSingleStore`、`split_raw`、`split`、`split_keys` 和 `encode_bytes`；未找到 `newCluster`、`schedule_delay`、多 Store/多 Region辅助函数在当前 Rust 仓库内的直接调用，不能据此推断它们未被外部 crate/API 使用。

## 错误处理与边界

拓扑操作返回 `RegionError`，错误源来自 manager：找不到 Region 为 `RegionNotFound`，切分键不在原 Region 开区间为 `SplitKeyOutOfRange`，新 peer 数量与原 Region 副本数不一致或无法定位切分目标为 `InvalidSplitKeys`。三个引导函数使用 `?` 原样传播错误；Rust 与 Go 的明显差别是 Rust 不在 bootstrap 失败时 panic。`BootstrapWithMultiStores` 还显式拒绝零 Store，避免对 `peer_ids[0]` 越界。

互斥锁或 manager 的内部锁若中毒会 panic，这是当前 mock 的故障策略，不会转换为 `RegionError`。`thread::sleep` 不可取消且不返回错误；超长延迟会占用当前 RPC 工作线程。`close` 无条件成功且不修改状态，实际资源释放由 `RPCClient::close`/server 及 `Arc` 生命周期承担。

必须注意 `_leader_peer_id` 当前未生效：leader 由 manager 从新 Region 的第一个 peer 推导。因此调用方传入非首 peer 的 leader ID 不会改变结果。另一个兼容边界是编码：传给 `split`/`split_raw` 的必须是裸键；若传入已编码键会再次编码并产生错误边界。

## 并发与资源生命周期

`Cluster` 可通过 `Arc` 在 RPC client、PD/测试控制代码之间共享。Region manager 内部用 `RwLock` 保护拓扑、用原子变量分配 ID；本文件不额外包住 manager 操作，因此一次 manager 方法调用的原子性由 manager 自身保证，而 `BootstrapWithMultiRegions` 的多次 split 不是整体事务，中途错误可能留下已经完成的前缀分裂。

延迟表的 `Mutex` 只覆盖 map 的 insert/remove。`handle_delay` 在睡眠前释放锁，这一顺序很重要：并行 RPC 可继续操作其他事件；两个线程竞争同一键时只有先 remove 的线程休眠。若第一个线程 remove 后尚未休眠，另一个线程可重新注册同一键，新事件将供后续一次调用消费。

本文件不创建长期线程；唯一阻塞是调用线程上的 `thread::sleep`。`Cluster::close` 不回收任何资源，`Cluster` 最后一个 `Arc` 释放时延迟表销毁，manager 则在其所有共享引用释放后销毁。新增后台任务或可取消延迟时，必须明确停止信号、join 顺序与 client/server 关闭的先后关系。

## 与 Go 版本的对应关系

同路径 `cluster.go` 是直接对照。`DelayKey`/`Cluster` 字段、一次性延迟的“锁内删除、锁外 sleep”、单/多 Store 与多 Region 引导的 ID 分配顺序均保持一致；Rust 用 `Arc<MockRegionManager>` 代替 Go 的匿名嵌入指针，用 snake_case 方法配合 crate 根再导出，兼容工厂和 bootstrap 函数仍保留 Go 风格命名。

存在三项应显式记录的差异。第一，Go `SplitRaw` 调用 manager 的 `SplitRaw`，Rust `split_raw` 和 `split` 都直接编码一次再调用 `MockRegionManager::split`，且忽略 `leader_peer_id`。第二，Go `Cluster.SplitKeys(start,end,count)` 由 manager/Badger 自行扫描数据；Rust 为避免隐藏存储访问，把候选 `data_keys` 作为额外参数传入，在本文件中排序、按同样商余算法选点后再调用 manager。`cluster_test.rs::split_keys_uses_storage_key_order_like_go` 专门验证无序输入仍按存储键序选择 `c`。第三，Go bootstrap 遇错 panic 且多 Store 假定 `n > 0`；Rust 返回 `Result` 并显式处理 `count == 0`。

`main_test.rs::test_cluster_split_raw_encodes_user_key` 验证 Rust 的 `split_raw` 与 `split` 都只编码一次并写入正确右 Region 边界。Go 目录没有 cluster 专属测试文件；相关 Go 行为主要由 `cluster.go`、`tikv/mock_region.go` 及上层 mockstore/coprocessor 测试间接覆盖。

## 扩展指南

- 新增拓扑操作时，优先在 `MockRegionManager` 中实现状态不变量，在 `Cluster` 中只处理用户键编码或接口适配；同步增加独立的 `cluster_test.rs` 或同目录其他 `*_test.rs`，不要把测试内嵌进生产文件。
- 修改分裂逻辑时，同时核对裸键/编码键边界、peer 数量、leader 选择、epoch 递增与 ID 水位；尤其不要让 `split_raw`/`split` 对同一输入重复编码。若开始支持 `leader_peer_id`，应在 manager API 和 Go 对照语义中一起落实，并增加非首 peer leader 的回归测试。
- 扩展 `split_keys` 时，应保持 `[start,end)`、稳定排序、商余分配以及“只在数据键处切分”的契约。建议补充 `count <= 1`、空范围、重复键、端点键、数据少于 count、manager 部分失败等独立用例。
- 增加延迟适用的 RPC 类型时，在 `RPCClient::dispatch` 的 server 调用之前接入，并用独立 RPC 测试证明键匹配、只消费一次和不同 Region 不互扰。若需要非阻塞/可取消等待，不应在持锁区等待，并需设计关闭时唤醒机制。
- 改变 bootstrap 返回值或错误策略会影响 crate 根公开 API；应同步核对 `lib.rs` 再导出、`mock.rs::New`、同路径 Go 实现和外部 mockstore 适配层。性能风险主要来自大 `data_keys` 快照的克隆与排序，以及同步 sleep 占用 RPC 线程。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore` 确认目标、模块及对照文件；`node --file pkg/store/mockstore/unistore/cluster.rs --offset 1 --limit 500` 读取目标全部 287 行并报告 21 个符号；`query BootstrapWithSingleStore/BootstrapWithMultiStores/BootstrapWithMultiRegions` 消除了 Go/Rust 同名符号歧义。精确 `explore`、`callers`、`callees` 未返回可用内容，故调用边用下列源码搜索补证。
- 源码与 crate 边界：`pkg/store/mockstore/unistore/cluster.rs`、`pkg/store/mockstore/unistore/lib.rs`、`pkg/store/mockstore/unistore/Cargo.toml`、`pkg/store/mockstore/unistore/mock.rs`、`pkg/store/mockstore/unistore/rpc.rs`、`pkg/store/mockstore/unistore/tikv/mock_region.rs`。
- Go 对照：`pkg/store/mockstore/unistore/cluster.go`、`pkg/store/mockstore/unistore/mock.go`、`pkg/store/mockstore/unistore/rpc.go`、`pkg/store/mockstore/unistore/tikv/mock_region.go`。
- 独立测试：`pkg/store/mockstore/unistore/cluster_test.rs` 验证无序候选键的排序/均匀切分；`pkg/store/mockstore/unistore/main_test.rs` 验证 `split_raw` 与 `split` 的编码边界。仓库 `rg` 还确认 `rpc.rs` 在三类请求中调用 `handle_delay`。
- 本任务是纯文档分析，未运行 Cargo。交付前按任务指定命令检查目标文件存在且固定二级标题恰好为 11 个，并人工复核没有把未找到的调用者或理想设计写成已实现事实。
