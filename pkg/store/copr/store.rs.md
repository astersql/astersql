# `pkg/store/copr/store.rs` 逻辑说明

## 文件定位

`store.rs` 是 `astersql-store-copr` crate 的组装与适配层。crate 入口 `pkg/store/copr/lib.rs` 以 `pub mod store` 声明本模块，并通过 `pub use store::*` 导出其公开 API；`pkg/store/copr/Cargo.toml` 将 crate 根设为 `lib.rs`，没有为本文件设置独立 feature 或条件编译项。本文件本身也没有 `#[cfg(...)]` 分支。

它不实现具体的 PD、TiKV 或 TiFlash 网络协议，而是用 `StoreBackend` 抽象这些能力，将一个后端和一份共享 `RegionCache` 组装成三类客户端表面：通用 RPC 的 `TikvClient`、Coprocessor 的 `CopClient`、MPP 的 `MppClient`。生产实现来自 `pkg/store/copr/network_backend.rs` 的 `impl StoreBackend for NetworkBackend`；生产接线位于 `pkg/store/driver/tikv_driver.rs`，其中先建立 `NetworkBackend`，再调用 `astersql_store_copr::Store::new`。

## 核心职责

本文件承担四项职责。

1. 用 `StoreBackend` 定义组装层所需的最小传输与可见性接口，包括 Region 元数据入口、普通/batch Coprocessor RPC、锁解析、MPP 分发与连接以及资源关闭。
2. 用 `StoreCopBackend` 和 `StoreMppTransport` 把同一个后端分别适配为 `CopBackend` 与 `MppTransport`，保证 Cop、MPP 和 `KvStore` 共享后端与 Region 路由状态。
3. 用 `Store` 管理 Coprocessor 缓存、副本读种子、CPU 并行度、TiFlash 部署模式和事件监听器所有权，并提供客户端工厂。
4. 保留与 Go API 对应的兼容入口：`kvStore`、`tikvClient` 类型别名以及 `NewStore` 构造函数。

其中 `StoreCopBackend::send_tiflash_batch` 还有一段具体协议装配逻辑：按 Region 切分 ANN/TiFlash ranges，构造 `kvproto::coprocessor::BatchRequest`，序列化后交给 `StoreBackend::send_request`。因此本文件不仅是薄门面，也是 TiFlash batch 请求进入通用传输层的桥接点。

## 主要符号

- `EndpointType`：公开端点分类，取值为 `TiKv`、`TiFlash`、`TiFlashCompute`、`TiDb`，默认值是 `TiKv`。`endpoint_type` 根据 `StoreType` 和 `disaggregated_tiflash` 参数完成映射。
- `ClientEventListener`：要求 `Send + Sync + 'static` 的事件回调 trait，唯一方法是 `on_event(&str)`。
- `StoreBackend`：公开依赖反转边界。必选方法覆盖 Region 后端、客户端/地址关闭、batch 请求、事件监听、Cop 请求、锁解析、可见性检查和 MPP 操作；`send_coprocessor_stream` 有一个明确返回“不支持”的默认实现，具体后端可覆盖。
- `TikvClient`：持有 `Arc<dyn StoreBackend>` 的公开包装。固有方法负责关闭、流式 Cop 请求、异步请求和监听器设置；同时实现 `RpcClient` 与 `MppAliveClient`。
- `StoreCopBackend`：私有 Cop 适配器。它组合 `StoreBackend` 和 `RegionCache`，实现 `CopBackend` 所需的 range 切分、batch 任务构造、发送、缓存失效、bucket 更新、锁处理和可见性检查。
- `StoreMppTransport`：私有 MPP 适配器，把 `dispatch`、`cancel`、`establish`、可见性检查、store 枚举和缓存失效委托给后端。
- `KvStore`：公开底层句柄，保存共享后端与共享 `RegionCache`，提供 `region_cache`、`check_visibility` 和 `client`。
- `Store`：公开聚合对象。重要状态为 `kv_store`、互斥保护的可选 `coprocessor_cache`、原子 `replica_read_seed`、`cpu_count`、两个 TiFlash 模式开关、原子 `closed` 和监听器列表。
- `Store::new` / `NewStore`：前者返回 `Store`，后者是返回 `Box<Store>` 的 Go 风格包装；两者都可能因 `CoprocessorCache::new` 失败而返回 `BatchResult` 错误。
- `kvStore` / `tikvClient`：仅为命名兼容保留的公开类型别名，不引入额外状态或行为。

## 执行流程

