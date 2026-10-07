# `br/pkg/restore/internal/import_client/import_client.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-restore-internal-import-client` 的主体实现。包入口 `br/pkg/restore/internal/import_client/lib.rs` 以 `#[path = "import_client.rs"]` 挂载该模块并 `pub use import_client::*`，因此调用方从 crate 根即可取得 `ImporterClient`、`ImportClient`、请求/响应占位类型和构造函数。`Cargo.toml` 将它声明为 workspace 内的 library crate，并通过 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/restore/internal/import_client`。

它位于 BR 恢复链路中“恢复编排”与 TiKV ImportSST RPC 之间：上层 `br/pkg/restore/snap_client/import.rs` 使用 `ImporterClient` 探测节点能力、下载 SST、设置限速并执行 `MultiIngest`；`br/pkg/restore/snap_client/tikv_sender.rs` 使用它发送强制分区范围请求。当前 Rust 文件不是生成的 gRPC stub，也没有真实网络依赖；它用本地 trait 和数据结构模拟 `kvproto/grpcio` 边界，默认构造出的 dialer 会返回不可用错误。测试或其他集成方必须经 `NewImportClientWithDialer` 注入可用传输实现。

## 核心职责

1. 以 `ImporterClient` 定义恢复侧需要的 ImportSST 操作集合，包括清理、应用、下载、批量下载、最新 MVCC 下载、导入、限速、能力探测和强制分区范围管理。
2. 以 `ImportClient` 按 `storeID` 缓存 `ClientConn`，并把普通 RPC 与 `MultiIngest` 的连接放入两个相互独立的连接池。
3. 在首次访问某个 store 时通过 `SplitClient::GetStore` 解析地址，优先选择 `PeerAddress`，再把 TLS、keepalive、3 秒最大退避等拨号参数交给可注入的 `GrpcDialer`。
4. 将上层操作转发到 `import_sstpb::ImportSSTClient`，同时保留 Go 版的错误包装和能力降级语义。
5. 显式关闭缓存连接，使 BRIE 等嵌入式调用在任务退出前能够释放连接资源。

## 主要符号

- `gRPCBackOffMaxDelay`：固定为 3 秒，传入每次 `DialArgs`，对应 Go 的同名常量。
- `Error`、`Code`、`Status`、`status_FromError`：本地错误与 gRPC 状态码适配层。能力探测只特别识别 `Code::Unimplemented`；`Error::Annotatef` 添加上下文但保留状态码。
- `Context`：由 `Arc<Mutex<Option<Error>>>` 实现的轻量取消令牌，提供 `Background`、`cancel`、`Err`、`Done`。本文件的转发逻辑并不主动检查取消状态，是否响应取消取决于注入的 `SplitClient`、dialer 或 RPC client。
- `TlsConfig`、`keepalive::ClientParameters`、`DialArgs`：保留 Go 拨号参数形状。`TlsConfig` 目前只是“是否启用 TLS”的占位类型，不携带证书材料。
- `metapb::Store` 与 `SplitClient`：仅保留按 store ID 获取 `Address`/`PeerAddress` 的最小元数据面。
- `import_sstpb`：定义本地请求、响应和 `ImportSSTClient` trait；这些类型只覆盖本文件及测试所需字段，并非完整 protobuf 生成物。
- `ClientConn`：可关闭连接抽象；`NewImportSSTClient` 从连接产生可共享的 RPC client。
- `GrpcDialer`：`Arc<dyn Fn(&Context, &DialArgs) -> Result<Box<dyn ClientConn>>>`，是实际传输的注入点。
- `ImporterClient`：上层依赖的公共 trait。`IsBatchDownloadLatestMVCCSupported` 在 trait 中提供默认实现，其余操作由 `ImportClient` 实现。
- `ConnCaches`：内部双 `HashMap<u64, Box<dyn ClientConn>>`，分别保存普通连接 `conns` 和导入连接 `ingest_conns`。
- `ImportClient`：持有元数据客户端、双池互斥锁、TLS/keepalive 配置和 dialer 的具体实现。
- `NewImportClient`：返回 `Box<dyn ImporterClient>`，使用 `default_dialer`；当前本地 trait 模式下第一次实际拨号必然失败。
- `NewImportClientWithDialer`：测试与集成入口，初始化空连接池并接受外部 dialer。
- `createGrpcConn`、`cachedConnectionFrom`、`GetIngestClient`：连接解析、创建、缓存以及 ingest 专用取连接的内部主链。

