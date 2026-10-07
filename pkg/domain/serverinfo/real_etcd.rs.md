# `pkg/domain/serverinfo/real_etcd.rs`

## 文件定位

[`real_etcd.rs`](real_etcd.rs) 是 `astersql-domain-serverinfo` crate 的生产 etcd 适配层：它把 [`syncer.rs`](syncer.rs) 定义的同步接口 `EtcdClient` 落到 `etcd-client 0.19`，让同步的 `Syncer` 代码可以访问 PD 内嵌 etcd。模块由 [`lib.rs`](lib.rs) 私有声明为 `real_etcd`，再通过 `pub use real_etcd::*` 导出 `RealEtcdClient`；因此外部 crate 使用的是 `astersql_domain_serverinfo::RealEtcdClient`，而不是直接访问模块路径。

生产接线可在 `pkg/session/runtime/session.rs` 的 Domain 初始化、`pkg/session/runtime/session_factory.rs` 的跨 keyspace transport，以及 `pkg/session/runtime/crossks_runtime.rs::target_real_etcd` 中看到。前两处还把同一连接的 `raw_client()` 和 `namespace()`交给 owner、TTL watch 或 `pkg/ddl/schemaver/syncer.rs::RealEtcdClient`，所以本类型既是 server-info 的 `EtcdClient` 实现，也是多个 Domain 元数据组件共享底层 etcd 连接的边界。

## 核心职责

- `RealEtcdClient::connect` 建立自有 Tokio runtime，配置 etcd 连接超时、gRPC keepalive 和可选双向 TLS，然后同步等待连接完成。
- `with_namespace`/`key` 给所有本接口发出的逻辑键统一增加 PD keyspace 前缀；`Get` 返回时再移除此物理前缀，使上层 `Syncer` 始终看到逻辑键。
- `EtcdClient` 实现提供 lease、精确/前缀读写、删除、租约撤销，以及状态端点 claim 所需的原子比较事务。
- `run_with_context` 在同步 API 与异步 etcd RPC 之间轮询 `Context::Done()`，形成可取消/有截止时间的传输边界。
- `GrantLease` 为每个租约启动后台 keepalive；`RevokeLease` 和 `Drop` 终止对应任务，避免后台任务脱离客户端生命周期。
- `raw_client` 与 `namespace` 暴露共享连接及其键空间策略，供 owner、DDL schema version 和 TTL watch 等需要原生 `etcd_client::Client` 的适配层复用。

## 主要符号

- `pub struct RealEtcdClient`：持有 `Arc<Runtime>`、受 `Mutex` 保护且可克隆的 `Client`、按 lease ID 索引的 `JoinHandle<()>` 表，以及不可变 `namespace`。
- `connect(endpoints, tls_files) -> Result<Self, SyncError>`：生产构造器。TLS 三元组依次是 CA、客户端证书、私钥路径；任一读取、runtime 创建或连接失败都会带阶段上下文返回。
- `new(client) -> Result<Self, SyncError>`：包装已经连接的原生客户端；调用者负责此前的连接、TLS 和 namespace 策略。本仓库主要共享连接路径使用的是 schemaver 中同名适配器，当前对该构造器未找到生产调用。
- `with_namespace(self, String) -> Self`：消费并返回客户端，把前缀设置为调用者给出的原始字符串；它不会自动添加或规范化斜杠。
- `key(&self, key)`、`client(&self)`：内部辅助函数，分别拼接物理键、在短暂持锁后克隆客户端句柄。
- `raw_client()`、`namespace()`：供其他 etcd 使用者复用连接并自行保持同一前缀约定。
- `run_with_context(runtime, context, future)`：每 20 ms 检查一次取消/截止时间；RPC 完成时把 `etcd_client::Error` 转成 `SyncError`。
- `impl EtcdClient for RealEtcdClient`：实现 `GrantLease`、`Get`、`Put`、`Delete`、`DeletePrefix`、`RevokeLease`、`CompareAndPut`、`TryCreateClaim`、`ReattachClaim`、`CompareAndDelete`。
- `impl Drop for RealEtcdClient`：排空 keepalive 表并 `abort` 全部任务；它不主动撤销 etcd lease，真正的键清理由上层 `Syncer::RevokeSession` 或 lease 到期负责。

## 执行流程