生产初始化链为 `pkg/store/driver/tikv_driver.rs` 的驱动打开流程 → `NetworkBackend::connect` → 将后端擦除为 `Arc<dyn StoreBackend>` → `Store::new`。`Store::new` 从后端取得 `RegionCacheBackend` 并创建唯一的共享 `RegionCache`，用当前 Unix 时间的纳秒部分初始化副本读种子，按配置创建可选 Coprocessor 缓存，读取 `thread::available_parallelism()` 作为 Cop 客户端并行度，最后记录 TiFlash 模式开关。

Coprocessor 路径从 `Store::get_client` 开始。方法创建 `StoreCopBackend`，使其共享相同后端和 RegionCache；随后递增副本读种子、克隆当前缓存句柄，并构造 `CopClient`。真正执行时，`CopClient` 经 `CopBackend` 调用本适配器：`split_key_ranges` 根据 `skip_buckets` 决定仅按 Region 切分，还是进一步按 bucket 切分；结果被转换成包含 Region、边界、bucket 版本、store 地址/ID 和 peer 的 `LocatedKeyRanges`。请求发送、锁解析和可见性检查再委托给后端。

TiFlash batch 路径位于 `StoreCopBackend::send_tiflash_batch`。它遍历请求中的每个 range 分区，调用 `RegionCache::split_key_ranges_by_locations`；每个 location 经 `BatchTaskSource::rpc_context` 获得目标地址，再装配一个 Region 的 protobuf 请求。读超时为零时使用 60 秒，否则使用请求给出的超时。响应包含 `retry_regions` 时立即返回传输错误，因为当前 ANN scan 路径不在此处重建 Region 任务；否则收集所有 batch 响应。

MPP 路径从 `Store::get_mpp_client` 开始。共享的 `RegionCache` 同时作为 `BatchTaskSource`，`StoreMppTransport` 负责传输，两个构造参数将存算分离与自动扩缩容策略交给 `MppClient`。MPP 的分发、取消、建连、可见性检查和缓存失效均继续委托到同一后端。

通用请求可通过 `KvStore::client` 获得 `TikvClient`。同步 `RpcClient::send_request` 原样委托；`send_request_async` 每次新建一个 OS 线程，在新线程中用独立的默认 `CancellationToken` 执行同步请求，完成后调用一次性回调。

## 数据与状态

`Arc` 是本文件的共享所有权基础：后端、RegionCache、缓存和监听器都可跨客户端共享。`KvStore` 保证由它派生的 `TikvClient`、`CopClient` 和 `MppClient` 指向同一后端；`Store::kv_store` 和 `KvStore::region_cache` 返回克隆后的 `Arc`，不会复制底层状态。

`coprocessor_cache` 使用 `Mutex<Option<Arc<CoprocessorCache>>>`，原因是关闭时需要原子地取走所有权，而客户端构造时需要安全克隆当前缓存。关闭之后 `get_client` 仍能构造客户端，但拿到的缓存为 `None`；本文件没有用 `closed` 拒绝该操作。

`replica_read_seed` 用 `AtomicU32` 维护，`next_replica_read_seed` 以 `AcqRel` 的 `fetch_add` 先增后返回；`u32` 溢出遵循原子整数运算的回绕语义。当前 `get_client` 会推进种子，但将结果绑定到 `_seed`，没有把它传入 `CopClient::new`。因此“种子驱动副本负载均衡”是保留状态和意图，不能据当前代码声称新建的 Rust `CopClient` 已实际消费该值。

`closed` 是 `AtomicBool`，用于确保 `close` 幂等；`clients` 是监听器强引用列表。`cpu_count` 在构造时固定为当前可用并行度，查询失败时退回 1，后续不会随运行时 CPU 配额变化而刷新。`disaggregated_tiflash` 与 `use_auto_scaler` 是构造后不变的布尔配置。

## 依赖与调用关系

上游生产调用者是 `pkg/store/driver/tikv_driver.rs`：驱动打开真实 TiKV 连接时创建 `NetworkBackend` 和本 `Store`，并把它保存在驱动内部。Rust 集成测试 `pkg/store/driver/coprocessor_adapter_test.rs` 多次以可控后端构造 `Store`，验证 DAG 路由、Region 重试、流关闭、资源控制与分页行为；`pkg/session/tests/paging_rpc.rs` 将同一构造接入 Session 的分页查询链。`pkg/store/driver/sql_fail_test.rs` 也构造该类型以覆盖失败路径。

主要下游关系如下：

- `Store::new` → `RegionCache::new`、`CoprocessorCache::new`、`thread::available_parallelism`。
- `Store::get_client` → `StoreCopBackend` → `CopClient::new`；`StoreCopBackend` 再调用 `RegionCache` 的 location/bucket 切分、batch task 上下文、Region/bucket 失效更新，以及 `StoreBackend` 的传输与锁接口。
- `Store::get_mpp_client` → `StoreMppTransport` 与共享 `RegionCache` → `MppClient::new`。
- `KvStore::client` → `TikvClient` → `RpcClient`/`MppAliveClient` → `StoreBackend`/`RegionCacheBackend`。
- `NetworkBackend` 在 `pkg/store/copr/network_backend.rs` 实现 `StoreBackend`；其测试 `pkg/store/copr/network_backend_test.rs` 直接通过 trait 调用验证超时、Region 失效、流关闭和锁解析。

