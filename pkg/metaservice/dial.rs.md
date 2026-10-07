# `pkg/metaservice/dial.rs`

源码：[pkg/metaservice/dial.rs](dial.rs)；独立 Rust 测试：[pkg/metaservice/dial_test.rs](dial_test.rs)；Go 对照：[pkg/metaservice/etcd.go](etcd.go) 与 [pkg/metaservice/etcd_test.go](etcd_test.go)。

## 文件定位

`dial.rs` 是 `astersql-metaservice` crate 中把同步的 BR、Lightning、IMPORT INTO 等调用方接到异步 PD/etcd 客户端的连接层。crate 根模块在 `pkg/metaservice/lib.rs` 中声明并重新导出 `dial`；依赖边界由 `pkg/metaservice/Cargo.toml` 确认，网络实现分别使用带 TLS feature 的 `etcd-client 0.19`、固定 tag `v0.4.2-aster.10` 的 `tikv-client` 以及 Tokio。文件自身不管理更高层元数据业务，而是完成三件事：取得完整 keyspace 元数据、根据 meta-service group 规则选择 etcd endpoints、返回自动附加 keyspace 前缀的同步客户端。

真实上游分为两类。`DialEtcdClient` 供 `br/pkg/task/common.rs::dialEtcdWithCfgAndFactory`、`br/pkg/task/operator/crr_checkpoint.rs::dialEtcdWithCfgAndFactory` 和 `lightning/pkg/importer/precheck_impl.rs::dialEtcdWithCfg` 使用；`NewEtcdClientFromStore` 则供 `lightning/pkg/importer/import.rs::newEtcdClientForLocalBackend` 与 `pkg/executor/importer/table_import.rs::newEtcdClientForAllocatorRebase` 借用现有 store 的 PD 状态。`pkg/store/driver/tikv_driver.rs` 中 `TikvStore` 对 `EtcdMetadataStore` 的实现是后一条链的生产实现。

## 核心职责

- `ConnectMetadataPD` 创建只用于成员发现和一次 keyspace 查询的 `tikv_client::MetadataClient`，再由 `NetworkPd` 收窄为本文件需要的 `MetadataPdClient` 接口。这样 `DialEtcdClient` 能加载一次 `DialKeyspaceMeta`，且无论后续 etcd 连接成功还是失败都会调用 `pd.close()`。
- `ResolveEtcdDialInfo` 决定最终 endpoints 与 namespace。调用方传入的非空 endpoint 优先保留；只有 endpoint 为空且未配置专属 group 时才调用 `get_pd_addrs` 做 PD 成员发现。专属 group 的地址及 GC/group 合法性委托 `pkg/metaservice/metamanager.rs::get_info` 检查。
- `NewEtcdClientFromPDClient` 把解析结果和 `EtcdDialConfig` 变为 `etcd_client::Client`，并包成 `NamespacedEtcdClient`。所有键操作都经 `key` 加上 `/keyspaces/tidb/<id>` 前缀；返回结果时 `get_entries` 再移除此命名空间前缀。
- `RpcWorker` 为同步调用者独占一个单线程 Tokio runtime，避免在已有 Tokio runtime 内直接嵌套 `block_on`。PD 和 etcd 网络请求均通过该 worker 排队执行，并由 `checked` 统一感知 `Context` 取消。
- `PdSecurity::etcd_tls` 将 PD 使用的 CA、证书和私钥材料转换为 etcd TLS 配置，确保两条连接使用相同身份材料。

## 主要符号