1. Domain 启动代码把 PD 内嵌 etcd endpoints 与可选 TLS 文件传给 `connect`。构造器创建两线程 Tokio runtime，设置 5 秒连接超时、10 秒 gRPC keepalive 间隔、3 秒 RPC 超时以及 idle keepalive，再同步执行 `Client::connect`。
2. keyspace 场景随后调用 `with_namespace`。例如 `session_factory.rs` 使用 `/keyspaces/tidb/{id}`；此后 `key` 直接执行 `namespace + logical_key`，因此命名空间格式是调用方必须维持的不变量。
3. `Syncer::NewSessionAndStoreServerInfo` 调用 `GrantLease(45)`。该方法先授予 lease，再启动循环：建立 lease keepalive pair，按 `max(ttl/3, 1s)` 发送 keepalive 并读取响应；建流、发送或收流失败后退出内层循环并重新建流。
4. `Syncer` 通过 `Put` 将 server info 绑定到该 lease；拓扑信息、陈旧登记清理与查询分别调用 `Put`、`Get`、`Delete`、`DeletePrefix`。`Get`、`Put`、`RevokeLease` 和三个 claim 事务经 `run_with_context`；`GrantLease`、两种普通删除与 `CompareAndPut` 则直接 `block_on`。
5. 状态端点登记由 `StatusEndpointClaim::acquire` 驱动。`TryCreateClaim` 以 `create_revision == 0` 原子创建；已有键时同一事务读取 value、lease、mod revision。若 ID 相同，`ReattachClaim` 仅在 value 与 mod revision 仍一致时把键换绑到新 lease；清理时 `CompareAndDelete` 仅在 value 与 lease 同时匹配时删除。
6. 正常关闭时 `Syncer::RevokeSession` 先关闭上层 session，再调用 `RevokeLease`；本实现先中止本地 keepalive，再请求 etcd revoke。客户端整体销毁时 `Drop` 中止所有尚存 keepalive。

## 数据与状态

`runtime` 用 `Arc` 保存，以便实例方法和后台任务共享 reactor；该 runtime 固定为两个 worker。`client` 放在 `Mutex<Client>` 中，但每次 RPC 前只在 `client()` 内持锁并克隆轻量客户端句柄，网络等待期间不持该锁。锁中毒被视为不可恢复的程序错误并 `expect` panic。

`keepalive: Mutex<HashMap<i64, JoinHandle<()>>>` 记录 lease 与后台任务的一一映射。插入同一 lease ID 会替换旧 handle，但正常 etcd lease ID 应唯一；代码没有显式 abort 被替换的旧 handle，因此扩展租约管理时不应人为复用 ID。`namespace` 在构造完成后只通过消费式 builder 设置，之后只读。

`Get` 把 etcd KV 映射成 crate 的 `KeyValue`：value 保留原始字节；零 lease 转成 `None`；key 用 UTF-8 损失转换后剥离 namespace。若响应键不以前缀开头，`strip_prefix(...).unwrap_or_default()` 会把逻辑键降为 `""`，而不是报协议错误。这是当前真实边界，调用者不能把空键当作已验证的 etcd 原键。

## 依赖与调用关系

crate 边界由 `pkg/domain/serverinfo/Cargo.toml` 给出：本文件直接使用 `etcd-client = 0.19` 的 `tls` feature，以及 Tokio 的 `rt-multi-thread`、`time`；接口类型、上下文、错误和状态 claim 观察值来自同 crate 的 `syncer.rs` 与 `status_endpoint_claim.rs`。`base64` 和日志依赖用于相邻模块，不由本文件直接使用。

上游生产调用者包括：

- `pkg/session/runtime/session.rs`：创建主 Domain server-info 客户端，并把 raw client/namespace 复用于 bootstrap owner lock 和 TTL watch。
- `pkg/session/runtime/session_factory.rs`：为目标 keyspace 创建 namespaced 客户端，并同时交给 server-info 与 DDL schemaver transport。
- `pkg/session/runtime/crossks_runtime.rs`：按 keyspace ID 延迟创建和缓存客户端。
- `pkg/ddl/schemaver/syncer.rs::RealEtcdClient::new`：通过 `raw_client`/`namespace` 共享连接，但自有 RPC runtime。

直接业务消费者是 `pkg/domain/serverinfo/syncer.rs::Syncer` 和 `pkg/domain/serverinfo/status_endpoint_claim.rs::StatusEndpointClaim`。前者使用租约和通用 KV 操作维护 server info、拓扑与陈旧 owner 信息；后者使用三种事务方法维护 advertised status endpoint 的单持有者约束。

