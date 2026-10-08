# `pkg/store/driver/tikv_driver.rs`

## 文件定位

本文件是 `astersql-store-driver` crate 的 TiKV 存储入口，位于 SQL/会话层与 PD、TiKV、Coprocessor、元数据服务之间。crate 根 `pkg/store/driver/lib.rs` 以私有模块 `tikv_driver` 装入本文件，再用 `pub use tikv_driver::*` 导出其公开 API；`pkg/store/driver/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/store/driver`。

服务启动链路中，`cmd/tidb-server/main.rs::registerStoresWithTiKVDriver` 把 `TiKVDriver` 包装成 `pkg/store/store.rs::TiKVStoreDriver` 并注册到 store registry；后者的 `Driver::Open` 调用本文件的 `TiKVDriver::Open`，随后调用 `TikvStore::StartGCWorker`。跨 keyspace 会话还会由 `pkg/session/runtime/crossks_store.rs::open_target_store_with_driver_and_tls` 和 `pkg/session/runtime/session_factory.rs` 直接调用 `OpenWithOptions`。

本文件不是单纯门面：它包含 URI/TLS/客户端选项解析、后端抽象、Store 缓存与资源回收、事务和元数据入口、锁等待聚合、请求追踪注入。但当前实现同时保留若干兼容占位，不能把所有与 Go 同名的方法理解为等价的完整实现。

## 核心职责

1. `parse_path` 解析 `tikv://` URI，得到 PD 地址、`disableGC` 和 keyspace；`Security::to_tls_config` 校验证书/私钥成对出现。
2. `TiKVDriver::setDefaultAndOptions` 每次打开前重载进程级 `GlobalConfig`，再应用一次性的 `DriverOption`，使库调用者能局部覆盖配置而不修改全局状态。
3. `TiKVDriver::OpenWithOptions` 选择注入后端或生产 `OfficialBackend`，建立 `ClientRuntime`、Coprocessor transport 和 Store 状态，并按 cluster/keyspace 缓存弱引用。
4. `TikvStore` 对外提供地址/TLS/客户端、事务时间戳、选项表、锁等待、keyspace 元数据和关闭生命周期。
5. `InjectTraceClient` 在同步或异步 TiKV 请求转发前，把 `TraceInfo` 写入 `Request.context.source_stmt`。
6. `DriverBackend` 隔离外部资源操作，使配置与生命周期测试可以使用 `InMemoryBackend` 或记录型测试后端。

## 主要符号

- `DriverError`：公开错误枚举。`InvalidPath`、`InvalidTls` 负责本地输入校验，`Backend` 包装外部客户端/锁中毒等字符串错误，`NotImplemented` 用于尚未实现的状态查询。
- `Security` / `TlsConfig`：TLS 文件路径输入与校验后的值对象。只要 CA、证书或私钥任一非空就生成 `Some(TlsConfig)`；证书和私钥必须同时配置。
- `GlobalConfig`、`get_global_config`、`set_global_config`：由 `OnceLock<RwLock<_>>` 保存的进程级配置快照。`metrics_labels_cell` 和 `ldflag_cell` 是另外两组独立全局状态。
- `ParsedPath` / `parse_path`：保留逗号分隔的多个 PD authority；识别 `disableGC`、`keyspaceName` 及兼容别名 `keyspace`，忽略未知 query 参数。
- `PdClientOptions` / `TiKVDriver::pdClientOptions`：汇总最大接收消息、keepalive、PD 超时、全局 forwarding 和指标标签。
- `DriverBackend`：打开/关闭 PD、创建/关闭 safe-point、关闭 Store、取时间戳、查询锁等待的同步抽象。三个关闭方法有默认空实现或成功结果。
- `OfficialBackend`：生产默认后端，持有共享 `Arc<RwLock<ClientRuntime>>`；时间戳、锁等待和关闭转给 `ClientRuntime`。其 `open_pd` 当前以 PD 地址的稳定 FNV-1a 哈希作为 cluster ID，`new_safe_point_kv` 当前构造合成的 safe-point ID，并未在这里创建真实 safe-point KV 客户端。
- `OfficialTransactionLockResolver`：把 Coprocessor 遇到的锁交回同一个 `ClientRuntime::resolve_locks`，错误映射成 `BatchError::OtherResponse`。
- `InMemoryBackend`：用于测试/显式注入的内存后端；cluster ID 同样来自稳定哈希，时间戳由 `Mutex<u64>` 单调递增。
- `DriverOption` 及 `WithSecurity`、`WithTiKVClientConfig`、`WithTxnLocalLatches`、`WithPDClientConfig`：消费一次的配置闭包。
- `TiKVDriver`：打开 Store 的有状态驱动；`backend == None` 表示走生产 client-rust 路径，`with_backend` 用于替换资源边界。
- `TikvStore` / `TikvStoreInner`：可克隆的共享句柄和受 `Mutex` 保护的实际状态。内部保存 uuid、TLS、GC/闩锁状态、cluster/keyspace、类型擦除选项、后端、可选 `ClientRuntime`、可选 Coprocessor Store 和惰性元数据快照。
- `MetadataSnapshot`：共享 `MetadataPdClient` 与可选 keyspace 元数据；实现 `EtcdMetadataStore` 时由 `metadata_snapshot` 惰性创建并缓存。
- `Version`、`Transaction`、`Snapshot`：轻量兼容类型。`Transaction` 只含 `start_ts`，`Snapshot` 只含版本；真实悲观事务由 `BeginPessimistic` 经 `kv_adapter::begin_transaction` 创建。
- `InjectTraceClient<C>` / `TikvClient`：请求注入包装器及其底层同步/异步发送接口。
- `Codec`、`MppClient`、`MemManager`、`CoprocessorClient`：兼容类型；其中后三者在本文件没有真实状态，`GetClient` 实际返回的是 `astersql_store_copr::CopClient`。

