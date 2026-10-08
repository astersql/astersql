# `pkg/store/mockstore/unistore/pd.rs`

## 文件定位

本文件位于 `astersql-store-mockstore-unistore` crate 的 PD（Placement Driver）模拟层。`pkg/store/mockstore/unistore/lib.rs` 通过 `pub mod pd` 声明模块并以 `pub use pd::*` 再导出公开符号；`pkg/store/mockstore/unistore/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/store/mockstore/unistore`，并直接依赖提供 URL 规范化的 `astersql-util` 及再导出为 `crate::tikv` 的 UniStore TiKV 子 crate。

在嵌入式 UniStore 启动链中，`pkg/store/mockstore/unistore/mock.rs::New` 从 `server::new_mock` 获得 `Arc<MockPd>`，再调用 `PdClient::new`，最终把 `Arc<PdClient>` 与 RPC client、Cluster 一起返回。因此本文件不是独立 PD 服务，也不进行网络通信；它是在进程内为测试环境提供 TSO、PD 成员/服务发现、全局配置、外部时间戳和 Keyspace 元数据的状态门面。最近目录没有 `doc.go`，本说明以 Rust 源码、模块入口、同路径 Go 文件和独立测试为事实依据。

## 核心职责

- `PdClient` 把底层 `tikv::mock_region::MockPd` 的 TSO 能力与本文件拥有的配置、地址、外部时间戳和 Keyspace 状态组合成单一进程内客户端。
- 全局配置路径把调用方给出的短名称统一映射为 `/global/config/<name>`，支持内存读写，并用一个有界通道模拟 Watch 快照推送。
- PD 地址在构造阶段经 `astersql_util::service_url::NormalizeServiceURL` 过滤、补全默认 `http` scheme；同一规范化地址列表同时驱动成员列表和服务发现，首个成员被视为 leader。
- 外部时间戳更新遵守两个约束：不得超过调用时取得的全局 TSO，且不得低于当前外部时间戳；并发更新通过原子 CAS 循环完成。
- `MockKeyspaceManager` 校验初始 ID/名称唯一性和 ID 上限，以 ID 有序存储元数据，并提供按名称、按 ID、分页列举及状态更新。
- `MockTsFuture` 模拟只能 `wait` 一次的异步 TSO 结果；`MockPdServiceDiscovery` 和 `MockPdServiceClient` 则提供无需真实连接、恒定可用的发现视图。

## 主要符号

- `MAX_KEYSPACE_ID`、`NULL_KEYSPACE_ID`、`LOGICAL_BITS`：分别表示协议允许的最大 Keyspace ID、空 Keyspace 哨兵和 TSO 逻辑位宽。`LOGICAL_BITS` 仅用于把 `(physical, logical)` 合成为比较上限。
- `EventType`、`GlobalConfigItem`：全局配置 Watch/读取结果的本地数据模型。当前事件类型只有 `None` 和 `Put`。
- `KeyspaceState`、`KeyspaceMeta`：Keyspace 的启用/禁用/归档状态及其 ID、名称元数据。
- `PdError` 与 `Result<T>`：本模块的字符串错误包装和统一返回类型；`Display` 原样输出内部消息。
- `PdMember`、`PdMembersResponse`：由注入地址合成的成员列表和可选 leader，不是从真实 PD 拉取的协议响应。
- `PdClient`：核心门面。字段 `pd` 指向共享 `MockPd`；`keyspaces` 管理 Keyspace；`global_config` 用读写锁保护配置表；`external_timestamp` 是原子值；`addresses` 保存规范化端点；`current_keyspace_id` 保存创建时绑定的 Keyspace。
- `PdClient::new`：可失败构造函数，失败只来自 `MockKeyspaceManager::new`；`newPDClient` 是 Go 风格兼容工厂，会对相同错误 `expect` 并返回 `Arc<PdClient>`。生产启动路径当前直接使用前者。
- `PdClient::{load_global_config,store_global_config,watch_global_config}`：配置读、写、监听入口；`get_ts`/`get_local_ts`/`get_ts_async` 提供同步、本地 DC 兼容和一次性异步 TSO。
- `PdClient::{set_external_timestamp,external_timestamp}`：外部时间戳的单调更新与读取；`current_keyspace_id` 返回构造参数。
- `PdClient::{load_keyspace,load_keyspace_by_id,all_keyspaces,update_keyspace_state}`：把 Keyspace 操作委托给内部 manager。
- `MockTsFuture::wait`：通过 `AtomicBool::swap` 保证每个 future 只消费一次。
- `MockPdServiceClient`、`MockPdServiceDiscovery`、`NewMockPDServiceDiscovery`：端点及发现视图；client 恒定报告可用、无需重试、已连接 leader，discovery 返回首个或全部 client，`remove_client_conn` 是空操作。
- `is_url`/`valid_url`/`normalize_mock_pd_addrs`：共享 URL 校验与规范化逻辑，非法端点被静默过滤。
- `MockKeyspaceManager`、`newMockKeyspaceManager`：分别是实际 manager 与供测试/Go 对照使用的兼容构造函数；内部 `BTreeMap<u32, KeyspaceMeta>` 保证按 ID 排序，`HashMap<String,u32>` 提供名称索引。

