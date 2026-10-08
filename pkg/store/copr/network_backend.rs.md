# `pkg/store/copr/network_backend.rs`

## 文件定位

[`network_backend.rs`](network_backend.rs) 是 `astersql-store-copr` crate 中的标准 TiKV DAG 网络后端。它弥补 `tikv-client = 0.4.2` 没有公开的 Coprocessor transport：向 PD 查询 Region、leader、Store 和 keyspace 元数据，通过 `tikvpb.Tikv` gRPC 发送 unary、server-streaming 和 store-batch Coprocessor RPC，再把 protobuf 响应还原为 crate 内部的 `CopProtocolResponse`。模块由 `pkg/store/copr/lib.rs` 公开并重导出，crate 边界由 `pkg/store/copr/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认。

生产接线在 `pkg/store/driver/tikv_driver.rs` 的 TiKV store 打开流程：它用 PD 地址、TLS、keyspace 和超时构建 `NetworkConfig`，注入 `OfficialTransactionLockResolver`，调用 `NetworkBackend::connect`，然后以 `Arc<dyn StoreBackend>` 交给 `Store::new`。因此本文件处在 Rust SQL 执行端 Coprocessor 迭代器与 PD/TiKV RPC 之间，不是 TiKV 事务客户端、TSO 或 SafePoint 的实现。

## 核心职责

- `NetworkPdKeyspaceClient` 提供一个可独立复用的 PD keyspace 查询客户端：先通过 `GetMembers` 探测非零 cluster ID，再向所有给定端点尝试 `LoadKeyspace`，并将 PD header 错误分类为 `NotBootstrapped`、`KeyspaceNotExist` 或 `Unexpected`。
- `NetworkBackend` 组合可注入的 `RegionMetadataTransport`、`StandardCoprocessorTransport` 和 `TransactionLockResolver`，同时实现 `RegionCacheBackend` 与 `StoreBackend`。它在每次 Coprocessor 发送前按 Region ID 重新定位真实地址和 peer，维护关闭状态，并在 Region 错误或 transport 错误后通知元数据边界失效路由。
- `GrpcRegionMetadataTransport` 把内部 key range 转为 PD 的 memcomparable Region key，实现 `ScanRegions`、`GetRegion`、`GetPrevRegion`、`GetRegionById` 和 `GetStore`，并把 leader、epoch、bucket 和 Store label 组装成 `KeyLocation`。
- `KeyCodec` 处理 API V1 的原始逻辑键和 API V2 的 `x + 24-bit keyspace id` 前缀；Region 查询再外层使用 8 字节分组的 memcomparable 编码。它也对响应 range、bucket 边界和锁中的 key/primary/secondaries 做逆向解码。
- `GrpcStandardCoprocessorTransport` 按 TiKV 地址缓存 `TikvClient`，构建完整 `kvrpcpb::Context` 和 `coprocessorpb::Request`，发送 RPC，并由 `pb_response` 提取数据、Region/lock/other error、paging range、batch child 结果、扫描量、读字节、CPU 和 read-pool 统计。

## 主要符号

- 配置与 PD keyspace：`NetworkSecurity { ca_path, cert_path, key_path }`、`NetworkConfig { pd_endpoints, security, keyspace_name, timeout }`、`PdKeyspaceErrorKind`、`PdKeyspaceError`、`NetworkPdKeyspaceClient::{connect, load_keyspace, load_keyspace_meta}`。`DEFAULT_RPC_TIMEOUT` 为 60 秒。
- 可注入边界：`RegionMetadataTransport` 抽象 Region/Store 定位；`StandardCoprocessorTransport` 抽象 batch/unary/stream RPC 及关闭；`CoprocessorResponseStream` 抽象逐包读取和取消；`TransactionLockResolver` 抽象事务锁解析。`from_transports` 是不经真实网络的测试/适配入口。
- 请求与锁：`StandardCoprocessorRequest` 绑定最终地址、Region epoch、peer 和 `CopWireRequest`；`TransactionLock` 是 protobuf runtime 中立的完整锁字段镜像，`From<&kvrpcpb::LockInfo>` 保留 async-commit、for-update TS、secondaries 等字段。
- 主后端：`NetworkBackend::{connect, from_transports, located_request, timeout_for, timeout_for_wire, serial_batch_timeout, send_coprocessor_stream}`，以及它的 `RegionCacheBackend`/`StoreBackend` 实现。`NetworkRegionBackend` 是只保留 metadata 的独立 Region 适配器，避免 `Store` 持有整个 transport 循环。
- 网络实现：`ChannelFactory`、`TlsMaterial`、`GrpcRegionMetadataTransport`、`GrpcStandardCoprocessorTransport`、`GrpcCoprocessorResponseStream`。`normalize_endpoint` 去掉 HTTP(S) scheme 和尾部斜杠；`pd_header`/`check_pd_header` 统一 PD header；`transport_error` 将外部错误收敛为 `BatchError::Transport`。
- 键与响应转换：`KeyCodec::{v1, v2, encode_key, encode_end_region_key, encode_range, decode_range, decode_boundary, decode_lock_info, encode_region_range, decode_region_range}`，`mem_encode`/`mem_decode`，`pb_key_range`/`key_range`，`lock_bytes`，`pb_response` 及一组 `response_*` 统计辅助函数。

## 执行流程

1. 连接时，`NetworkBackend::connect` 创建两线程 grpcio `Environment`，`ChannelFactory::new` 可选读取 TLS 文件，`GrpcRegionMetadataTransport::connect` 遍历 PD 端点执行 `GetMembers`。如指定 keyspace，它继续 `LoadKeyspace` 并构建 V2 `KeyCodec`；否则使用 V1 codec。然后共享 channel factory 和 codec 创建 gRPC Coprocessor transport。
2. 任务构建时，上层 `Store`/`RegionCache` 通过 `RegionCacheBackend` 调用 metadata transport。单键、结束键、Region ID 和批量 range 分别转为 PD RPC；`GrpcRegionMetadataTransport::location` 验证 leader，补查 Store，解码 Region/bucket 边界，返回 `KeyLocation`。
3. unary 发送时，`StoreBackend::send_coprocessor` 先由 `located_request` 按 task Region ID 定位。如 task 的 epoch 与 PD 结果不同，它先通知旧 Region 失效；缺少 Store 地址则返回 `MissingRegion`。每次真实尝试都按 store ID 获取 `attempt_limiter` permit，记录 `ReadStats`，并在调用结束时释放 permit。
4. 无显式 `client_read_timeout` 时使用 60 秒默认超时。有显式超时时，`read_replicas` 使 leader 优先排序，按副本逐一尝试；只有识别为 `deadline exceeded` 的错误会继续下一副本，全部超时后回到初始 leader 使用 60 秒。串行 store-batch 请求的 wire timeout 为基础值乘以 `tasks.len() + 1`，使整个串行批次有足够时间。
5. `GrpcStandardCoprocessorTransport::protobuf_request` 写入 Region/epoch/peer、优先级、隔离级别、replica/stale/retry 标志、执行上限、bucket version、resource group、lock hints、keyspace 上下文、DAG/Analyze/Checksum 类型、paging 和 store-batch child。它在地址缓存中取 `TikvClient` 后发送 `coprocessor`、`coprocessor_stream` 或 `batch_coprocessor`。
6. `pb_response` 同时处理父响应与 `StoreBatchTaskResponse`，通过 task ID 建立 child map 和 Region-error/locked 集合，解码锁与 range，并从新旧 exec details 提取扫描、读字节、response bytes、CPU 和 read-pool 统计。上层 `coprocessor.rs` 再依据这些字段执行 Region 重建、锁解析、paging 和 runaway 判定。
7. 流式路径返回 `GrpcCoprocessorResponseStream`，`next` 同步等待下一个 gRPC packet，流结束后返回 `None`；`close` 取消 receiver，`Drop` 保证遗漏的流也会关闭。
8. 收到锁时，`resolve_lock` 解码 protobuf。普通 `LockInfo` 转为一个 `TransactionLock`；shared-lock wrapper 本身不代表事务，因此展开其 `shared_lock_infos` 并以同一 caller start TS 一次交给注入的 resolver。

## 数据与状态

`NetworkBackend` 只持有三个 trait object、可选 `ClientEventListener` 的 `Mutex` 和 `AtomicBool closed`。目前 listener 仅由 `set_event_listener` 存储，本文件的发送路径没有触发它。`GrpcRegionMetadataTransport` 保存 PD client 列表、cluster ID、timeout 和 codec，不保存 Region 条目；所以其 `invalidate_region` 是 no-op，后续调用天然重查 PD。真正的唯一 Region 缓存在上层 `Store/RegionCache`，与文件顶部的设计声明一致。

`GrpcStandardCoprocessorTransport` 用 `Mutex<HashMap<String, TikvClient>>` 缓存每个地址的 client，并用独立 `AtomicBool` 拒绝关闭后的新 client 请求。`close_address` 只删除指定地址；`close` 标记关闭并清空全部 client。`NetworkBackend::close_client` 用 `swap` 保证底层 close 最多调用一次。

`KeyCodec` 不可变，保存 keyspace 名、可选 24-bit ID、物理前缀和下一 keyspace 起点。V2 空 end key 必须编码为下一前缀，解码时再还原为逻辑无上界，以防扫描跨越 keyspace。`CopProtocolResponse` 中的 `batch_responses`、`batch_region_errors` 和 `batch_locked` 保留子任务级部分成功/失败状态，上层不必因一个 child 失败重做整批。

## 依赖与调用关系

上游主链是 `pkg/store/driver/tikv_driver.rs::open` -> `NetworkBackend::connect` -> `Store::new` -> `pkg/store/copr/store.rs::send` -> `StoreBackend::send_coprocessor` -> gRPC transport。`pkg/store/copr/coprocessor.rs` 的 worker 调用 `CopBackend::send`，并在 `resolve_response_lock` 中调用 backend `resolve_lock`；Region 错误和 bucket version 则通过 `invalidate_region`/`update_buckets` 回到 `RegionCache`。RustCodeGraph 将 `send_coprocessor` 连到 `coprocessor.rs::send` -> `build_cop_iterator` -> `check_store_batch_coprocessor` 的执行链。

下游直接依赖来自 `Cargo.toml`：`grpcio` 提供 channel、unary 和 streaming RPC，`kvproto`/`protobuf 2.8` 提供 PD、keyspace、TiKV 消息与 client stub，`futures-executor`/`futures-util` 用于同步拉取 gRPC stream，`tokio` 用于 failpoint 延迟路径，`fail` 注入 `tikvclient/mockBatchClientSendDelay`，`astersql-config-kerneltype` 决定 NextGen 读字节计费口径。`tikv-client` 不负责这里的 DAG RPC，但生产端注入的锁 resolver 和 `ReadStats` 等仍与它协作。

`NetworkPdKeyspaceClient` 还被 `pkg/session/runtime/session_factory.rs`、`session.rs`、`crossks_runtime.rs`、`pkg/store/driver/kv_adapter.rs` 和 `pkg/util/metricsutil/common.rs` 用于 keyspace 元数据查询；它与 `NetworkBackend` 共用 TLS/channel/PD header 思路，但不共享连接实例或 Region 状态。

## 错误处理与边界

- PD 或 gRPC 连接错误统一映射到 `BatchError::Transport`；空 PD 端点、cluster ID 为 0、缺少 keyspace/Store/Region 数据都显式失败。`NetworkPdKeyspaceClient` 额外保留可用于上层重试决策的 PD 错误类别。
- PD 端点按输入顺序遍历；RPC transport 错误会继续下一端点，而成功返回但 header 含错误时通常立即返回该协议错误。`batch_locate_key_ranges` 只以所有输入 range 的最小 start/最大 end 做一次 `ScanRegions`，上限固定为 10,240，不分页；这是当前实现边界。
- `KeyCodec::v2` 拒绝超过 24 bit 的 keyspace ID。解码会拒绝 keyspace 之外的 Region/range/lock key；`mem_decode` 检查不完整分组、非零 padding 和结束组后多余字节。错误的锁 protobuf 被标记为 `OtherResponse`。
- `send_coprocessor` 对 Region error 保留协议响应供上层决定重试，同时失效旧 Region；transport 错误也失效 Region。只有 deadline-exceeded 类错误触发显式 read-timeout 副本轮询，其他结果立即停止。
- `send_request` 在发送前和每个 batch response 后检查 `CancellationToken`；它把 payload 解析为 protobuf `BatchRequest`。`StandardCoprocessorTransport::send_batch` 的默认实现是明确的“unavailable”错误，而非伪成功。
- 当前标准 DAG backend 明确不提供 MPP：`dispatch_mpp`、`cancel_mpp`、`establish_mpp` 都返回 `OtherResponse`。`NetworkBackend` 自身的 TiFlash/compute/topology/TiDB address 查询返回空；`NetworkRegionBackend::tiflash_rpc_context` 仅能从 PD peers 中挑选 `engine=tiflash` 的 Store，其他 TiFlash 列表功能仍为空。`check_visibility` 目前也是 `Ok(())` 门面，不能解读为真正的 GC SafePoint 验证。

## 并发与资源生命周期

trait 边界都要求 `Send + Sync + 'static`，共享实例使用 `Arc`。可变共享状态只有 listener 和 TiKV client map，分别由 `Mutex` 保护；poison 时通过 `expect` panic，这是当前不可恢复边界。`closed` 使用 Acquire/Release 或 AcqRel：一旦 `close_client` 或 transport `close` 发布关闭，新定位/客户端创建会返回 `BatchError::Closed`；已取得的 gRPC handle 由 grpcio 自身管理。