## 执行流程

`TiKVDriver::Open` 只是以空选项调用 `OpenWithOptions`。后者的主流程如下：

1. 以全局配置重置 driver 字段并依次消费 `DriverOption`。
2. `parse_path` 校验 scheme、非空 PD 地址和布尔参数；`pdClientOptions` 与 `Security::to_tls_config` 形成连接配置。
3. 若已注入 `DriverBackend`，直接使用它且不建立 `ClientRuntime`；否则根据 PD 地址、PD 超时、TLS 和 keyspace API V2 配置调用 `ClientRuntime::connect`，再构造共享该 runtime 的 `OfficialBackend`。
4. 调用 `backend.open_pd` 得到 cluster ID，以 `tikv-{cluster_id}/{keyspace}` 形成 uuid。
5. 锁住进程级 `store_cache`，清理没有强引用的条目。若相同 uuid 的弱引用仍可升级，关闭本次多余的 PD 连接；生产路径还关闭刚建立的多余 runtime，然后返回缓存中的同一 `TikvStoreInner`。
6. 调用 `new_safe_point_kv`。失败时关闭 PD；生产路径额外关闭 runtime。
7. 生产路径为 `astersql_store_copr::NetworkBackend` 组装 PD 地址、超时、keyspace、TLS 与 `OfficialTransactionLockResolver`，再创建唯一的 `astersql_store_copr::Store`。连接或构造失败时依次清理 safe-point、PD 和 Store runtime。
8. 构造 `TikvStoreInner`：`disableGC` 取反得到 `enable_gc`，只有启用 local latches 才保存容量；最后把弱引用写入缓存并返回句柄。

运行期间，`Begin`/`CurrentVersion` 通过 backend 取时间戳；`BeginPessimistic` 进入 `kv_adapter` 的真实 client-rust 事务路径；`GetClient` 从共享 Coprocessor Store 获取客户端；`GetLockWaits` 合并各后端响应并跳过失败项。

`metadata_snapshot` 先检查 Store 未关闭和已有缓存，再依据 Store TLS 与 PD 地址调用 `ConnectMetadataPD`。非空 keyspace 必须能由 PD 加载元数据；创建完成后再次加锁，处理 Close 竞态或另一线程已经发布快照的竞态，未采用的 PD 客户端会立即关闭。

`Close` 先在 Store 锁内做幂等标记、取走并关闭元数据 PD 快照，然后移除全局缓存，关闭 Coprocessor Store/其 KV 客户端，最后调用 backend 的 safe-point、PD 和 Store 关闭钩子。返回值优先保留 Coprocessor 关闭错误；仅其成功时才返回 backend Store 关闭结果。

## 数据与状态

