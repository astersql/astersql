# `pkg/autoid_service/client.rs`

## 文件定位

`client.rs` 是 `astersql-autoid_service` crate 的生产客户端适配层，并由 `lib.rs` 以 `pub mod client` 暴露。它不实现 AutoID 的分配算法，也不维护表级 ID 水位；它把 `astersql-meta-autoid`（在本 crate 的 `Cargo.toml` 中别名为 `autoid_dependency`）定义的 `LeaderDiscovery`、`AutoIdClientConnector`、`AutoIdClient` 和 `ClientConnection` 四个抽象，落到 etcd、grpcio 与 kvproto 的真实实现上。

当前应用接线位于 `pkg/session/runtime/create_table_resources.rs::requirement`：非 `mock-storage` 路径读取 PD/etcd endpoints、集群 TLS 文件和 keyspace namespace，调用本文件的 `client_discover`，再把得到的 `Arc<ClientDiscover>` 放入建表所需的 `Requirement`。mock-storage 路径使用内存客户端并绕过本文件的真实网络实现。

## 核心职责

- `Discovery::leader` 在带 namespace 的 etcd 前缀下查找最早创建的 AutoID Leader 注册项，并把首个键值的 value 解码为 gRPC 地址。
- `Connector::connect` 按 `ClientTls` 构造安全或非安全的 grpcio channel，同时返回 RPC 客户端与可单独关闭的连接句柄。
- `RpcClient::{alloc_auto_id,rebase}` 在仓库内部请求/响应类型与 `kvproto::autoid` protobuf 类型之间做字段级转换，并给每次 RPC 设置 30 秒超时。
- `Connection::close` 释放本适配层保留的 channel 引用，供 `ClientDiscover::reset_conn` 的延迟关闭流程使用。
- `client_discover` 一次性组装单线程 Tokio runtime、etcd client、单线程 grpcio environment、发现器和连接器，形成可由上层缓存、重连和重试的 `ClientDiscover`。

因此，本文件负责“发现和传输”，而连接缓存、版本化重置、Leader 未产生时的退避、RPC 失败重试及业务错误解释均在 `pkg/meta/autoid/autoid_service.rs::ClientDiscover` 与 `SinglePointAllocator` 中完成。

## 主要符号

- `fn err(e: impl ToString) -> AutoIdError`：内部统一错误映射器。etcd、grpcio、UTF-8、运行时构造和锁中毒错误都被折叠为 `AutoIdError::Storage(e.to_string())`。
- `pub struct ClientTls { ca, cert, key }`：公开 TLS 字节容器。`ca` 用作根证书；`cert` 或 `key` 任一非空时，代码会把两者一起作为客户端身份材料交给 etcd 与 grpcio。该类型可克隆并有空字节向量默认值。
- `struct Discovery`：持有一个 Tokio runtime、受 `Mutex` 保护的 `etcd_client::Client` 和精确 namespace。实现 `LeaderDiscovery`，但不公开具体类型。
- `struct Connector`：持有共享的 grpcio `Environment` 和可选 TLS 材料，实现 `AutoIdClientConnector`。
- `struct RpcClient`：封装 kvproto 生成的 `AutoIdAllocClient`，实现内部 `AutoIdClient` trait。
- `struct Connection(Mutex<Option<grpcio::Channel>>)`：channel 生命周期句柄；`Option::take` 让关闭操作幂等。
- `pub fn client_discover(endpoints, tls, namespace)`：本文件唯一公开构造入口，返回 `Arc<ClientDiscover>`。

本文件没有条件编译的生产分支；末尾 `#[cfg(test)] #[path = "client_test.rs"] mod tests` 仅把独立测试文件挂入测试构建，生产逻辑与测试逻辑没有写在同一个文件中。

## 执行流程