## 执行流程

1. `mock.rs::New` 创建 server、Region manager 和底层 `MockPd`，随后调用 `PdClient::new(mock_pd, pd_addresses, current_keyspace_id, cluster_keyspaces)`。构造器先建立 Keyspace manager；若任一 ID 超限或 ID/名称重复则整条 `New` 链返回 `NewError::Pd`，否则初始化空配置表、零外部时间戳并规范化地址。
2. TSO 请求由 `PdClient::get_ts` 直接调用 `MockPd::get_ts`；`get_local_ts` 忽略 DC 名称并走相同路径。`get_ts_async` 保存 `Arc<PdClient>` 和初始为 `false` 的 `used`，第一次 `wait` 获取当时的 TSO，第二次立即报错。
3. 配置写入时，`store_global_config` 在写锁内给每个名称添加 `/global/config/` 前缀并覆盖旧值。读取时，`load_global_config` 在读锁内按输入顺序构造结果：命中项携带 `Put`，缺失项保留空值和默认 `None`，revision 固定返回 `0`。
4. `watch_global_config` 在创建通道前克隆当前配置快照，随后启动线程，最多执行 10 轮；每轮逐项发送单元素事件向量。接收端被丢弃或通道发送失败时线程提前退出。创建 Watch 之后的配置更新不会进入该快照。
5. `set_external_timestamp` 先取得一次全局 TSO 并按 `(physical << 18) + logical` 合成上限。新值超过上限直接失败；否则循环读取当前原子值，拒绝回退、接受幂等相等值，并以 `compare_exchange` 重试并发竞争直至写入成功。
6. 服务发现与成员列表都使用构造时已经规范化的 `addresses`。`get_all_members` 按注入顺序从 1 分配 member ID，并把第一项复制为 leader；空地址产生空成员和 `None` leader。`service_discovery` 再构造对应 client 列表。
7. Keyspace 构造时逐项检查 ID 上限、重复 ID 和重复名称，再同时写入有序表和名称索引。名称查询先从名称索引取 ID、再从有序表取元数据；ID 查询直接查表。`all(start_id, limit)` 使用包含 `start_id` 的范围并按 ID 升序返回，`limit == 0` 表示不限量；状态更新在写锁内原地修改并返回副本。

## 数据与状态

`PdClient` 的状态分为共享底层状态和自身状态。`Arc<MockPd>` 由 server 创建并与客户端共享，TSO 的生成规则由 `tikv/mock_region.rs::MockPd` 负责；本文件只读取它。配置表、Keyspace 表与名称索引属于当前 `PdClient` 实例，不做持久化，也不与其他客户端自动同步。`current_keyspace_id` 在构造后不可变。

全局配置的键总以 `/global/config/` 保存；`store_global_config` 不读取 `path`，因此若调用方把已经带前缀的名称传入，会再次添加前缀。Watch 克隆的是调用瞬间的 `HashMap`，其遍历顺序没有稳定保证；每个配置项会被重复发送至多 10 次，而不是仅在变更时推送。

Keyspace 的 `BTreeMap` 与名称 `HashMap` 在成功构造后应满足一一对应。公开操作只允许改变 `KeyspaceMeta.state`，不会修改 ID 或名称，因此不会破坏索引关系。`keyspaces()` 与 `keyspace_names_map()` 返回副本，调用方无法借此修改内部状态。

外部时间戳用 `AtomicU64` 保存，初始为零；读取使用 `Acquire`，成功更新使用 `AcqRel`。成员与发现地址是 `Vec<String>`，保持合法输入的相对顺序；非法地址被过滤，不保留错误详情。

## 依赖与调用关系

生产上游调用边为 `pkg/store/mockstore/unistore/mock.rs::New → PdClient::new → MockKeyspaceManager::new`；`lib.rs` 的 glob 再导出还允许 crate 使用方直接访问公开模型和兼容工厂。`pd_test.rs::set_up_suite` 及其他测试通过 `New` 获得客户端，再调用配置、成员、发现和 Keyspace API。