- 全局配置、指标标签、ldflag 和 Store 缓存分别由独立的 `OnceLock` 初始化；前三者使用 `RwLock`，缓存使用 `Mutex<HashMap<String, Weak<_>>>`。
- 缓存键是 cluster ID 与 keyspace 的组合，不直接包含 TLS 或 PD 地址文本。默认后端的 cluster ID 当前由地址列表稳定哈希产生，因此地址顺序变化会形成不同缓存键；相同地址和 keyspace 的配置差异不会形成不同键。
- 缓存只持有 `Weak<Mutex<TikvStoreInner>>`，不会独自延长 Store 生命周期；但本文件没有 `Drop` 自动关闭，调用者仍需显式 `Close` 来释放外部资源。
- `TikvStore` 的克隆共享同一 `TikvStoreInner`，因此关闭、GC 标志、动态选项和元数据快照对所有克隆可见。
- 动态选项以 `HashMap<String, Arc<dyn Any + Send + Sync>>` 保存；`GetOption<T>` 类型不匹配时返回 `None`，`SetOption(..., None)` 删除键。
- `metadata_snapshot` 的 PD 客户端和 keyspace 元数据具有 Store 生命周期；`Close` 会取走快照并关闭 PD，关闭后的后续元数据访问报错。
- `Request`/`Response`/`TraceContext` 是本文件自有的轻量请求模型，不是完整 TiKV protobuf 模型。

## 依赖与调用关系

上游直接证据：

- `cmd/tidb-server/main.rs::registerStoresWithTiKVDriver` 配置并注册 `TiKVDriver`。
- `pkg/store/store.rs::TiKVStoreDriver::Open` 序列化可变 driver 访问，调用 `Open` 和 `StartGCWorker`，再包装成 canonical storage。
- `pkg/session/runtime/crossks_store.rs`、`pkg/session/runtime/session_factory.rs` 为目标 keyspace 组装 URI/TLS option 并调用 `OpenWithOptions`。
- `pkg/session/runtime/session.rs` 根据 `has_real_client_runtime` 决定是否使用真实 PD/etcd 元数据，并调用 `etcd_namespace`。
- `pkg/session/runtime/system_query.rs` 经存储抽象调用 `GetLockWaits`。

下游直接依赖：

- `crate::client_runtime::{ClientConfig, ClientRuntime, KeyspaceConfig}`：生产事务、时间戳、锁解析和关闭边界。
- `astersql_store_copr::{NetworkBackend, Store, CopClient, TransactionLockResolver}`：DAG Coprocessor transport 与 RegionCache。
- `crate::kv_adapter::begin_transaction` 与 `astersql-store-driver-txn`：真实悲观事务适配。
- `astersql_metaservice::{ConnectMetadataPD, EtcdMetadataStore}`：keyspace 元数据和 etcd namespace。
- `thiserror`、`url::form_urlencoded`：错误声明与 query 解码。

`pkg/store/driver/Cargo.toml` 还声明带固定 tag `v0.4.2-aster.10` 的 `astersql/client-rust` Git 依赖；本文件通过 `client_runtime` 间接使用它。RustCodeGraph 将本文件索引为 136 个符号，并报告文件被 `pkg/session/runtime/session.rs`、两个 session 测试及 mock storage 等引用；但对目标 Rust 符号执行精确 `callers/callees` 查询未输出边，因此上述调用关系以索引文件引用、模块代码和 `rg` 精确引用共同核验。

## 错误处理与边界