1. `pkg/session/runtime/create_table_resources.rs::requirement` 从 storage 取得 `DDLPDEndpoints` 和 `DDLKeyspaceID`。有 CA 配置时读取 CA、可选证书和私钥；非 nullspace 的 namespace 形如 `/keyspaces/tidb/{keyspace}`。
2. `client_discover` 建立只有一个 worker 的 Tokio multi-thread runtime；etcd 连接参数设置 5 秒连接超时、30 秒请求超时，并在启用 TLS 时配置 CA 及可选客户端身份。
3. 该函数同步 `block_on(Client::connect(...))` 完成初始 etcd 建连，然后用同一 runtime 和互斥 etcd client 构造 `Discovery`；另建一个单线程 grpcio `Environment` 构造 `Connector`，两者交给 `ClientDiscover::new`。
4. 上层 `ClientDiscover::get_client` 首次缺少缓存客户端时，根据 keyspace 生成 Leader 路径并调用 `Discovery::leader`。后者先执行 `Context::check`，再以 `namespace + path` 为查询前缀，按 create revision 升序且 limit 1 获取首项；没有键时返回 `Ok(None)`，有键时把 value 作为 UTF-8 地址返回。
5. 上层在没有 Leader 时自行退避重试；拿到地址后调用 `Connector::connect`。连接器按 TLS 配置建立 grpcio channel，以 channel 构造 kvproto `AutoIdAllocClient`，并把 channel 的另一引用封装为 `Connection`。
6. 分配请求进入 `RpcClient::alloc_auto_id`：先检查上下文，将 database/table、数量、increment/offset、unsigned 标记和 keyspace ID 全量写入 wire request，执行最长 30 秒的 `alloc_auto_id_opt`，再返回 `(min, max]` 边界及 UTF-8 `errmsg`。
7. 重置请求进入 `RpcClient::rebase`：同样先检查上下文，转换 database/table、base、force 和 unsigned 字段，执行最长 30 秒的 `rebase_opt`，再解码 `errmsg`。
8. 当上层因 RPC 错误重置缓存时，`ClientDiscover::reset_conn` 先移出连接，并在线程中等待 200ms 后调用 `Connection::close`；本实现取走保存的 channel 引用。下一次 `get_client` 将重新查 Leader 和建连。

## 数据与状态

本文件不保存 AutoID 水位或批次范围。`AutoIdRequest`/`AutoIdResponse` 的业务状态由 `pkg/meta/autoid/autoid_service.rs` 定义，其中成功分配区间语义为 `(min, max]`；本文件只保持字段不丢失地往返转换。响应 `errmsg` 是服务端业务状态，不会在本文件内自动提升为 `Err`，而由 `SinglePointAllocator::{alloc_inner,rebase_inner}` 检查并转换为服务错误。

持久的进程内状态有三类：`Discovery.runtime` 驱动 etcd 异步调用；`Discovery.client` 是被互斥保护的共享 etcd client；`Connector.env` 是所有新 gRPC channel 共享的 grpcio environment。每个已连接客户端还包含一个 kvproto stub 和一个 `Connection` 中的可取走 channel 引用。`namespace` 在构造后不变，并直接与上层生成的 Leader path 拼接，不做斜杠正规化。

`ClientTls` 同时供 etcd/rustls 风格 API 和 grpcio credential builder 使用，二者都复制证书字节。当前判断条件是“cert 或 key 任一非空就配置 identity”，并不在本文件预先验证两者必须成对或 PEM 有效；验证失败会由底层连接栈报告。

## 依赖与调用关系

上游生产调用边为：

`pkg/session/runtime/create_table_resources.rs::requirement` → `client_discover` → `ClientDiscover::new`。

后续运行调用由 `pkg/meta/autoid/autoid_service.rs` 驱动：`ClientDiscover::get_client` → `Discovery::leader` / `Connector::connect`；`SinglePointAllocator::alloc_inner` → `RpcClient::alloc_auto_id`；`SinglePointAllocator::rebase_inner` → `RpcClient::rebase`；`ClientDiscover::reset_conn` → `Connection::close`。

直接依赖及用途如下：

- `autoid_dependency`（`astersql-meta-autoid`）：提供内部 trait、上下文、请求/响应、错误和 `ClientDiscover` 状态机。
- `etcd-client`（启用 `tls` feature）：连接 PD 暴露的 etcd endpoints，并按 prefix/create order 发现 Leader。
- `grpcio`（禁用默认 feature，启用 `openssl-vendored`、`protobuf-codec`）：建立传输 channel、配置 TLS 和 RPC deadline。
- `kvproto`（固定 Git tag `v0.0.2-aster.20260929`，启用 `protobuf-codec`）：提供 AutoID wire request、生成客户端和 keyspace oneof。
- `tokio`（`rt-multi-thread`、`time`）：为异步 etcd client 提供本文件私有 runtime。