attempt limiter permit 是 `send_coprocessor`/stream 调用内的局部 RAII 值，覆盖真实 RPC 尝试，返回后自动释放；每个副本尝试独立获取。`ReadStats::record_attempt` 也以每次尝试为粒度记录 store ID、耗时和超时标志。

grpcio server stream 在同步 API 上用 `block_on(StreamExt::next)` 拉取；显式 `close` 与 `Drop` 都调用 receiver cancellation，并用本地 `closed` bool 保持幂等。failpoint 延迟路径使用 `OnceLock<tokio::runtime::Runtime>` 创建单 worker 共享 runtime，同时以 Tokio timeout 和 grpcio `CallOption` 双层限制。PD 和 TiKV client 持有 `Environment` 的 `Arc`，保证 channel/stream 存活期内 gRPC 环境不会提前释放。

## 与 Go 版本的对应关系

本文件没有同名 Go 文件；它把 Go `pkg/store/copr/coprocessor.go` 依赖的 client-go 能力拆成 Rust 可注入边界。Go `BuildCopIterator`/`buildCopTasks` 通过 client-go RegionCache 构建任务，Rust 对应链是 `coprocessor.rs` + `store.rs` + 本文件的 `RegionMetadataTransport`/`GrpcRegionMetadataTransport`。Go 的 request context 由 client-go 组装，Rust 在 `protobuf_request` 中显式镜像 priority、isolation、replica/stale read、resource group、lock hints、paging 和 store-batch 字段。