## 执行流程

普通 RPC（例如 `DownloadSST`）的流程为：调用 `GetImportClient`；`cachedConnectionFrom(..., ingest=false)` 锁住缓存并按 `storeID` 查找 `conns`；命中时从已有连接创建 RPC client，未命中时调用 `createGrpcConn`。后者执行 `SplitClient::GetStore`，优先采用非空 `PeerAddress`，否则使用 `Address`，组装 `DialArgs { backoff_max_delay: 3s, block: true, fail_on_non_temp_dial_error: true, ... }` 并调用 dialer。建连成功后先生成 client，再把连接插入缓存，最后由具体方法转发请求。

`MultiIngest` 使用同一流程，但经私有 `GetIngestClient` 选择 `ingest_conns`。因此同一 store 的普通下载/控制请求与 ingest 请求不会复用同一个 `ClientConn`。`br/pkg/restore/snap_client/import.rs::ingest` 在取得 region leader 后以其 `StoreId` 调用该入口，并在响应含业务 `Error` 时由上层转成恢复错误。

能力探测逐个、顺序遍历 store ID，并使用默认空请求调用真实 RPC 面：

- `CheckBatchDownloadSupport` 遇 `Unimplemented` 返回 `Ok(false)`，允许上层关闭批量合并路径；其他错误附加 store ID 后立即返回。
- `IsBatchDownloadLatestMVCCSupported` 遇 `Unimplemented` 返回 `Ok(false)`，其他错误带 store ID 返回；空 store 列表返回 `true`。
- `CheckBatchDownloadLatestMVCCSupport` 对 `Unimplemented` 返回包含“升级 TiKV 或关闭 retain-latest-mvcc-version”的强错误，不静默降级。
- `CheckMultiIngestSupport` 对 `Unimplemented` 返回节点不支持 multi ingest 的明确错误；其他错误附加 store ID。

`CloseGrpcClient` 在持锁状态下先复制普通池的 key，依次关闭并仅在关闭成功后删除条目，再以相同方式处理 ingest 池。任何一次关闭失败都会立即终止，失败项及尚未遍历的连接仍留在缓存中，可供调用方再次清理。

## 数据与状态

连接缓存的键只有 `storeID`，不包含地址或配置版本。首次成功建连后，后续请求持续复用该 store 对应的连接；如果 PD 返回的地址发生变化，本文件不会自动失效缓存，需要调用 `CloseGrpcClient` 后重新获取。普通池和 ingest 池各自最多为每个访问过的 store 保存一条连接。

`ImportClient` 的 TLS 与 keepalive 配置在构造时固定，拨号时克隆到 `DialArgs`。`Context` 的取消错误由共享互斥状态保存，克隆 context 会共享取消状态。`Error` 同时携带可显示消息和可选 gRPC code；`Annotatef` 不丢失 code，这保证上层包装后仍有机会识别 `Unimplemented`。

请求/响应对象由调用者拥有并以共享引用传入，RPC client 负责产生响应。本层对 `ClearResponse.Error`、`DownloadResponse.Error` 或 `IngestResponse.Error` 等业务字段不作解释；它只传播传输层 `Result`。`AddForcePartitionRange` 和 `RemoveForcePartitionRange` 则主动丢弃成功响应体，仅保留成功/失败。

## 依赖与调用关系

直接下游依赖均在本 crate 内以 trait 表达：`SplitClient::GetStore` 提供 store 元数据，`GrpcDialer` 建立 `ClientConn`，`ClientConn::NewImportSSTClient` 提供 `import_sstpb::ImportSSTClient`，随后各方法调用对应 RPC。`Cargo.toml` 的 `[dependencies]` 为空，印证当前实现不直接链接 kvproto、grpcio 或外部 TLS 库。

已核对的直接 Rust 上游包括：