RustCodeGraph 对本文件记录了 82 个符号，并能精确定位上述关键节点；其 `callers`/`callees` 查询对这些 trait 方法和同名方法返回空边。因此调用关系以精确节点加仓库引用搜索交叉确认，空图边不作为“无调用者”的证据。

## 错误处理与边界

所有后端可失败操作统一返回 `BatchResult`，本层多数方法不改写错误而用 `?` 或直接返回进行传播。`Store::new` 的显式失败点是 Coprocessor 缓存创建；系统时间早于 Unix epoch 不会失败，而是通过 `unwrap_or_default` 使用零时长；CPU 并行度查询失败则退回 1。

`StoreBackend::send_coprocessor_stream` 的默认错误是 `BatchError::OtherResponse("standard coprocessor stream transport is unavailable")`。这允许只实现非流接口的测试后端通过编译，但调用者不能由 trait 存在推断流式能力；生产 `NetworkBackend` 显式覆盖该方法。

`send_tiflash_batch` 的边界包括：找不到 Region 上下文时返回 `MissingRegion`；protobuf 序列化失败转为 `BatchError::Transport`；单个响应错误立即传播；发现非空 `retry_regions` 返回固定传输错误，不返回已收集的部分结果。它逐 location 构造单 Region 请求，协议类型硬编码为 `103`，扩展该路径时必须核对 TiFlash 协议常量和 Go 端行为。

互斥锁中毒通过 `expect` 触发 panic，消息分别指出 listener 或 cache owner 锁；这不是可恢复的 `BatchResult`。`add_event_listener` 先把监听器加入列表，再把最新监听器设置到后端；后端接口只接收一个 `Option<Arc<...>>`，因此列表负责生命周期保留，而底层是否支持多个监听器不能从本文件推断。`endpoint_type` 对当前封闭的 `StoreType` 四个变体穷尽匹配，没有 Go `default` 分支；将来扩展枚举会产生编译期遗漏提示。

## 并发与资源生命周期

`Store` 的共享状态通过原子或互斥量保护，因此其成员可被多个派生客户端并发访问。`close` 用 `closed.swap(true, Ordering::AcqRel)` 保证只有首次调用清空监听器并取走 Coprocessor 缓存；`Drop` 再调用一次 `close`，不会重复释放这些资源。`is_closed` 用 Acquire 读取状态。

关闭语义刻意只覆盖 Coprocessor 侧所有权：`Store::close` 不调用 `StoreBackend::close_client`，也不销毁共享 `KvStore`。这与 Go `Store.Close` 只关闭 Coprocessor cache 的边界一致，并允许已经克隆出的 `Arc<KvStore>` 继续存在。真正关闭底层客户端应通过 `TikvClient::close` 或拥有后端的外层驱动生命周期完成；`pkg/store/driver/coprocessor_adapter_test.rs` 的端到端关闭传播验证属于外层驱动与 `NetworkBackend` 的组合行为，不能误归因于 `Store::close` 单独完成。