- `DialKeyspaceMeta { id, name, config }`：连接与 codec 所需的完整 keyspace 快照。`id` 用于 namespace，`name/config` 被转换为 `KeyspaceMeta` 供 group 规则解析。
- `MetadataPdClient: PdClient`：在已有成员发现能力上增加 `load_keyspace` 和显式 `close`。`NetworkPd` 是真实网络实现；`pkg/metaservice/dial_test.rs::Pd` 是独立测试替身。
- `PdSecurity` 与 `PdSecurity::etcd_tls`：保存三个可选文件路径。全空返回 `Ok(None)`；证书、私钥只配置一个会报 `MetaServiceError::ServiceUrl`；读取失败也附带具体路径。
- `PdClientFactory`：线程安全的依赖注入闭包，返回共享的 `MetadataPdClient`。生产路径缺省使用 `ConnectMetadataPD`，测试和嵌入方可注入受控实现。
- `RpcWorker`、`WorkerMessage` 与 `checked`：内部同步/异步桥。`call` 把闭包与一次性结果通道送到 worker；`checked` 在 future 完成和上下文取消之间竞争。
- `EtcdDialConfig`：etcd TLS、5 秒连接超时、10 秒 keepalive 间隔、3 秒 keepalive 超时和 idle keepalive 开关。keepalive 间隔为零时完全不配置 keepalive；超时为零且 keepalive 已启用时使用 20 秒默认值。
- `EtcdDialInfo { endpoints, namespace }` 与 `ResolveEtcdDialInfo`：纯解析结果及其入口。keyspace ID 必须不大于 `0x00ff_ffff`；有元数据时 namespace 固定为 `/keyspaces/tidb/<id>`，无元数据时为空。
- `NamespacedEtcdClient`：可克隆的同步客户端门面，公开 `put`、`get`、`get_entries`、`delete`、`grant`、`keepalive`、`time_to_live`、`revoke`、`close`，以及 `with_context`、`endpoints`、`namespace`、`is_closed`。
- `EtcdEntry`：读取结果中的去前缀键、原始值和 lease ID。
- `EtcdMetadataStore` 与 `NewEtcdClientFromStore`：从 store 借用 PD 客户端和与 codec 一致的 keyspace 元数据，不关闭 store 所有的 PD 客户端。

## 执行流程

新建 PD 路径从 `DialEtcdClient` 开始：

1. 使用注入的 `PdClientFactory`，或由 `ConnectMetadataPD` 创建 `NetworkPd`。后者在 `RpcWorker` 的 runtime 上调用 `tikv_client::MetadataClient::connect`，连接超时固定为 5 秒，TLS 由 `tikv_client::SecurityManager` 加载。
2. `keyspace_name` 为空时不加载元数据；非空时恰好调用一次 `MetadataPdClient::load_keyspace`。返回 `None` 会变为带 keyspace 名称的 `MissingKeyspaceMeta`。
3. 调用 `NewEtcdClientFromPDClient`；局部闭包保存结果后，无条件执行 `pd.close()`，因此错误路径也释放本次连接拥有的 PD client。
4. `ResolveEtcdDialInfo` 仅过滤严格等于空字符串的调用方 endpoint，空白字符串不会被 trim。若仍为空且 keyspace 没有 `GROUP_ID_KEY`，才通过 `get_pd_addrs(context, pd, false)` 查询成员地址。
5. 元数据被映射成 `KeyspaceMeta` 后传给 `get_info`。专属 group 使用配置中的 group 地址并校验 group ID、地址和 keyspace-level GC；全局 group 使用调用方/发现得到的 PD 地址。
6. 有 keyspace 元数据时验证 24 位 ID 上限并生成 namespace；随后 `NewEtcdClientFromPDClient` 创建另一个 `RpcWorker`，组装 `ConnectOptions`，异步连接 endpoints，最后把 session、endpoint、namespace 和原始 `Context` 放入 `NamespacedEtcdClient`。

已有 store 路径由 `NewEtcdClientFromStore` 调用 `store.pd_client()` 与 `store.keyspace_meta()`，再直接进入上述第 4 至第 6 步。它不调用 `MetadataPdClient::close`，这与 trait 注释所声明的借用所有权一致。

每个业务操作先由 `NamespacedEtcdClient::key` 拼接 namespace，再由私有 `call` 克隆底层 etcd client 与当前 context 并排队到 worker。`get_entries` 将服务端键的 namespace 字节前缀剥离；若响应键意外不含该前缀，则保留原键而不是报错。`keepalive` 建立 lease stream、发送一次 keep-alive、等待一次响应，并拒绝 stream 提前关闭或 TTL 非正的结果。

## 数据与状态

连接状态集中在 `EtcdSession { worker, client }`。`client` 是 `Arc<Mutex<Option<Client>>>`：`Some` 表示可提交操作，`close` 在 worker 队列中把它置为 `None`，`is_closed` 与后续 `call` 都读取这一状态。多个 `NamespacedEtcdClient` clone 共享同一个 session、endpoint 向量和 namespace，但各 clone 可以通过 `with_context` 绑定不同的取消上下文；关闭任何一个 clone 会关闭全部 clone 共享的底层客户端。