- `br/pkg/restore/snap_client/client.rs::initClients` 把 `CheckMultiIngestSupport` 注册为 importer 创建回调。
- `br/pkg/restore/snap_client/import.rs::CheckBatchDownloadSupport` 根据探测结果设置 `mergeSst`；同文件的 latest-MVCC 两种探测分别用于严格校验和可回退的 peer-download-retry 判断。
- `br/pkg/restore/snap_client/import.rs::ingest` 对 region leader 调用 `MultiIngest`。
- `br/pkg/restore/snap_client/tikv_sender.rs::compactAndCheckSSTRange` 与 `removeForcePartitionRange` 分别调用 Add/Remove 请求，并由上层把 `Unimplemented` 视作兼容性降级。
- `br/pkg/restore/snap_client/import.rs` 与 `br/pkg/restore/log_client/import.rs` 的关闭路径调用 `CloseGrpcClient`。

RustCodeGraph 将目标文件标记为被 7 个文件使用，包括 `import_client_test.rs`、`snap_client/client.rs`、`snap_client/import.rs`、`snap_client/import_test.rs`、`split/client.rs` 等；由于 trait 动态分派未产生稳定的精确方法调用边，上述关系又通过直接入口搜索逐一核实。

## 错误处理与边界

`createGrpcConn` 对元数据查询和拨号错误执行 `Error::Trace`；当前实现中 `Trace` 是恒等函数，只保留 Go 调用形状。各 RPC 转发方法只包装“取得 client”阶段的错误，RPC 自身错误原样返回。能力探测对 `Unimplemented` 有专门分支，对其他错误使用 `Annotatef` 加上操作名和 store ID，便于定位节点。

空 store 列表不会发起 RPC：布尔探测返回 `true`，严格探测返回 `Ok(())`。文件没有校验 store 地址为空的情况；空地址会原样交给 dialer。它也不校验请求字段或响应中的业务错误，这些属于 protobuf 服务或上层恢复器职责。

互斥锁通过 `expect("import client mutex poisoned")` 或 `unwrap` 获取；若持锁线程 panic 导致 poison，调用会再次 panic，而不是返回 `Error`。默认 dialer 总是返回包含地址、退避和 TLS 启用状态的错误，所以 `NewImportClient` 在当前 Rust 本地 trait 模式中不能独立完成真实 TiKV RPC。这一限制与 Go 版真实 `grpc.DialContext` 不同，不能把接口存在误写成已接通生产网络。

## 并发与资源生命周期

`ImportClient` 通过单个 `Mutex<ConnCaches>` 保护两个连接池，使并发首次访问同一 store 时只插入一条缓存连接。锁覆盖缓存查询、`GetStore`、拨号和插入全过程，也覆盖 `CloseGrpcClient` 的全部关闭循环；这避免重复建连和关闭期间并发取用，但慢速元数据查询、阻塞拨号或连接关闭会阻塞所有 store、两个池的其他操作。

RPC 实际调用发生在取得 `Arc<dyn ImportSSTClient>` 并释放缓存锁之后，因此长时间下载/导入不会占用缓存互斥锁。缓存保存连接所有权，临时 RPC client 使用 `Arc`；关闭缓存连接并不保证外部已经取得的 client 立即失效，具体语义由注入的 `ClientConn` 实现决定。

资源生命周期从构造开始，以惰性拨号建立连接，直到显式 `CloseGrpcClient`。`ImportClient` 没有 `Drop` 自动清理实现；调用方必须在恢复任务退出路径主动关闭。关闭按普通池后 ingest 池的顺序串行执行，且“先关闭成功、再删除”与 Go 版一致。

## 与 Go 版本的对应关系

Rust 的 `ImporterClient` 方法集合、3 秒退避、PeerAddress 优先、普通/ingest 双连接池、能力探测分支及关闭成功后删除的规则，均对应 `br/pkg/restore/internal/import_client/import_client.go`。`br/pkg/restore/internal/import_client/import_client_test.rs::test_import_client` 也复刻了 Go `TestImportClient` 的 RPC 回显、MultiIngest 故障预算、能力探测和关闭行为。