`send_request_async` 不使用 Tokio runtime，也没有线程池或并发上限；每次调用产生一个 `JoinHandle<()>` 和一个 OS 线程。丢弃句柄不会取消请求，默认 cancellation token 也未暴露给调用方。回调 panic 只会终止该工作线程。高频使用或需要取消时，应优先在接口层重新设计执行器/取消语义，而不是在当前实现上无限派生线程。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/store.go`。Rust 的 `KvStore` 对应 Go `kvStore`，`TikvClient` 对应 Go `tikvClient`，`Store::new`/`NewStore` 对应 Go `NewStore`，`get_client`、`get_mpp_client`、`next_replica_read_seed` 和 `endpoint_type` 分别对应 `GetClient`、`GetMPPClient`、`nextReplicaReadSeed` 和 `getEndPointType`。

保持一致的语义包括：构造 Store 时建立 Coprocessor cache；种子使用原子先加后返回；获取 Cop 与 MPP 客户端时共享底层 store；`Close` 不关闭底层 KV store；TiFlash 在存算分离模式下映射为 `TiFlashCompute`。Go 测试 `pkg/store/copr/coprocessor_test.go` 通过 `NewStore` 和 `GetRegionCache` 覆盖 Region split、bucket 版本更新、锁处理和 store-batch 超时等实际行为，为 Rust 适配层的迁移语义提供对照。

当前实现也有明确差异。Go `NewStore` 接收具体 `*tikv.KVStore`，Rust 接收可注入的 `Arc<dyn StoreBackend>` 并显式传入两个 TiFlash 配置布尔值；Go 从全局配置读取存算分离状态。Go `GetClient` 把新种子存入 `CopClient.replicaReadSeed`，而 Rust 当前只推进种子，未传入 `CopClient::new`。Go 异步请求复用 client-go 的 async callback 和调用者 context；Rust 每次新建线程且使用默认 cancellation token。Go `getEndPointType` 对未知值回退 TiKV，Rust 因枚举穷尽而没有运行时回退。Go cache 的 `Close` 被显式调用；Rust 通过从 `Option<Arc<_>>` 取走引用并依赖 `Arc`/`Drop` 生命周期释放，若客户端仍持有缓存引用，实际析构会延后。

## 扩展指南

新增一种底层操作时，应先判断它属于路由元数据、Cop 传输还是 MPP 传输：路由能力放入 `RegionCacheBackend`；跨后端必须实现的传输/可见性能力加入 `StoreBackend`；仅 Cop 或 MPP 消费的适配分别落在 `StoreCopBackend` 或 `StoreMppTransport`。同步修改生产 `NetworkBackend` 和所有 mock 实现，避免靠默认实现掩盖生产缺口。

修改 Region/range 行为时，重点检查 `StoreCopBackend::split_key_ranges`、`send_tiflash_batch` 和 `RegionCache` 的 location/bucket API。需同步 Rust 的 `pkg/store/copr/region_cache_test.rs`、`pkg/store/copr/coprocessor_test.rs`、`pkg/store/copr/network_backend_test.rs`，并与 Go 的 `pkg/store/copr/coprocessor_test.go` 对照半开区间、Region split、bucket version、retry 和部分响应语义。协议字段变化还需核对 `kvproto` tag 与类型编号，避免静默产生线协议不兼容。

修改构造或关闭时，必须保留“Coprocessor Store 不拥有底层 KV store”的边界，或明确同步调整外层 `pkg/store/driver/tikv_driver.rs` 的关闭顺序及相关集成测试。若让 `closed` 真正禁止创建新客户端，应补独立 Rust 回归测试并决定 API 是否改为返回 `Result`；不能只增加检查后保留当前无错误返回类型。

副本读种子的 Rust 消费路径是当前最需要谨慎核对的扩展点：若恢复 Go 语义，应修改 `Store::get_client` 与 `CopClient` 构造/状态，并用独立测试证明连续客户端得到不同种子及其负载均衡效果。异步请求若改用 Tokio 或共享线程池，则需同时定义取消、回调 panic、运行时关闭和 `JoinHandle` 兼容策略，并评估高并发下的资源与性能变化。

## 验证依据

- 源码全貌：`pkg/store/copr/store.rs`，核对了全部 634 行、公开/私有类型、trait、impl、函数、别名和无条件编译事实。
- crate 边界：`pkg/store/copr/Cargo.toml` 与 `pkg/store/copr/lib.rs`，确认 crate 名、根模块、依赖、模块声明和公开再导出。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/copr` 确认本模块及测试面；`node --file pkg/store/copr/store.rs` 读取全文件；`query` 精确定位 `StoreBackend`、`NewStore`、`get_client`、`get_mpp_client`、`split_key_ranges`、`endpoint_type`。对这些节点运行 `callers`/`callees` 得到空边，故改用仓库引用搜索补证且未把空边解释为未使用。
- 生产调用与实现：`pkg/store/copr/network_backend.rs` 的 `impl StoreBackend for NetworkBackend`；`pkg/store/driver/tikv_driver.rs` 的 `NetworkBackend::connect` 和 `Store::new` 接线。
- Rust 测试证据：`pkg/store/copr/network_backend_test.rs` 直接验证 trait 后端的超时、Region 失效、流关闭和锁解析；`pkg/store/driver/coprocessor_adapter_test.rs` 验证用本 `Store` 组装后的 DAG 路由、重试、关闭传播和资源控制；`pkg/session/tests/paging_rpc.rs` 验证 Session 查询链中的真实接入。当前未发现同目录 `store_test.rs` 或直接覆盖 `endpoint_type`、`next_replica_read_seed`、`add_event_listener`、`Store::close` 单体语义的 Rust 测试，这是后续行为修改时应补的测试缺口。
- Go 对照：`pkg/store/copr/store.go`；相关行为测试位于 `pkg/store/copr/coprocessor_test.go` 与 `pkg/store/copr/copr_test/coprocessor_test.go`。
- 本任务是纯文档分析，按任务约束未运行 Cargo；验收使用任务指定的 11 章结构命令，并人工复核所有现状结论均能回指上述符号或文件。