主要下游边包括 `PdClient::get_ts → tikv::mock_region::MockPd::get_ts`、`set_external_timestamp → get_ts`、`service_discovery/NewMockPDServiceDiscovery → normalize_mock_pd_addrs → astersql_util::service_url::NormalizeServiceURL`，以及各 `PdClient` Keyspace 方法到 `MockKeyspaceManager` 同名语义方法的委托。标准库承担 `RwLock`、原子操作、有界 `sync_channel` 和后台线程。

RustCodeGraph 的目标文件节点报告该文件被 11 个文件引用，但精确 `callers`/`callees` 查询未在本次运行中返回可用边；因此上述调用边以目标文件节点、`mock.rs` 启动代码和仓库源码搜索交叉确认。仓库搜索没有找到 `newPDClient` 的 Rust 调用者，不能据此断言外部用户不存在；它仍是 crate 根公开 API。

## 错误处理与边界

`MockKeyspaceManager::new` 对 `id > MAX_KEYSPACE_ID`、重复 ID 和重复名称返回 `PdError`；`NULL_KEYSPACE_ID == u32::MAX` 自然落入超限检查。按名称或 ID 查询不存在项返回文本 `ENTRY_NOT_FOUND`；名称索引命中但有序表缺项会返回 `keyspace list and name map mismatch`，这是内部不变量破坏的诊断。状态更新不存在 ID 也返回 `ENTRY_NOT_FOUND`。

`PdClient::new` 保留构造错误，`newPDClient` 则 panic；二者适用于不同兼容入口。配置和 Keyspace 的 `RwLock` 一旦中毒均通过 `expect` panic，不转换为 `PdError`。地址规范化失败不会让构造失败，只会丢弃该地址；若全部地址无效，成员列表和 discovery client 都为空。

外部时间戳的上限只基于函数开始时取得的一次 TSO；它保证不会写入大于该快照的值，并以 CAS 保证自身单调，但不提供与后续 TSO 读取组成事务的能力。`physical as u64` 假定 mock TSO 物理部分非负，这与当前 `MockPd` 的时间来源相符。`MockTsFuture::wait` 第二次调用返回 `cannot wait tso twice`，不会再次访问底层 PD。

Watch 的 `sync_channel(16)` 会在接收方消费过慢时阻塞发送线程；接收端断开会正常结束线程。`close`、`remove_client_conn` 都是空操作，不代表真实网络连接或后台任务已关闭。

## 并发与资源生命周期

`PdClient` 通常置于 `Arc` 中并被 RPC/测试代码共享。全局配置使用单个 `RwLock<HashMap<...>>`；一次 load 持有读锁覆盖整批名称，一次 store 持有写锁覆盖整批写入。Keyspace 的有序表和名称索引使用两个独立 `RwLock`，但构造后名称索引不再修改，运行期状态更新只锁有序表；名称查询先释放名称读锁，再获取元数据读锁。

外部时间戳不使用互斥锁。CAS 循环使并发较大值更新不会被较小值覆盖：竞争失败者重新读取当前值，并根据新的单调性条件成功重试或返回错误。`MockTsFuture` 的 `used` 也是原子量，因此多个线程同时等待同一 future 时至多一个成功。

每次 `watch_global_config` 都创建一个未命名、未 join 的短生命周期线程。线程拥有配置快照和 sender；发完最多 10 轮或发现 receiver 已断开后自然退出。`PdClient::close` 不向这些线程发送停止信号；提前停止依赖丢弃对应 receiver。若配置为空，线程不发送数据并很快结束。

service discovery 与成员结果都是地址和 client 的克隆快照，不拥有 socket、gRPC connection 或重试任务。Keyspace/配置等内存状态在最后一个 `PdClient` 所有者释放时销毁；底层 `MockPd` 在其所有 `Arc` 强引用释放后销毁。

## 与 Go 版本的对应关系

同路径 `pkg/store/mockstore/unistore/pd.go` 是直接对照。Rust 保留了 Go `pdClient` 的核心数据：嵌入/持有 `MockPD`、Keyspace manager、全局配置、原子外部时间戳、规范化地址和当前 Keyspace ID；配置前缀、固定 revision、Watch 最多十轮、local TSO 等同 global TSO、一次性 future、首成员为 leader、外部时间戳的 TSO 上限与单调 CAS、Keyspace 排序/唯一性/分页规则也相互对应。