`NetworkPd` 同样以 `Mutex<Option<Arc<MetadataClient>>>` 表示开关状态。`close` 只取走客户端，不直接停止 worker；最后一个 `Arc<RpcWorker>` 被释放时，worker 的 `Drop` 才发送 `Stop` 并 join 线程。`DialKeyspaceMeta` 是连接时读取的快照，本文件不会监听或刷新配置；store 路径则由 `EtcdMetadataStore` 的实现负责提供一致的 PD 与 codec 元数据快照，`TikvStore` 通过一次 `metadata_snapshot()` 分别导出它们。

命名空间只是客户端侧字符串前缀，不是独立事务或 ACL。所有 value 保持原始字节；`EtcdEntry` 额外保存服务端 lease ID。endpoint 顺序沿用调用方、PD 成员或 group 配置解析结果，本文件不排序、不去重。

## 依赖与调用关系

直接下游关系由 RustCodeGraph 对 `dial.rs` 的符号/调用边确认：

- `DialEtcdClient -> ConnectMetadataPD | PdClientFactory -> MetadataPdClient::load_keyspace -> NewEtcdClientFromPDClient -> MetadataPdClient::close`。
- `NewEtcdClientFromStore -> EtcdMetadataStore::{pd_client,keyspace_meta} -> NewEtcdClientFromPDClient`。
- `NewEtcdClientFromPDClient -> ResolveEtcdDialInfo -> get_pd_addrs/get_info -> etcd_client::Client::connect`。
- `NetworkPd::{get_all_members,load_keyspace}` 与各 `NamespacedEtcdClient` RPC 方法都通过 `RpcWorker::call -> checked` 进入异步客户端。

`get_pd_addrs` 来自 `pkg/metaservice/etcd.rs`，负责 PD 成员 URL 解析、退避和取消；`get_info`、`KeyspaceMeta`、`GROUP_ID_KEY` 与 group 校验来自 `pkg/metaservice/metamanager.rs`。外部网络依赖是 `tikv-client` 的 metadata API 和 `etcd-client` 的 KV/lease API。crate 根 `pkg/metaservice/lib.rs` 公开重新导出这些符号，所以上游以 `astersql_metaservice::...` 直接调用。

RustCodeGraph 的文件级索引显示 `dial.rs` 被 BR、Lightning、executor/store 及其测试等文件使用；图的精确 `callers` 子命令对这些 Go 风格大写 Rust 符号未返回边，因此上游调用位置又用局部 `rg` 核对，未把空结果解释为“无调用者”。

## 错误处理与边界

- TLS 三路径全空是明文连接；证书与私钥必须成对。CA 可单独提供。文件读取和 `SecurityManager::load` 错误分别映射为 `ServiceUrl` 与 `Pd`。
- `RpcWorker::new` 会等待 runtime 构造完成；runtime 构造失败、启动握手通道断开、任务发送失败或结果通道断开都转成 `MetaServiceError::Pd`。内部 mutex 使用 `unwrap`，因此锁中毒会 panic，而不是转为领域错误。
- `checked` 在开始前先查取消状态，执行中每 10 毫秒轮询一次；取消返回 `Cancelled`。这不会撤销已由远端提交但尚未收到响应的副作用，因此调用者不能把取消等同于“操作必未发生”。
- 调用方 endpoints 只删除 `""`，不会删除 `" "`；该兼容行为由 `resolve_preserves_proxy_and_filters_only_empty_endpoints` 明确测试。专属 group 时即使没有调用方 endpoints 也不查询 PD members。
- keyspace ID 超过 24 位返回 `Pd("invalid keyspace id ...")`。此检查对应 Go 版 `tikv.NewCodecV2` 建 codec 的限制；Rust 直接构造 namespace，没有复制整个 codec 对象。
- etcd 网络错误经 `MetaServiceError::Etcd` 原样包装；已关闭客户端则在发任务前返回 `Pd("etcd client is closed")`。`close` 是幂等的：重复调用仍只会将 `Option` 置空。
- `keepalive` 只完成一次请求/响应，不启动持续保活任务；空响应和过期 lease 被显式拒绝。`grant` 不限制 TTL，合法性由 etcd 服务端决定。
- `DialEtcdClient` 拥有并关闭它创建/注入的 PD client；`NewEtcdClientFromPDClient` 与 `NewEtcdClientFromStore` 借用 PD，不会关闭它。选择错误入口会造成所有权语义差异。