- `parse_path` 拒绝非 `tikv://`、空地址和非法 `disableGC`；它允许 authority 后带路径但只取 `/` 前内容，也会忽略未知 query 参数。
- TLS 只校验“证书与私钥成对”，不在这里检查文件是否存在、内容是否合法或 CA 是否必填；实际握手错误由下游连接返回。
- `OpenWithOptions` 对 runtime、safe-point 和 Coprocessor 构造错误映射为 `DriverError::Backend`，并在已获取资源的各失败分支显式回滚。关闭钩子的错误多为 best effort，回滚分支会丢弃 `close_store` 错误。
- 标准库 `Mutex`/`RwLock` 均直接 `unwrap`；若锁被 poison，这些路径会 panic，而不是返回 `DriverError`。`OfficialBackend` 访问 runtime 的锁则显式映射为 `Backend`。
- `GetClient` 在注入后端路径没有 Coprocessor Store 时返回“coprocessor transport is unavailable”。
- `GetLockWaits` 与 Go 一样跳过单个 Store 的错误响应，整体始终返回 `Ok`；这会牺牲局部错误可见性以保留可用结果。
- `ShowStatus` 明确返回 `NotImplemented`。`GetMPPClient`、`GetMemCache`、`GetSnapshot` 和普通 `Begin` 只提供轻量兼容行为，不应作为功能完整性的证据。
- `StartGCWorker` 当前只在 `enable_gc` 时设置布尔标记，没有创建后台 GC worker；重复调用也不会报错。它与 Go 的真实 worker 生命周期存在实质差异。
- `OfficialBackend::open_pd` 不从 PD 获取真实 cluster ID，`new_safe_point_kv` 也不持有真实 safe-point 客户端；依赖这些语义的功能必须先补齐实现与回归测试。
- `InjectTraceClient` 仅在有 `TraceInfo` 时创建/覆盖 `SourceStmt`，底层错误原样返回；与 Go 同步路径相比，Rust 未执行 flight-recorder dump trigger 检查。

## 并发与资源生命周期

`TiKVDriver::OpenWithOptions` 在整个缓存检查、外部资源创建和缓存插入期间持有全局 `store_cache` Mutex。这保证相同 uuid 不会并发创建两个已发布 Store，但也意味着慢连接会串行阻塞其他 cluster/keyspace 的 Open。

`TikvStoreInner` 用单个 Mutex 串行保护所有状态；多数 getter 会克隆小型值或 `Arc` 后立即释放锁。`Begin`/`CurrentVersion` 则在持有 Store Mutex 时调用 backend，若后端耗时会阻塞同 Store 的其他操作。`InMemoryBackend` 另用 Mutex 保证时间戳严格单调。

元数据快照采用“锁外连接、锁内发布”的双检模式：它避免在网络连接期间持有 Store 锁，并通过第二次检查安全处理并发 Close 和重复初始化。失败或竞争中落败的 PD 客户端会关闭。

`Close` 通过 `closed` 标志幂等；克隆句柄都观察同一状态。它先释放 Store Mutex 再取得缓存锁和执行外部关闭，避免长时间持有 Store 锁。没有 `Drop` 兜底意味着遗忘显式 Close 时，Rust `Arc` 会释放内存对象，但是否完全关闭外部客户端取决于各下游类型自身的 Drop 行为，本文件未作保证。

`InjectTraceClient::SendRequestAsync` 在调用底层异步接口前完成就地注入；callback 的执行与线程模型完全由底层客户端决定，包装器不保存请求或 callback。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/driver/tikv_driver.go`。Rust 保留了 `TiKVDriver`、Option 构造器、`Open/OpenWithOptions`、`tikvStore` 别名、地址/TLS/GC/事务/锁等待/追踪注入等接口形状，并由独立测试固定若干 Go 行为：

- 每次 Open 先重载全局默认再应用 option；option 不回写全局配置。
- `tikv://pd1,pd2` 的逗号地址、`disableGC` 和 keyspace 解析语义。
- 相同 cluster/keyspace 复用缓存 Store，命中缓存后关闭多余 PD；Close 幂等并移除缓存。
- `disableGC` 阻止 GC 启动标记；动态 option 的 `None` 删除语义。
- 锁等待聚合跳过失败响应；同步/异步追踪均在转发前注入 `SourceStmt`。

主要差异也必须保留在阅读结论中：

- Go 使用 PD client 的真实 `GetClusterID`、`CodecPDClient`、真实 `EtcdSafePointKV` 和 `tikv.KVStore`；Rust 默认路径建立真实 `ClientRuntime` 与 Coprocessor transport，但 cluster ID/safe-point 设置仍由 `OfficialBackend` 合成。
- Go `StartGCWorker` 构造并启动 `gcworker.GCWorker`，Close 时关闭它；Rust只记录布尔状态。
- Go 的 `Begin`、`GetSnapshot`、`GetMPPClient`、`GetMemCache` 返回完整实现；Rust同名普通入口多为轻量类型或占位，真实事务能力集中在 `BeginPessimistic`/`kv_adapter`。
- Go 的全局缓存以强引用保存 `*tikvStore`；Rust使用弱引用，存活性由外部 `Arc` 决定。
- Go 同步追踪包装还检查 flight-recorder dump trigger；Rust未移植该副作用。
- Rust新增 `metadata_snapshot`/`EtcdMetadataStore` 适配，惰性持有元数据 PD 客户端并计算 `/keyspaces/tidb/{id}` namespace。