Rust 用 `Arc`、`RwLock` 和原子量显式提供并发保护；Go 的 `globalConfig` 普通 map 没有锁，`mockTSFuture.used` 也是普通布尔值。Rust Watch 在启动前克隆快照，Go goroutine 直接遍历 client map，因此两者面对并发修改时并不完全等价。两边都忽略配置 `path`/revision，且 discovery 不维护真实连接。

接口范围并非一比一。Go `pdClient` 为满足完整 `pd.Client` 还包含大量返回空值、空操作或 panic 的方法；Rust `PdClient` 是具体门面，没有实现该完整 trait，也没有移植这些无行为接口。反之，Rust `MockKeyspaceManager::update_state` 已实际修改状态，而当前 Go `UpdateKeyspaceState` 仍为 `panic("unimplemented")`。文档不把未出现在 Rust 类型上的 Go 接口视为已支持。

地址规范化都委托同仓库 utility 语义；Go 测试包含 `unix://`，Rust `pd_test.rs::service_discovery_normalizes_injected_unix_and_http_addresses` 同样验证裸 host、HTTPS、Unix 地址和非法字符串。Keyspace 的 Go 实现用排序 slice 与二分搜索，Rust 用 `BTreeMap`，对外仍保持包含 `start_id`、按 ID 升序且 `limit == 0` 不限量的结果。

## 扩展指南

- 新增 PD 能力前先确定它应由底层 `MockPd` 维护还是由 `PdClient` 自身维护：TSO/Region 元数据应优先落在 `tikv/mock_region.rs`，纯客户端状态才放在本文件。同步扩展独立 `pd_test.rs`，不要把测试嵌入生产文件。
- 修改全局配置时需明确键是否已经带 `/global/config/`、Watch 是快照还是增量、revision 是否有意义，以及事件类型/删除事件的规则。若改为持续 Watch，必须设计取消信号、关闭通道、线程 join 和慢消费者背压，避免泄漏后台任务。
- 扩展 Keyspace 时保持 ID/名称索引一致、ID 上限和有序分页契约。若允许改名或删除，必须在一次一致性边界内同步两个索引，并增加并发查询、重复名称、分页端点和状态转换的回归测试。
- 调整外部时间戳时应保留“不得超过全局 TSO、不得回退、相等幂等”的三项契约，并为并发竞争补专门测试。改变 TSO 位宽必须与 PD/oracle 协议同步，不能只修改 `LOGICAL_BITS`。
- 扩展服务发现时注意当前 member ID 仅按地址顺序临时生成，首项固定为 leader，client 没有连接对象。若引入动态 leader、连接缓存或重试，需要同步定义线程安全、连接回收和 `close/remove_client_conn` 的真实语义，并覆盖空地址与非法地址。
- 若目标是兼容完整 Go `pd.Client`，应逐项记录真正被上层使用的方法，不应为了“接口齐全”批量加入返回成功的桩；每个新增行为都需对应 Go 差异和独立测试。

## 验证依据

- RustCodeGraph：`status` 确认索引可用（7,032 个 Rust 文件、307,296 个节点）；`files --filter pkg/store/mockstore/unistore/pd.rs` 确认目标文件被索引且含 72 个符号；`node --file ... --offset 1 --limit 500` 与 `--offset 498 --limit 80` 读取完整 534 行；`query MockPD`、`query PdClient --kind struct --json` 用于消除同名符号歧义。`explore` 及精确 `callers`/`callees` 本次没有返回可用调用边，故以源码搜索补证。
- crate 与启动边界：`pkg/store/mockstore/unistore/Cargo.toml`、`pkg/store/mockstore/unistore/lib.rs`、`pkg/store/mockstore/unistore/mock.rs`、`pkg/store/mockstore/unistore/tikv/mock_region.rs`。
- Go 对照：`pkg/store/mockstore/unistore/pd.go`，并以 `pkg/store/mockstore/unistore/pd_test.go` 核对配置、服务发现、成员和 Keyspace 语义。
- Rust 独立测试：`pkg/store/mockstore/unistore/pd_test.rs` 覆盖配置读写/Watch、URL 规范化、成员 leader、Keyspace 排序分页、重复/非法 ID 与按 ID 查询；仓库搜索显示外部时间戳的直接测试目前位于底层 `tikv/mock_region_test.rs`，没有覆盖本文件 `PdClient::set_external_timestamp`，`MockTsFuture` 和 `update_keyspace_state` 也未检索到直接测试。
- 本任务只新增说明文档，按约束未运行 Cargo。交付以任务指定的 11 个固定二级标题结构检查和人工事实复核为验证门槛。