关键差异是网络边界：Go 直接依赖 `grpc.ClientConn`、kvproto 生成 client、TLS credentials 和真实 `grpc.DialContext`；Rust 的 `Cargo.toml` 无外部依赖，`TlsConfig`、protobuf 类型和 gRPC client 都是局部最小替身，真实行为只能由注入的 dialer/client 提供。Rust 测试虽然用 `TcpListener` 取得地址字符串，但不启动 gRPC 服务。

Rust 将 Go 的 `checkStoreBatchDownloadLatestMVCCSupport` 私有辅助逻辑内联到两个探测方法（布尔版本是 trait 默认实现），可观察语义仍保持：成功为支持、`Unimplemented` 为不支持、其他错误带 store ID 上抛。Rust 的 `Context` 也只是近似 Go `context.Context`，没有 deadline、value 或自动中断阻塞操作的能力。

## 扩展指南

新增 ImportSST RPC 时，应同时扩展 `import_sstpb::ImportSSTClient`、公共 `ImporterClient` 和 `impl ImporterClient for ImportClient`，明确它应走普通池还是 ingest 池，并在独立测试文件 `br/pkg/restore/internal/import_client/import_client_test.rs` 的 mock server 与断言中补齐行为。若 Go 同路径已有对应能力，还应对照 Go 方法的错误包装、状态码降级和请求空值语义，避免只补接口桩。

若改变连接缓存策略，应重点审查 store 地址变化、同 store 并发首次拨号、关闭失败后的重试以及普通/ingest 流量隔离。缩小锁粒度虽然可能改善跨 store 并发，但必须避免重复连接泄漏及 Close/Get 竞态；当前单锁串行化是明确的不变量。

若要接通真实生产 gRPC，正确扩展点是实现或替换 `GrpcDialer`、`ClientConn` 和 `import_sstpb::ImportSSTClient` 边界，而不是让上层绕过 `ImporterClient`。同时需要把完整 protobuf 字段、TLS 证书语义、取消/超时传播和连接关闭行为纳入独立测试；当前占位类型不能证明这些能力已经实现。

能力探测扩展应区分“可以安全回退”的 `Ok(false)` 与“配置要求此能力”的强错误。新增探测还应保持短路顺序、错误中 store ID 上下文以及空 store 集合的确定语义，并在 `import_client_test.rs` 中覆盖成功、`Unimplemented` 和其他错误三类分支。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/restore/internal/import_client/import_client.rs` 读取了完整 815 行实现，并报告该文件被 7 个文件使用；`query ImportClient`、`query NewImportClient`、`query CheckBatchDownloadSupport`、`query cachedConnectionFrom` 用于核对 Rust/Go 对应符号。精确 `callers/callees` 对动态 trait 方法未返回可用结果，因此未据此臆造调用边。
- 目标实现：`br/pkg/restore/internal/import_client/import_client.rs`，核对了全部常量、类型、trait、构造函数、连接缓存、RPC 转发、能力探测和关闭逻辑。
- crate 边界：`br/pkg/restore/internal/import_client/Cargo.toml` 与 `br/pkg/restore/internal/import_client/lib.rs`，确认 library 入口、Go 包映射、空外部依赖和扁平再导出。
- Go 对照：`br/pkg/restore/internal/import_client/import_client.go` 与 `br/pkg/restore/internal/import_client/import_client_test.go`，核对真实 gRPC 拨号、双池缓存、探测语义和端到端测试意图。
- Rust 独立测试：`br/pkg/restore/internal/import_client/import_client_test.rs`，核对拨号参数、请求字段回显、双池关闭、MultiIngest 错误传播、latest-MVCC 的布尔/严格探测及空 store 行为。
- 直接上游：`br/pkg/restore/snap_client/client.rs`、`br/pkg/restore/snap_client/import.rs`、`br/pkg/restore/snap_client/tikv_sender.rs`、`br/pkg/restore/log_client/import.rs`，核对恢复主链中的创建回调、能力选择、导入、强制分区范围和资源关闭位置。
- 本任务为纯文档分析，按任务约束未运行 Cargo；交付验证只执行固定 11 章节结构检查，并人工检查没有把本地 trait 占位误写为真实网络能力。
