# `pkg/util/topsql/reporter/mock/pubsub.rs`

源文件：[`pubsub.rs`](./pubsub.rs)；Go 对照：[`pubsub.go`](./pubsub.go)；Rust 测试：[`pubsub_test.rs`](./pubsub_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。

## 文件定位

本文件属于独立 Cargo 包 `astersql-util-topsql-reporter-mock`。同目录 `lib.rs` 以公开模块 `pub mod pubsub` 暴露它，并通过 `tipb` 模块包含构建期生成的 tonic protobuf/gRPC 类型。它不是 TopSQL 生产服务端实现，而是供测试创建回环 TCP 监听器、可选注册 `TopSqlPubSub` 服务、启动异步 gRPC 服务并控制停止的生命周期替身。

当前 Rust 仓库中的直接使用都位于测试：`pubsub_test.rs` 验证真实 gRPC 订阅与重复注册，`migration_aster_unit_test.rs::pubsub_binds_loopback_and_stops_cleanly` 验证动态回环地址和重复停止。`pkg/util/topsql/Cargo.toml` 也只在 `dev-dependencies` 中以 `topsql_mock` 引入该 crate；因此不能把该文件描述为线上 SQL 请求或 reporter 上报主链的一环。`pkg/util/topsql/reporter/Cargo.toml` 以 `topsql_protocol` 依赖同一 crate，主要是复用生成的协议类型，不能据此推断 mock server 在生产路径启动。

## 核心职责

- `NewMockPubSubServer` 在 `127.0.0.1:0` 绑定由操作系统分配的端口，并保存稳定、可供客户端连接的地址。
- `MockGrpcServerHandle::register_top_sql_pub_sub` 延迟保存一个一次性服务工厂，使测试能复刻 Go 中先取得 `*grpc.Server`、注册服务、再调用 `Serve` 的顺序。
- `mockPubSubServer::Serve` 把标准库监听器交给 Tokio；已注册服务时启动真正的 tonic gRPC server，未注册时也持续接受 TCP 连接，以保留 Go 空 `grpc.Server` 的监听生命周期。
- `MockGrpcServerHandle::Stop`、`mockPubSubServer::Stop` 和 `Drop` 共同提供跨克隆句柄的停止信号与兜底清理。
- `Server` 和 `Address` 分别把服务注册/停止句柄与实际监听地址交给测试装配代码。

## 主要符号

- `ServeFuture`：装箱、固定地址且可跨线程发送的异步服务结果；输出保留 `tonic::transport::Error`，供后台任务记录。
- `ServiceFactory`：一次性闭包类型，消费 Tokio 监听器和 `watch::Receiver<bool>`，构造 `ServeFuture`。使用 `FnOnce` 是因为被注册的服务值会被移入 tonic server。
- `MockGrpcServerHandle`：可克隆的公开生命周期句柄。`stop` 是共享 watch 发送端；`service` 是 `Arc<Mutex<Option<ServiceFactory>>>`，在多个句柄间共享唯一待启动服务。
- `MockGrpcServerHandle::is_stopped`：读取 watch 当前值，供测试观察停止是否已发出；它不等待后台任务真正退出。
- `MockGrpcServerHandle::register_top_sql_pub_sub<T: TopSqlPubSub>`：把生成的 `TopSqlPubSubServer<T>` 封装进服务工厂；同一 server 只允许注册一次。
- `MockGrpcServerHandle::Stop`：广播 `true`，忽略没有接收者等发送失败；克隆句柄也能停止同一服务。
- `mockPubSubServer`：公开类型但沿用 Go 的非 CamelCase 名称。`listen` 保存尚未交给 Tokio 的监听器，`grpcServer` 保存句柄，`addr` 保存连接地址，`serving` 记录是否已启动。
- `NewMockPubSubServer`：公开构造入口，返回 `io::Result<mockPubSubServer>`。
- `mockPubSubServer::{Serve, Server, Address, Stop}`：分别启动、克隆句柄、复制地址和发出停止；`Drop::drop` 自动调用 `Stop`。

文件只有 crate 级 `allow(non_snake_case, non_camel_case_types)`，用于保留 Go 风格 API；没有常量、trait 定义或条件编译项。

## 执行流程

1. `NewMockPubSubServer` 同步绑定 `127.0.0.1:0`，把监听器设为 nonblocking，读取实际 `local_addr`，创建初值为 `false` 的 watch 通道和空服务槽，最后令 `listen=Some`、`serving=false`。
2. 测试通过 `Server` 取得克隆句柄。若需要真实 RPC，调用 `register_top_sql_pub_sub`：方法构造一个闭包，闭包未来会建立 `tonic::transport::Server`、添加生成的 `TopSqlPubSubServer`，并以 watch 变化作为 graceful shutdown future。服务此时尚未启动。
3. `Serve` 首先检查 `serving`；已启动则幂等返回。否则从 `listen` 中一次性取走标准库监听器并转换为 Tokio 监听器，再订阅停止通道并从互斥槽中一次性取走服务工厂。
4. 若存在服务工厂，`Serve` 用 `tokio::spawn` 驱动 tonic server。shutdown future 先检查停止值，随后等待变化，观察到 `true` 或发送端消失时结束；transport 错误只在后台记录 warning。
5. 若没有注册服务，后台任务在停止通知和 `listener.accept()` 之间 `select!`。成功连接会被立即丢弃但循环继续；停止、watch 关闭或 accept 出错会退出。
6. 成功派生后台任务后，`Serve` 设置 `serving=true` 并返回。调用方用 `Address` 构造客户端端点，最终通过 server 或句柄的 `Stop` 广播停止；对象被丢弃时还有 `Drop` 兜底。

## 数据与状态

状态分成三部分：尚未启动的 `Option<TcpListener>`、共享的 `Option<ServiceFactory>` 和 watch 中的布尔停止值。两个 `Option` 都体现“一次消费”语义：监听器只在第一次有效 `Serve` 时取走，服务工厂也只在启动时取走；`serving` 使后续 `Serve` 直接成功而不重复派生任务。

`service` 槽的合法注册状态是 `None -> Some(factory) -> None`。在 `Serve` 取走工厂前，第二次注册因槽为 `Some` 返回 `AlreadyExists`；启动后槽又成为 `None`，当前实现没有额外的“已启动”标志放在句柄中，因此理论上仍可再次注册，但已经没有第二次消费该工厂的路径。这是当前 API 的真实边界，不应把“整个对象生命周期绝对只能注册一次”作为已实现保证。

`stop` 值只从构造时的 `false` 走向 `true`，没有复位入口。`addr` 在构造时从已绑定监听器取得，之后不变；`Address` 每次克隆字符串。`is_stopped` 只证明停止信号的当前值，不证明监听 socket 或后台 task 已完成回收。

## 依赖与调用关系

模块装配边为 `lib.rs -> pubsub.rs`。`lib.rs::tipb` 通过 `tonic::include_proto!("tipb")` 提供 `TopSqlPubSub` trait、`TopSqlPubSubServer` 和客户端；源协议 `proto/topsql.proto` 定义 `Subscribe(TopSQLSubRequest) returns (stream TopSQLSubResponse)`。`build.rs` 与 Cargo 的 `tonic-build`/`protoc-bin-vendored` 负责生成这些类型。

直接下游依赖如下：

- 标准库 `TcpListener`、`io`、`Future`、`Pin`、`Arc`、`Mutex`：负责同步绑定、错误边界、异步类型擦除与共享服务槽。
- `tokio::net::TcpListener`、`tokio::spawn`、`tokio::select!`、`tokio::sync::watch`：负责异步监听、后台任务和广播停止。
- `tokio_stream::wrappers::TcpListenerStream`：把 Tokio listener 适配为 tonic 的 incoming stream。
- `tonic::transport::Server` 与生成的 `TopSqlPubSubServer`：承载真实 gRPC 订阅服务。
- `log::warn!`：记录启动后 tonic transport 失败，target 固定为 `top-sql`。

RustCodeGraph 的符号 Trail 显示，`NewMockPubSubServer` 的调用者是 `pubsub_test.rs` 中两个测试和 `migration_aster_unit_test.rs::pubsub_binds_loopback_and_stops_cleanly`；`register_top_sql_pub_sub` 的调用者是 `registered_service_handles_real_grpc_subscription_and_stops` 与 `duplicate_service_registration_is_rejected`。精确仓库搜索未发现生产 Rust 调用者。

## 错误处理与边界

- 构造阶段的 bind、`set_nonblocking` 和 `local_addr` 失败通过 `io::Result` 原样返回；失败时不会产生 server 对象。
- `register_top_sql_pub_sub` 在 mutex poisoned 时返回 `io::Error::other("mock pubsub service lock poisoned")`，重复注册返回 `io::ErrorKind::AlreadyExists`；测试明确断言后者。
- `Serve` 在标准监听器转 Tokio 监听器或锁服务槽失败时同步返回错误，并且尚未设置 `serving=true`。注意监听器在转换或后续锁失败之前已从 `Option` 取走；失败后重试会因 `listen=None` 直接返回成功，不能恢复原监听器。
- 启动之后，tonic transport 错误不再返回给 `Serve`，只写 warning；未注册分支的 accept 错误静默结束。调用方因此不能仅凭 `Serve` 成功判断后台任务一直健康。
- `Serve` 可以在未注册服务时调用；此时只接受并丢弃 TCP 连接，不实现 HTTP/2 或 protobuf。它适合绑定/生命周期测试，不适合声称 RPC 可用。
- 在 Tokio runtime 外调用 `Serve` 会在 `tokio::spawn` 处失败；签名没有表达这一前置条件。现有调用都位于 `#[tokio::test]`。
- `Stop` 是幂等、无错误返回的尽力通知。它不 join 后台 task，也不等待端口完全关闭。

## 并发与资源生命周期

构造和 `Serve(&mut self)`/`Stop(&mut self)` 需要 server 的独占可变借用，避免同一实例并发取走监听器。`MockGrpcServerHandle` 可跨克隆共享：watch sender 本身负责通知同步，服务工厂槽由 `Arc<Mutex<_>>` 串行访问；`ServiceFactory` 和 `ServeFuture` 都要求 `Send`，以满足 Tokio 多线程 runtime 的任务迁移。

服务注册必须发生在第一次有效 `Serve` 取走工厂之前，才能进入真实 gRPC 分支。停止可由 wrapper 或任意克隆句柄发出；tonic 分支将信号作为 graceful shutdown，未注册分支在 `select!` 中退出。若对象未经显式停止直接离开作用域，`Drop` 会发送停止信号并丢弃尚未移交的标准监听器。

第一次 `Serve` 后监听器归后台 task 所有，wrapper 不保存 `JoinHandle`。因此 `Stop` 后的资源释放由任务调度推进，测试中的 `yield_now` 只给任务一次运行机会，不构成严格 join。`migration_aster_unit_test.rs` 覆盖重复 `Stop` 不 panic；`pubsub_test.rs` 覆盖克隆句柄停止已注册服务并观察 watch 值。

## 与 Go 版本的对应关系

直接对照是 `pkg/util/topsql/reporter/mock/pubsub.go`。两版都在 `127.0.0.1:0` 绑定动态端口，保存实际地址，向调用者暴露 server 句柄，以异步方式启动服务，并把启动后的 serve 错误降级为日志；`Stop` 都设计为可安全调用。Go 的真实使用见 `pkg/util/topsql/topsql_test.go::TestTopSQLPubSub` 和 `TestPubSubWhenReporterIsStopped`：先 `tipb.RegisterTopSQLPubSubServer(server.Server(), service)`，再 `go server.Serve()`，最后 `defer server.Stop()`。Rust 的延迟工厂正是为了保持这一装配顺序。

已验证的实现差异如下：

- Go 构造时直接创建 `*grpc.Server`，生成代码可直接向其注册任意 gRPC 服务；Rust 句柄不是通用 tonic builder，只提供 `register_top_sql_pub_sub<T: TopSqlPubSub>`，当前仅支持一个 TopSQL PubSub 服务。
- Go `Serve` 自身阻塞，应由调用者启动 goroutine；Rust `Serve` 内部 `tokio::spawn` 后立即返回，并以 `serving` 保证 wrapper 上重复调用幂等。
- Go 的 `grpc.Server.Stop` 是立即停止；Rust 使用 `serve_with_incoming_shutdown`，更接近异步 graceful shutdown，且没有等待任务完成的接口。
- Go 没有显式重复注册错误或服务槽；Rust 在启动前对第二次注册返回 `AlreadyExists`。
- Rust 在未注册服务时实现 TCP accept 循环，用以模拟空 Go gRPC server 的监听生命周期；它不会提供 gRPC 协议响应。
- Rust 增加 `Drop` 自动停止和 `is_stopped` 观测接口；Go 依赖调用方显式 `Stop`。

Rust 独立测试没有逐行复刻 Go 的两个 TopSQL 端到端测试，而是以 `EchoPubSub` 验证真实 tonic 客户端能收到流式响应，并单独验证注册冲突与生命周期。生产 reporter 行为仍应由 reporter/topsql 层测试负责。

## 扩展指南

- 若要支持额外 gRPC 服务，优先扩展服务注册抽象与 builder 组装，而不是复制监听器/停止逻辑。必须明确多服务是否允许、注册顺序以及启动后注册的错误，并在独立 `pubsub_test.rs` 中增加真实客户端回归。
- 若要严格禁止启动后注册，应让共享状态同时记录 `serving`，使 `register_top_sql_pub_sub` 在工厂已被取走后返回可辨识错误；只检查 `Option` 不足以区分“尚未注册”和“已经启动”。
- 若调用方需要确认关闭完成，应保存 `JoinHandle` 或增加显式 async shutdown/join API，并测试停止后重绑定端口或客户端连接结束；不能把 `is_stopped` 当作资源已释放证明。
- 修改 `Serve` 的错误策略时，需要分别覆盖同步启动错误、tonic 后台错误和未注册 accept 错误。若错误要回传，API 很可能需要 async 化或增加错误通道，并评估对 Go 风格调用顺序的兼容影响。
- 调整地址或网络行为时，保持默认只绑定 loopback 和动态端口，避免测试暴露到外部接口或发生端口争用；同步更新 `migration_aster_unit_test.rs::pubsub_binds_loopback_and_stops_cleanly`。
- 修改 protobuf 服务签名时，同步检查 `proto/topsql.proto`、生成的 trait 使用、`EchoPubSub`、reporter 的 `TopSQLPubSubService` 以及 Go 的 `tipb.RegisterTopSQLPubSubServer` 对照。性能关注点主要是每个 server 一个后台任务、每个连接的 tonic 开销以及 mutex 注册临界区；当前 mock 不面向生产吞吐。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/topsql/reporter/mock` 确认目标、Go 对照、模块入口与测试已索引；`node --file pkg/util/topsql/reporter/mock/pubsub.rs` 读取全部 193 行并显示该文件有 22 个符号。
- 符号与调用边：`query`/`node` 核对 `MockGrpcServerHandle`、`mockPubSubServer`、`NewMockPubSubServer`、`register_top_sql_pub_sub`；Trail 给出构造函数到两个结构体的实例化边，以及三个 Rust 测试调用者和两个注册调用者。独立 `callers` 命令在 30 秒内未返回，因此用 Trail 与局部精确 `rg` 交叉验证，未据此推断更多生产调用。
- 已读生产与配置：`pkg/util/topsql/reporter/mock/pubsub.rs`、`pubsub.go`、`lib.rs`、`Cargo.toml`、`proto/topsql.proto`，以及 `pkg/util/topsql/reporter/Cargo.toml`、`pkg/util/topsql/Cargo.toml`。目标包及其父目录没有 `doc.go`。
- 已读测试：`pkg/util/topsql/reporter/mock/pubsub_test.rs`、`migration_aster_unit_test.rs`，并读取 Go `pkg/util/topsql/topsql_test.go` 中 `TestTopSQLPubSub`、`TestPubSubWhenReporterIsStopped` 的装配流程。
- 本任务只新增说明文档，按计划不运行 Cargo。交付检查只执行任务指定的 11 章节结构命令，并人工复核“存在原因、运行流程、安全扩展点”均有上述源码、配置、调用边或测试证据。