Go `copIteratorWorker.handleLockErr` 对 `shared_lock_infos` 展开为多个 `txnlock.Lock`，然后用 `ResolveLocksWithOpts` 一次解析；Rust `NetworkBackend::resolve_lock` 保持这一语义，且 `TransactionLock` 保留 Go resolver 需要的完整字段。API V2 下，Rust `KeyCodec::decode_lock_info` 在交给 client-rust resolver 前递归剔除 keyspace 物理前缀，对应 client-go 的 keyspace truncate 边界。

Go `handleCopResponse` 依赖 Region error、lock、other error、bucket version、batch child 和 exec details 做局部重建与统计；Rust `pb_response` 不在 transport 层执行这些策略，而是完整保留证据给 `coprocessor.rs`。`response_read_bytes_v2` 在 NextGen kernel 下使用 `max(total_versions_size, processed_versions_size)`，否则用 processed size，这一口径有 `network_backend_runaway_test.rs` 的 Go 对齐回归。

差异和未完成面必须保持显式：Go/client-go 有更完整的 Region 缓存、store liveness、TiFlash/MPP/topology 和 SafePoint 能力；本文件刻意不建第二份 Region 缓存，`is_store_alive` 仅检查地址非空，MPP 不可用，多个 TiFlash 枚举方法返回空，`check_visibility` 不做实际检查。这些是已验证的当前代码事实，不是已支持功能。