crate 根 `pkg/autoid_service/lib.rs` 公开 `client` 模块；`Cargo.toml` 将 Go 对照包标记为 `pkg/autoid_service`，但该目录没有同名 `client.go`，真实的 Go 客户端发现、RPC 建连与分配调用集中在 `pkg/meta/autoid/autoid_service.go`。

## 错误处理与边界

- 所有公开入口和 trait 方法均返回 `autoid_dependency::Result`。本文件的底层错误统一变成 `AutoIdError::Storage`，因此不会保留 etcd/grpcio 的结构化错误类别；上层只对 `AutoIdError::Rpc` 走特定 RPC 重试，而这一适配层的 `map_err(err)` 结果属于 Storage。扩展错误映射时必须核对该重试契约。
- `Discovery::leader` 和两个 RPC 方法在发起 I/O 前调用 `Context::check`；但传入 grpcio 的只有固定 30 秒 `CallOption`，内部 `Context` 的取消信号不会作为 grpcio cancellation token 传递给已经发出的调用。
- etcd 查询无匹配项是正常的 `Ok(None)`，由上层退避；锁中毒、etcd 请求失败、Leader value 非 UTF-8 则立即返回错误。
- `Connector::connect` 的 grpcio `connect` 是通道构造，不在本方法内等待握手完成，因此很多网络或证书错误可能在首次 RPC 时才显现。
- `alloc_auto_id` 明确写入 keyspace oneof；`rebase` 的 wire 类型没有在此设置 keyspace 字段。该差异与当前 kvproto 接口及 Go 调用保持一致，不应自行补字段。
- 服务端返回的 `errmsg` 必须是 UTF-8；非法字节会成为传输层 `Err`，空字符串才代表本层观察到的无业务错误响应。
- `Connection::close` 对已经关闭的句柄再次调用仍返回 `Ok(())`。若内部互斥锁中毒则返回错误，且不会恢复锁内 channel。

## 并发与资源生命周期

`ClientDiscover` 可被多个 allocator/session 以 `Arc` 共享，其上层 `RwLock` 和连接版本控制负责防止重复建连和过期错误清除新连接。本文件的 `Discovery` 通过 `Mutex<Client>` 串行进入 etcd client，再由专属 Tokio runtime 同步 `block_on`；因此同一 discovery 的 Leader 查询不会并行持锁执行。不得从该 runtime 自身的 worker 上调用 `leader` 并形成嵌套阻塞，否则需要重新评估 runtime 的 `block_on` 使用方式。

grpcio `Environment` 由 `Arc` 共享且只有一个线程。每次 Leader 建连会创建一个 channel；`RpcClient` 与 `Connection` 各持有 channel 相关引用。上层重置先停止发布客户端，再等待 200ms 关闭连接，目的是给进行中的 RPC 留出收尾窗口；本文件仅通过 `Option::take` 丢弃连接句柄，不负责 join 后台线程。最后一个 `ClientDiscover`/相关客户端被释放时，私有 Tokio runtime、etcd client、grpcio environment 和剩余 channel 按 Rust 所有权顺序销毁。

证书和私钥以 `Vec<u8>` 常驻 `ClientTls`，并在构造两个传输栈时克隆；本文件没有主动清零敏感字节。若要调整线程数、复用 runtime 或加强密钥生命周期，需同时评估发现串行性、grpcio 回调负载和进程级资源占用。

## 与 Go 版本的对应关系

Go 的对应实现不在 `pkg/autoid_service` 下的同名文件，而在 `pkg/meta/autoid/autoid_service.go`：`ClientDiscover.GetClient` 用 etcd `WithFirstCreate` 查 `GetAutoIDServiceLeaderEtcdPath`，建立 gRPC client 并缓存连接；`singlePointAlloc.alloc` 与 rebase 路径组装同一组 protobuf 字段；`ResetConn` 清缓存并延迟 200ms 关闭连接。Rust 将这些职责拆成 `pkg/meta/autoid/autoid_service.rs` 的策略层和本文件的真实 I/O 适配层。

已对齐的关键语义包括：Leader key 按最早 create revision 选择；nullspace 使用 `tidb/autoid/leader`，keyspace 路径带前导 `/`；分配请求包含 keyspace oneof；返回范围为 `(min, max]`；RPC 错误后由发现器版本化重置连接；旧连接延迟关闭。`pkg/autoid_service/client_test.rs::normal_ddl_plan_create_table_autoid_rpc_rebase_and_allocate` 还验证了真实 grpcio 服务上的 rebase 110 后分配一个 ID 得到 `(110, 111)`，并显式关闭连接与服务。