RustCodeGraph 的文件索引报告本文件被 `pkg/domain/infosync/info.rs`、其测试、serverinfo 的 info/status-claim 等共 7 个文件引用，但针对 trait 实现方法的精确 callers/callees 查询未返回边；上述生产边均以仓库 `rg` 直接引用复核，而不是根据空图结果推断。

## 错误处理与边界

`connect` 对 runtime、TLS 三个文件和连接阶段分别添加 `start/read/connect` 文本，便于定位失败。多数 RPC 再加逻辑操作及未加 namespace 的 key，例如 `put etcd key {key}`；调用者看到的是业务键。`run_with_context` 自身只保留 etcd 原始错误字符串，外层方法按需补充操作上下文。

取消语义不完全统一：`Get`、`Put`、`RevokeLease` 和三个 claim 事务经 `run_with_context`，可在约 20 ms 粒度丢弃尚未完成的 future；`GrantLease`、`Delete`、`DeletePrefix`、`CompareAndPut` 只在发起前检查 `Context::Done()`，随后直接 `block_on`，RPC 进行中不会再响应该上下文。`TryCreateClaim` 与 `ReattachClaim` 没有单独的前置检查，但 `run_with_context` 会在启动轮询后检查。文档或新代码不能把所有方法描述为同样的全程可取消。

租约授予成功但 keepalive 建流持续失败时，后台任务会永久重试，接口仍已返回 lease ID；它不向上层暴露“保活已失效”信号。keepalive 响应只检查是否存在，没有验证响应 TTL。`Drop` 只 abort 本地任务、不等待任务完成也不 revoke lease；进程异常或客户端被丢弃后依赖 etcd TTL 回收。

claim 事务对响应形状做严格校验：失败分支必须恰有一个 `Get` 响应且含一个 KV，否则返回明确错误。`CompareAndPut` 的已有值分支同时比较 value 和 lease（缺省 lease 按 0 比较）；不存在分支比较 create revision 为 0。所有事务失败都返回 `Ok(false)`，只有传输/响应错误才返回 `Err`。

## 并发与资源生命周期

`RealEtcdClient` 满足 `EtcdClient: Send + Sync`：原生 `Client` 通过短临界区克隆，keepalive 表由独立互斥锁保护，namespace 初始化后不变。多个同步调用可以从不同线程进入；它们的 future 都提交到同一两线程 runtime，并由各调用线程执行 `Runtime::block_on`。增加长耗时同步逻辑时应注意 worker 数固定以及在 Tokio runtime 内再次 `block_on` 的调用约束。

每次 `GrantLease` 产生一个长生命周期 Tokio task。任务外层在建流失败后等待一个 keepalive interval 再试；内层先 sleep 一个 interval，再发送 keepalive并等待一条响应，失败后回到外层重建。`RevokeLease` 在发送 revoke 前从 map 移除并 abort 任务，即使 context 已取消也不会恢复 keepalive；因此 revoke 失败后该 lease 只能依赖 TTL 到期。`Drop` drain map，确保 handle 不再留在容器中。

claim 并发安全依赖 etcd transaction，而非本地锁：首次创建以 create revision CAS 决胜；同 ID 重启以 value + mod revision 防止观察后被第三方改写；删除以 value + lease 防止旧 generation 或冲突方删除新持有者。`syncer_test.rs` 的 real-etcd ignored 测试和 `syncer_test.go::TestStatusEndpointClaim` 都覆盖了命名空间隔离、并发竞争、同 ID 换租约、旧租约清理失败与 revoke 联动删除。

## 与 Go 版本的对应关系

Go 目录没有单独的 `real_etcd.go`：Go `pkg/domain/serverinfo/syncer.go` 直接持有 `*clientv3.Client`，由 `concurrency.Session`、`util.PutKVToEtcd` 等调用原生 client。Rust 为了让同步器可测试且兼容同步 API，额外抽出了 `EtcdClient` trait 和本文件这一生产实现；所以应按“操作语义”而不是按文件一一对应。

通用行为对应关系为：Rust `GrantLease` + keepalive map 对应 Go `tidbutil.NewSession`/`concurrency.Session`；Rust `Get`/`Put`/删除方法对应 Go clientv3 KV 调用及 `util.PutKVToEtcd`；Rust namespace 的物理键拼接/读回剥离对应 Go namespaced client 的键空间包装。