## 扩展指南

- 补齐真实 PD/safe-point 语义时，优先修改 `OfficialBackend::open_pd`、`new_safe_point_kv`、对应关闭钩子及 `TikvStoreInner` 的资源字段；必须保持 `OpenWithOptions` 各失败分支和 `Close` 的逆序清理完整，并更新 `driver_lifecycle_test.rs` 与真实 TiKV 测试。
- 增加 URI 参数时修改 `ParsedPath` 和 `parse_path`，同步扩展 `config_test.rs` 的有效、非法、缺省和编码边界用例，并核对 Go `config.ParsePath`。
- 新增 per-open 配置应增加 `TiKVDriver` 字段与 `DriverOption`，同时在 `Default` 和 `setDefaultAndOptions` 中赋值，避免连续 Open 泄漏上一次 option。
- 修改缓存身份时审查 uuid 的 cluster、keyspace、TLS/安全配置含义；同时覆盖并发 Open、缓存命中、多句柄 Close 和失败后重开。全局缓存锁当前覆盖网络连接，若缩小锁范围需引入明确的 single-flight 机制。
- 扩展元数据时复用 `metadata_snapshot` 的锁外连接/锁内发布模式，并确保 Close 竞态下新建资源被关闭；同步 `meta_service_group_test.rs`。
- 扩展请求追踪时同时更新 `SendRequest` 与 `SendRequestAsync`，保持“先注入、后转发”和错误/回调透传；同步 `client_test.rs`，如需对齐 Go 还应评估 flight-recorder trigger。
- 实现目前的占位接口时不要只替换返回类型外形；应接到同一 `ClientRuntime`/Coprocessor Store，避免创建第二套路由缓存或事务客户端，并在独立测试文件中验证真实行为。Rust 源文件和测试逻辑必须继续分文件保存。
- 性能风险集中在全局 Open 锁、每次访问的 Store Mutex、锁内 backend 时间戳调用及全量锁等待聚合；兼容风险集中在 Go 接口语义、keyspace API V1/V2、资源关闭顺序和错误映射。

## 验证依据

事实核验读取了以下路径：

- 目标实现：`pkg/store/driver/tikv_driver.rs`。
- crate 边界与依赖：`pkg/store/driver/lib.rs`、`pkg/store/driver/Cargo.toml`。
- Go 对照：`pkg/store/driver/tikv_driver.go`。
- 生产调用入口：`cmd/tidb-server/main.rs`、`pkg/store/store.rs`、`pkg/session/runtime/crossks_store.rs`、`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/session.rs`。
- 独立 Rust 测试：`pkg/store/driver/config_test.rs`、`driver_lifecycle_test.rs`、`client_test.rs`、`meta_service_group_test.rs`、`kv_adapter_test.rs`、`real_tikv_test.rs`；其中元数据 group 测试标记为 `ignore` 并要求外部 `ASTER_ETCD_TEST_ENDPOINT`。

RustCodeGraph 证据：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/driver` 找到目标及相邻 Go/测试文件；`query` 定位 `TiKVDriver`（第 486 行）、`OpenWithOptions`（第 539 行）、`BeginPessimistic`（第 946 行）和 `metadata_snapshot`（第 1003 行）；`node --file` 分段读取了目标文件全部 1,268 行。精确 `callers/callees` 对这些 Rust 符号未返回可用边，因此没有据此臆造调用关系，改由索引的 file-use 信息与 `rg` 引用结果补证。

相关测试给出的直接行为证据包括：`config_test.rs` 的默认/option/路径断言，`driver_lifecycle_test.rs` 的缓存、关闭顺序、GC 标志和锁等待断言，`client_test.rs` 的同步/异步追踪注入与错误透传，`meta_service_group_test.rs` 的元数据缓存和 Close 后失效，以及 `real_tikv_test.rs` 的错误 PD 快速失败。此次任务为纯文档分析，按计划不运行 Cargo；最终仅执行固定十一章节的结构命令，并人工检查未把占位接口写成完整实现。