## 扩展指南

- 新增或修改 Coprocessor 请求字段时，从 `CopWireRequest` 与 Go request/context 定义开始核对，修改 `GrpcStandardCoprocessorTransport::protobuf_request`，并在独立的 `pkg/store/copr/network_backend_test.rs` 增加传输边界回归；不要把测试内嵌进本源文件。
- 扩展响应统计或新错误类型时，同步修改 `pb_response`、必要的 `response_*` helper 和 `CopProtocolResponse`，保持父响应与 batch child 对称；数据会影响 paging、runaway、RU 计费和局部重试，应同步 `network_backend_runaway_test.rs` 及 `coprocessor_test.rs`。
- 修改 keyspace 或 Region 编码必须同时审查 `KeyCodec`、`mem_encode/mem_decode`、PD locate/scan、Coprocessor range、bucket key 和 lock key，以及 V2 空 end key 的下一 keyspace 上界。回归应扩展 `transaction_region_codec_matches_memcomparable_and_keyspace_boundaries` 和 lock decode 用例，兼容性风险是跨 keyspace 读或路由到错误 Region。
- 如要引入真正的 Region 缓存，必须先重新审查“只有上层 `RegionCache`”的不变量；不应直接往 `GrpcRegionMetadataTransport` 添加第二份状态。否则 invalidate、bucket version、leader 更换与并发任务会出现两套真相。
- 实现 MPP/TiFlash/topology 或 visibility 不是局部删除默认错误即可：需要扩展 `StoreBackend`/`RegionCacheBackend`、网络客户端和生命周期，并与 Go/client-go 实际行为对齐。在未具备完整语义前，应保持现有显式“unavailable”边界，不能用空成功响应伪装支持。
- 修改关闭、client cache 或 stream 时，要保持 `close_client` 幂等、关闭后禁止新 client、`close_address` 的局部性、stream `Drop` 取消和 attempt permit 的 RPC 粒度。并发风险集中在 mutex poison/panic、持锁时间和 close/send 竞态；性能风险集中在每次任务的 PD 重查、`block_on` 流读取和大 batch 响应聚合。