claim 语义可直接与 `pkg/domain/serverinfo/status_endpoint_claim.go` 核对：`TryCreateClaim` 对应 `tryCreate` 的 create-revision transaction；`ReattachClaim` 对应 `reattach` 的 value + mod-revision CAS；`CompareAndDelete` 对应 `remove` 的 value + lease CAS。两边都把 claim 冲突和检查失败保持为 warning-only，不阻止 server info 注册。Go 集成测试 `syncer_test.go::TestStatusEndpointClaim` 是完整语义基准；Rust `syncer_test.rs::status_endpoint_real_etcd_transactions_namespace_and_lease_cleanup` 是同类真实 etcd 覆盖，但被 `#[ignore]` 标记并要求 `ASTERSQL_TEST_ETCD_ENDPOINT`。

一个需要明确保留的差异是取消/超时实现：Go RPC 原生接收 `context.Context`；Rust `Context` 是 crate 内简化类型，本文件以 20 ms timeout 轮询桥接，且部分方法只做发起前检查。修改时应优先向 Go 的可取消边界靠齐，不能假定当前实现已经完全等价。

## 扩展指南

- 新增通用 etcd 能力时，先在 `syncer.rs::EtcdClient` 定义语义，并同步实现 `MemoryEtcdClient`、本文件 `RealEtcdClient` 及测试中的故障/代理客户端；Rust 单元测试继续放在独立的 `syncer_test.rs`，不要内嵌进生产文件。
- 新增原子状态变更时优先用一个 etcd `Txn` 表达完整不变量；涉及“观察后更新”时同时比较 mod revision，涉及清理时比较 value 和 lease，避免 ABA、旧 generation 和误删赢家。
- 新增网络 RPC 应决定是否需要全程取消。若需要，应经 `run_with_context` 或统一改造后的等价 runner；同时为取消前、进行中取消、父截止时间写独立测试。不要照搬当前 `Delete`/`CompareAndPut` 的仅前置检查而不记录理由。
- 调整 namespace 时必须同时审计 `key`、`Get` 的剥离逻辑以及所有 `raw_client` 调用者；原生客户端不会自动应用本类型的前缀，调用者必须显式拼接 `namespace()`。需覆盖空 namespace、有/无尾斜杠、两个 keyspace 隔离。
- 修改 lease 生命周期时同时检查 `GrantLease`、`RevokeLease`、`Drop` 和 `Syncer::{RevokeSession,Restart}`；重点风险是授予后保活失败、revoke RPC 失败、重复 lease handle、客户端 drop 后短暂残留键。
- 行为对齐应同步检查 Go `syncer.go`、`status_endpoint_claim.go` 和 `syncer_test.go::TestStatusEndpointClaim`。真实 etcd 行为应扩展 Rust 的 ignored 集成测试，内存语义则扩展 `MemoryEtcdClient` 相关独立测试。
- 性能上避免在 `client`/`keepalive` 锁内执行网络等待；若提高并发或增加 watch/stream，应评估固定两 worker runtime、同步 `block_on` 调用和每 lease 一个任务的成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/domain/serverinfo` 找到 `real_etcd.rs` 及相邻 Rust/Go 文件；`node --file pkg/domain/serverinfo/real_etcd.rs` 读取完整 406 行；`query RealEtcdClient`、`query run_with_context`、`query TryCreateClaim`、`query CompareAndPut` 核对类型与签名。精确 callers/callees 对这些 trait 实现未返回边，未据此声称不存在调用者。
- 目标与 crate：`pkg/domain/serverinfo/real_etcd.rs`、`lib.rs`、`Cargo.toml`。
- Rust 接口与调用：`pkg/domain/serverinfo/syncer.rs`、`status_endpoint_claim.rs`、`pkg/session/runtime/session.rs`、`session_factory.rs`、`crossks_runtime.rs`、`pkg/ddl/schemaver/syncer.rs`。
- Rust 测试：`pkg/domain/serverinfo/syncer_test.rs::normal_schema_barrier_transport_cancellation_and_parent_deadline` 验证传输 future 不越过取消/父截止时间；`status_endpoint_real_etcd_transactions_namespace_and_lease_cleanup` 验证真实 etcd 的 namespace、claim CAS、换租约及清理，且测试明确标为需外部 fixture 的 ignored 测试。
- Go 对照：`pkg/domain/serverinfo/syncer.go`、`status_endpoint_claim.go`、`syncer_test.go::TestStatusEndpointClaim`；后者覆盖冲突仅告警、并发单赢家、同 ID 重启、重挂竞态、所有权安全删除、revoke 和 namespace 隔离。
- 本任务是纯文档分析，按计划不运行 Cargo，也未执行真实 etcd ignored 测试；结论来自已索引源码、直接引用搜索与现有测试断言。交付前以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复查没有把 ignored 测试写成已实际运行。