## 并发与资源生命周期

每次 `ConnectMetadataPD` 和每次 `NewEtcdClientFromPDClient` 都各自创建一个当前线程 Tokio runtime 与 OS 线程。`mpsc::channel` 是无界同步通道；单个 worker 串行执行其收到的闭包，因此共享一个 `NamespacedEtcdClient` session 的网络操作不会在该 runtime 内并发执行。同步调用者一直阻塞到结果通道返回，长 RPC 会阻塞同一 session 后续排队操作。

`RpcWorker::Drop` 发送 `Stop` 后 join。worker 循环只匹配 `Call`，收到 `Stop` 或所有 sender 断开都会退出；由于消息按序接收，`Stop` 之前已入队的调用会先完成。`EtcdSession` 与 `NetworkPd` 都通过 `Arc` 延长 worker 生命周期；最后一个所有者释放后才 join。`NamespacedEtcdClient::close` 关闭逻辑客户端状态，但 worker 线程直到 session 最后一个 `Arc` 释放才回收。

`Context` 可在 clones 间共享内部取消标志；`with_context` 只替换门面上的 context，不复制底层连接。`MetadataPdClient`、factory 与 store trait 都要求 `Send + Sync`，而 `RpcWorker::call` 的闭包与结果要求 `Send + 'static`，确保跨线程传递安全。代码没有后台 lease keepalive 循环、自动重连管理器或配置热更新任务。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metaservice/etcd.go`，测试是 `pkg/metaservice/etcd_test.go`。主要语义保持一致：

- Rust `ResolveEtcdDialInfo` 对应 Go `resolveEtcdDialInfo`：都仅过滤空字符串、优先保留 caller endpoints、专属 group 跳过 PD member discovery，并从 keyspace ID 建 `/keyspaces/tidb/<id>` namespace。
- Rust `DialEtcdClient` 对应同名 Go 函数：都使用 V1 discovery PD client、对非空 keyspace 名称显式执行一次 metadata lookup、缺失时把名称放入错误，并在返回前关闭临时 PD client。Rust 将 caller component 与 PD options 封装进上游/`tikv-client` 固定连接接口，因此签名比 Go 少这些参数。
- Rust `NewEtcdClientFromPDClient` 对应 Go 导出函数和内部 `newEtcdClientFromPDClient`。Go 通过 `clientv3.Config.Context/Endpoints` 与 `etcd.SetEtcdCliByNamespace` 改写客户端；Rust 用 `ConnectOptions` 建连接，并在每个 KV 方法中显式拼接/剥离 namespace。
- Go 直接返回异步生态中的 `*clientv3.Client`；Rust 为同步移植代码提供 `NamespacedEtcdClient` 和专属 `RpcWorker`。Rust 目前只暴露本文件列出的 KV/lease 子集，不能据此推断已经覆盖 Go etcd client 的全部 API。
- Go 用 `tikv.NewCodecV2` 与 `keyspace.MakeKeyspaceEtcdNamespace` 生成命名空间；Rust 直接检查 24 位 ID 上限并格式化相同路径。新增 codec 模式或 namespace 规则时必须同时核对这两处，而不能只改字符串。
- Rust 独有 `EtcdMetadataStore`/`NewEtcdClientFromStore`，用于在 Rust store 边界借用已经解析好的 PD 与 metadata，避免另建 PD client；其所有权意图与 Go 内部直接复用已有 store client 的调用方式相同，但不是 Go 文件中的同名 API。

`pkg/metaservice/dial_test.rs` 与 Go 测试共同证明 proxy endpoint 保留、专属 group 不做成员发现、缺失 metadata 的错误文本与单次加载。Rust 的 ignored real-etcd 测试还覆盖 namespace 读写、lease、取消、关闭和 PD 所有权；默认单元测试不会启动真实 etcd。

## 扩展指南

- 增加 etcd 操作时，应在 `NamespacedEtcdClient` 上实现，并统一经过 `key`（键相关操作）、私有 `call` 和 `checked`，避免绕过 namespace、关闭状态或取消语义。测试放在独立的 `pkg/metaservice/dial_test.rs`，不要内嵌到生产文件。
- 修改 endpoint/group 选择时，入口是 `ResolveEtcdDialInfo`；同时核对 `pkg/metaservice/metamanager.rs::get_info/get_group` 和 Go `pkg/metaservice/etcd.go::resolveEtcdDialInfo`。必须保留 caller proxy 优先级，并分别测试全局 group、专属 group、空字符串与空白字符串。
- 修改 keyspace namespace 或 ID 范围时，同时对照 Go 的 `tikv.NewCodecV2`/`keyspace.MakeKeyspaceEtcdNamespace` 以及 store 提供的 codec metadata；这是数据兼容边界，错误前缀可能让调用者读写另一个租户或不可见键。
- 扩充连接参数时，在 `EtcdDialConfig`、`NewEtcdClientFromPDClient` 的 `ConnectOptions` 映射以及 BR/Lightning 上游配置转换处同步接线，并添加默认值、零值和 TLS 测试。连接/keepalive 设置会影响故障恢复时延和资源消耗。
- 改动 PD 生命周期时需明确两条所有权路径：`DialEtcdClient` 必须在所有退出路径关闭临时 PD；`NewEtcdClientFromStore` 必须继续借用而不能关闭 store-owned PD。现有 `factory_*` 测试应同步扩充成功与失败计数断言。
- 若要提高单 session 并发度，不能仅把 worker 改成多线程 runtime；当前队列顺序、`Client` clone、close 排队以及同步返回共同形成可观察行为。应先设计 close 与在途请求的顺序保证，并添加独立并发回归测试。
- 真实网络行为依赖 ignored 测试的 `ASTER_ETCD_TEST_ENDPOINT`。新增 lease/事务/watch 等行为时，应同时提供无需外部服务的确定性单元测试和显式 opt-in 的真实服务测试。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/metaservice/dial.rs` 被识别为 604 行、63 个符号，`pkg/metaservice/dial_test.rs` 为 11 个符号。
- RustCodeGraph 源码与符号查询：读取 `pkg/metaservice/dial.rs` 全部 1–604 行；精确查询确认 `DialEtcdClient`（554）、`NewEtcdClientFromPDClient`（510）、`ResolveEtcdDialInfo`（281）和 `NewEtcdClientFromStore`（589）。`node dial.rs::DialEtcdClient` 给出到 `ConnectMetadataPD`、`NewEtcdClientFromPDClient` 与 `close` 的直接 trail；文件级调用边确认 `ResolveEtcdDialInfo -> get_pd_addrs/get_info`、`NewEtcdClientFromPDClient -> ResolveEtcdDialInfo`、`NewEtcdClientFromStore -> pd_client/keyspace_meta/NewEtcdClientFromPDClient`。
- crate 与模块边界：读取 `pkg/metaservice/Cargo.toml`、`pkg/metaservice/lib.rs`；目标包没有 `doc.go`。Cargo 声明确认 `etcd-client` TLS、Tokio 与带已发布 tag 的 `tikv-client` 依赖。
- 直接实现依赖：读取 `pkg/metaservice/etcd.rs` 的 `Context`、`PdClient`、`get_pd_addrs`，以及 `pkg/metaservice/metamanager.rs` 的 `MetaServiceError`、group 常量和 `get_info/get_group`。
- Go 对照：读取 `pkg/metaservice/etcd.go` 全文及 `pkg/metaservice/etcd_test.go` 的相关测试，核对 V1 PD、单次 metadata lookup、endpoint 优先级、namespace 与错误文本。
- Rust 测试：读取 `pkg/metaservice/dial_test.rs` 全文；覆盖空 endpoint 过滤、专属 group、PD 加载/关闭计数、无效 group，以及 opt-in real-etcd 的 namespace、lease、取消、关闭和所有权。
- 上游证据：局部读取 `br/pkg/task/common.rs`、`br/pkg/task/operator/crr_checkpoint.rs`、`lightning/pkg/importer/precheck_impl.rs`、`lightning/pkg/importer/import.rs`、`pkg/executor/importer/table_import.rs`、`pkg/store/driver/tikv_driver.rs` 与 `pkg/session/runtime/session.rs` 的直接调用/trait 实现位置。
- 本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰好包含上述 11 个固定二级标题，并人工复核所有“已支持”结论均能回指到上述源码、调用边或测试。