可见差异是 Go 直接持有 `clientv3.Client` 和 `grpc.ClientConn`，并从全局配置内部取得 TLS；Rust 由会话资源接线显式传入 endpoints/TLS/namespace，并以 trait 拆分发现、连接、RPC 和关闭。Go 会记录连接/reset 日志与指标；本文件自身不记录日志或指标。Rust 的每个 RPC 固定 30 秒 deadline，而 Go 主要依赖传入 context。上述差异是当前源码事实，不代表功能等价性之外的设计承诺。

## 扩展指南

- 新增或改变 AutoID RPC 字段时，应先修改 `autoid_dependency` 中的内部请求/响应，再同步 `RpcClient` 的双向转换，并在独立的 `pkg/autoid_service/client_test.rs` 增加真实 grpcio 往返断言；不要把测试嵌入 `client.rs`。
- 改变 Leader 注册路径或 keyspace namespace 时，必须联合检查 `get_auto_id_service_leader_etcd_path`、`create_table_resources.rs::requirement` 与 `Discovery::leader` 的字符串拼接，覆盖 nullspace 和非 nullspace，防止多/少一个 `/` 或跨 keyspace 查询。
- 增强 TLS 校验时，优先在 `client_discover` 进入底层 builder 前验证 CA、cert/key 成对关系，并同时覆盖 etcd 与 grpcio 两条配置路径。兼容风险是现有只配置 CA 的单向 TLS，性能风险是重复解析/复制证书。
- 调整错误分类时，应同步检查 `SinglePointAllocator::retry_rpc` 只针对 `AutoIdError::Rpc` 的分支；错误类别决定是否重连重试，错误映射不当会把瞬时故障变成直接失败，或把永久配置错误变成重试风暴。
- 调整超时/取消时，要区分 etcd 的 5 秒 connect/30 秒 request timeout、grpcio 的 30 秒 call timeout和上层 `Context`。建议新增取消前、I/O 中取消、deadline 到期三个独立测试场景。
- 调整并发模型或资源复用时，应保持 `Connection::close` 幂等、旧连接不立即影响 in-flight RPC，以及 `ClientDiscover` 双检锁只建立一个已发布客户端的不变量。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/autoid_service` 确认目标、模块入口及测试均在索引中。
- RustCodeGraph 源码/符号查询：`node --file pkg/autoid_service/client.rs`；`node client_discover`；`node client.rs::leader`、`client.rs::connect`、`client.rs::alloc_auto_id`、`client.rs::rebase`、`client.rs::close`。索引同时报告目标文件由 `pkg/session/runtime/create_table_resources.rs` 使用。
- 生产接线：`pkg/session/runtime/create_table_resources.rs::requirement`，核对 endpoints、TLS 文件、namespace、mock 分支和 `client_discover` 调用。
- trait 与策略层：`pkg/meta/autoid/autoid_service.rs`，核对 `LeaderDiscovery`、`AutoIdClientConnector`、`AutoIdClient`、`ClientConnection`、`ClientDiscover::{get_client,reset_conn}` 以及 `SinglePointAllocator::{alloc_inner,rebase_inner}`。
- crate 边界：`pkg/autoid_service/Cargo.toml` 与 `pkg/autoid_service/lib.rs`，核对模块公开方式、依赖 feature、kvproto tag 和 Go package metadata。
- Go 对照：`pkg/meta/autoid/autoid_service.go` 的 `NewClientDiscover`、`GetClient`、`singlePointAlloc.alloc`、rebase 请求和 `ResetConn`；`pkg/domain/domain.go` 的 Go 侧 `NewClientDiscover` 接线。
- 独立测试：`pkg/autoid_service/client_test.rs::normal_ddl_plan_create_table_autoid_rpc_rebase_and_allocate`。该测试覆盖非 TLS gRPC 通道、rebase、alloc、响应范围与显式关闭；当前没有覆盖 etcd Leader 查询、TLS、非法 UTF-8、取消或锁中毒的本文件专属测试。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证仅包括固定章节结构命令、链接/路径核对和人工事实复查。