## 验证依据

- RustCodeGraph 状态：本仓库索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。查询 `NetworkBackend network_backend coprocessor` 确认 `NetworkBackend` 在本文件 343 行、`NetworkConfig` 在 64 行、`NetworkPdKeyspaceClient` 在 115 行、`NetworkRegionBackend` 在 741 行，并展示 `send_coprocessor` -> `coprocessor.rs::send` -> `build_cop_iterator` 的调用链。
- RustCodeGraph 源码证据：通过 `node --file pkg/store/copr/network_backend.rs` 分段阅读 1–2023 行，核对了 transport trait、`NetworkBackend` 两个 trait impl、`KeyCodec`、PD RPC、TiKV RPC、stream Drop 和 `pb_response`；通过 `node --file pkg/store/driver/tikv_driver.rs --offset 570 --limit 85` 确认生产构造与 `Store::new` 接线。
- crate/模块证据：`pkg/store/copr/Cargo.toml` 确认 crate、`tikv-client`、`grpcio`、`kvproto`、`protobuf`、futures、Tokio、failpoint 和 kernel-type 依赖；`pkg/store/copr/lib.rs` 确认 `network_backend` 公开重导出及独立 `network_backend_test.rs` 挂载；`pkg/store/copr/store.rs` 与 `coprocessor.rs` 确认 backend -> Store -> worker 的调用和锁处理位置。
- Go 对照：`pkg/store/copr/coprocessor.go` 的 `BuildCopIterator`、`handleCopResponse`、`handleLockErr` 和 bucket-version 处理，用于核对任务路由、Region/lock/batch 错误和 shared-lock 展开语义。仓库中不存在同名 `network_backend.go`，因此未声称逐函数对应。
- 测试证据：`pkg/store/copr/network_backend_test.rs` 验证串行 batch timeout、attempt permit 释放、Region 重定位/失效、stream close、普通与 shared lock、V1/V2 memcomparable/keyspace 边界、三副本超时后默认超时回退与 `ReadStats`；`pkg/store/copr/network_backend_runaway_test.rs` 验证新旧 exec details、batch child processed keys 和 kernel-specific read bytes。
- 本任务为纯文档分析，按计划不运行 Cargo。交付时使用任务指定的结构检查，要求文件存在且上述固定二级标题恰好 11 个；未做网络集成试验，真实 PD/TiKV/TLS 可用性不在本次验证范围。
