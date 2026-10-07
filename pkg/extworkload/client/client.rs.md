# `pkg/extworkload/client/client.rs`

## 文件定位

本文件实现 `astersql-extworkload-client` crate 的外部工作负载控制器 gRPC 客户端。crate 入口 `pkg/extworkload/client/lib.rs` 通过 `pub mod client` 和 `pub use client::*` 再导出这里的公共 API；`pkg/extworkload/client/Cargo.toml` 将库入口指定为 `lib.rs`，并用 `build.rs` 从 `pkg/extworkload/proto/externalworkload.proto` 生成 tonic 消息、客户端桩和测试所需的服务端桩。

在生产调用链中，`pkg/extworkload/real_client.rs::RealController::connect` 把上层同步配置转换为本文件的 `Option`，创建 Tokio runtime 后调用 `New`；`RealController` 的各方法再通过 `Runtime::block_on` 调用本文件的异步 `Client`。更上层的 `pkg/extworkload/manager.rs` 负责请求超时、指标标签和工作负载语义，本文件只负责 RPC 编码、发送和响应归一化。

## 核心职责

- 用 `Client`、`gcv2Client`、`ttlClient`、`autoAnalyzeClient` 四个 trait 描述 Ping、GC v2、TTL 和自动分析 RPC 能力。
- 由 `New` 校验选项、规范化控制器地址、配置 TLS 与拦截器，并创建惰性连接的 tonic 客户端。
- 由 `grpcClient::header` 为所有改变工作负载状态的请求附加 keyspace ID、keyspace 名和 TiDB pool；`Ping` 是唯一不带该公共头的 RPC。
- 将调用参数逐字段编码到 protobuf 请求，并经生成的 `ExternalWorkloadControllerClient` 发出请求。
- 由 `mapTonicResponse`/`mapResponse` 统一处理传输错误、空响应和控制器业务错误，特别把 `ErrorType::Paused` 映射为可匹配的 `ClientError::ControllerPaused`。
- 由 `Context` 把可选 `Duration` 转为 tonic request timeout；由 `Close` 使本地客户端进入不可再调用状态。

## 主要符号

- `pb`：`tonic::include_proto!("externalworkload")` 生成的 protobuf 与 gRPC API 命名空间。生成来源由 `build.rs` 指向 `../proto/externalworkload.proto`。
- `ClientError::{ControllerPaused, Message}`：对外错误类型。前者是控制器暂停 worker 的稳定分类，后者承载地址、建连、关闭后调用、传输或业务消息错误。`ErrControllerPaused` 是为对齐 Go 包级哨兵名保留的常量值。
- `UnaryClientInterceptor`：线程安全、可共享的一元元数据拦截函数，签名为 `Arc<dyn Fn(Request<()>) -> Result<Request<()>, Status> + Send + Sync>`。
- `Option`：连接和身份配置，包含 `KeyspaceID`、`KeyspaceName`、`TiDBPool`、`ControllerAddr`、可选 `ClientTlsConfig` 和拦截器列表。`Option::with_addr` 只填写地址，其余字段为空或零值。
- `Context`：每次调用的轻量上下文，只保存可选超时；`background` 无超时，`with_timeout` 设置超时，私有 `request` 负责创建 tonic `Request<T>`。
- `Client`：组合三个领域子 trait，并增加同步 `Close` 与异步 `Ping`。trait 及子 trait 均要求 `Send`，但不要求 `Sync`，且所有 RPC 都接收 `&mut self`。
- `gcv2Client`：`RegisterGCV2`、`RecycleGCV2`、`UpdateGCLifeTime`。
- `ttlClient`：`RegisterTTLTask`、`DeleteTTLTableInfo`、`RecycleTTLTask`、`UpdateTTLJobEnable`。
- `autoAnalyzeClient`：`RegisterAutoAnalyze`、`RecycleAutoAnalyze`。
- `ChainInterceptor`：按配置顺序执行全部拦截器，任一拦截器返回 `Status` 后立即停止链。
- `Stub`：带 `ChainInterceptor` 的生成客户端类型别名。
- `grpcClient`：具体实现，持有配置快照 `opt` 和可被 `Close` 取走的 `Option<Stub>`。
- `New`：公共构造函数，返回 `Box<dyn Client>`；创建的是 `connect_lazy` 通道，因此构造成功不代表远端当前可达。
- `normalizeAddr`：去除首尾空白；无 scheme 时保留地址；有 scheme 时验证 URL，并返回去掉 userinfo、路径、查询和片段后的 authority，同时保留显式默认端口。
- `mapTonicResponse`、`mapResponse`：RPC 的统一响应映射层；后者公开是为了独立验证 Go 的防御性空响应分支。

## 执行流程

1. `RealController::connect` 组装本文件的 `Option`，在已进入 Tokio runtime 的上下文中调用 `New(Some(&option))`。
2. `New` 先拒绝 `None`，再由 `normalizeAddr` 拒绝空地址、不可解析 URL 或没有 host 的带 scheme 地址。
3. 构造函数根据 `TLSConfig` 选择 `https` 或 `http`，用 `Endpoint::from_shared` 创建 endpoint，必要时应用 TLS 配置，然后调用 `connect_lazy`。此时不进行网络握手。
4. 生成客户端通过 `with_interceptor` 包装 `ChainInterceptor`；配置和桩被存入 `grpcClient`，再擦除为 `Box<dyn Client>` 返回。
5. 调用领域方法时，先用 `Context::request` 包装具体 protobuf 消息并设置可选超时。除 `PingRequest` 外，每个请求都从 `grpcClient::header` 取得一致的租户/池身份。
6. 生成桩执行具体异步 RPC；所有结果进入 `mapTonicResponse`，解包 tonic `Response<pb::Response>` 后交给 `mapResponse`。
7. `mapResponse` 依次检查传输错误、空响应、缺失业务错误、`Ok`、`Paused` 和其他错误类型。成功返回 `()`；失败保留方法名以便定位。
8. `Close` 对 `stub` 执行 `take`。第一次和后续关闭都返回成功，但之后任何 RPC 都会在 `grpcClient::stub` 处返回 “client is closed”，不会发送网络请求。

字段映射保持直接且无隐式换算：safe point 与任务 ID 使用 `u64`，表 ID 和 GC 生命周期使用 `i64`，TTL 开关使用 `bool`。GC 生命周期从 `Duration` 到秒数的换算发生在上层 `pkg/extworkload/manager.rs` 或桥接层，而不是本文件。

## 数据与状态

`Option` 在构造时被完整克隆进 `grpcClient`，因此后续修改调用方原始配置不会影响已创建客户端。每次状态变更 RPC 都重新从该快照构造 `pb::RequestHeader`；keyspace 同时携带 `KeyspaceId` oneof 和 `keyspace_name`，并携带 `tidb_pool`。

客户端唯一可变生命周期状态是 `stub: Option<Stub>`：`Some` 表示可尝试 RPC，`None` 表示已关闭。连接由 tonic `Channel` 持有并按需建立；本文件没有连接重试队列、业务缓存或后台任务。`Context` 是 `Copy` 的值对象，只表达有无调用超时，不支持取消句柄、任意 metadata 或截止时间戳。

`ChainInterceptor` 保存 `Arc` 函数列表的克隆。每次调用由 tonic 传入 metadata 请求，链中前一个拦截器的返回值会成为后一个拦截器的输入，因此拦截器顺序是可观察行为。

## 依赖与调用关系

上游生产链为：`pkg/extworkload/manager.rs` 的 `manager` → `pkg/extworkload/real_client.rs::RealController` → 本文件 `Client`/`grpcClient` → tonic 生成桩 → external workload controller。`manager` 为请求附加超时与指标，`RealController` 负责同步/异步边界和 runtime，本文件负责协议边界。

下游依赖包括：

- `tonic`：`Endpoint`、`Channel`、TLS、请求/响应、状态、拦截器和生成桩。
- `prost` 与 `tonic-build`：protobuf 数据类型及构建期代码生成；`protoc-bin-vendored` 避免依赖系统 protoc。
- `async-trait`：让对象安全 trait 暴露异步 RPC 方法。
- `thiserror`：错误显示实现。
- `url`：验证带 scheme 的控制器地址。

RustCodeGraph 的文件节点确认 `client.rs` 包含 55 个索引符号；源码节点显示其测试引用来自 `client_test.rs` 和 `migration_aster_unit_test.rs`。对精确符号的图查询还确认 `mapTonicResponse` 调用 Rust `mapResponse`。生产接线由 `real_client.rs` 中对 `astersql_extworkload_client as remote`、`remote::New` 和各远端 trait 方法的直接调用补证。

## 错误处理与边界

- 配置边界：`New(None)`、空地址、非法带 scheme 地址、无 host 地址均返回 `ClientError::Message`。不带 scheme 的非空字符串不会做 host/port 语义验证，错误可能延迟到首次 RPC。
- TLS 边界：是否存在 `TLSConfig` 同时决定 URI scheme；TLS 配置应用失败会带 “create external workload controller client” 上下文返回。
- 网络边界：`connect_lazy` 使 DNS、连接、握手和远端不可达错误在 RPC 时出现，由 `mapResponse` 包装为 `external workload rpc <method>`。
- 响应边界：`Ok(None)` 被视为协议异常；`response.error == None` 与 `ErrorType::Ok` 都视为成功；`Paused` 使用专门变体；其他类型保留控制器消息。
- 拦截边界：任何拦截器返回 `Status` 都会中止后续拦截器和 RPC，并作为传输错误进入统一映射。
- 关闭边界：`Close` 幂等，但关闭后不可重开；错误发生在本地 `stub()` 检查。
- `normalizeAddr` 会移除 userinfo，并保留显式 `:80`/`:443` 以匹配 Go `url.URL.Host`。无 scheme 分支仅 trim，不移除路径或 userinfo。

## 并发与资源生命周期

`Client` 只要求 `Send`，每个方法使用 `&mut self`，API 设计要求调用方串行取得可变访问；本文件不承诺同一个客户端可被并发调用。共享拦截器必须是 `Send + Sync`，由 `Arc` 支持跨客户端克隆，但拦截器自己的可变状态需要内部同步。

tonic `Channel` 在 `New` 中惰性创建，首次 RPC 才真正连接。`grpcClient` 不创建线程或任务；生产桥接层 `RealController` 拥有两 worker 的 Tokio runtime，并在各同步方法中 `block_on`。`Close` 丢弃桩及其 channel 持有关系；真正的 runtime 生命周期由 `RealController` 管理。

测试 `client_test.rs::start_stub_server` 启动后台 tonic server 并在结束时 `abort` server task；这验证了客户端资源使用方式，但后台任务不属于生产 `grpcClient`。超时由 tonic request metadata/`grpc-timeout` 传播，本文件没有额外 timer 或取消任务。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/extworkload/client/client.go`，公共配置字段、四组接口、十个 RPC、请求头和请求字段保持一一对应。Rust 用 trait 组合对应 Go 嵌入接口；用 `Box<dyn Client>` 对应 Go interface；用 `ClientError::ControllerPaused` 对应可由 `errors.Is` 匹配的 `ErrControllerPaused`。

关键实现差异如下：

- Go `grpc.NewClient` 返回 `*grpc.ClientConn` 并由 `Close` 关闭；Rust 用 `connect_lazy` channel，并通过清空 `Option<Stub>` 表达关闭。
- Go 接收 `context.Context`，天然支持截止时间、取消和值；Rust `Context` 当前只保存可选相对超时。生产桥接 `RealController::context` 将上层“存在 deadline”粗化为固定 30 秒，而非保留原截止时间。
- Go 的 unary interceptor 签名可以访问方法、请求、响应、连接和 invoker；本文件的 `UnaryClientInterceptor` 只操作 `Request<()>` metadata，能力范围更窄，但声明顺序一致。
- Go `mapResponse` 通过运行时栈 `callerRPCName()` 获取方法名；Rust 各 RPC 显式把稳定方法名传给 `mapTonicResponse`，避免依赖栈检查。
- Rust `normalizeAddr` 对带 scheme 地址显式移除 userinfo，并保留显式默认端口；其目标是复现 Go `url.URL.Host` 的 authority 行为。

`pkg/extworkload/client/client_test.rs` 与 `migration_aster_unit_test.rs` 使用内存 tonic server 验证全部请求字段、公共 header、拦截次数、暂停/未知错误、地址（包括 IPv6）和空响应。`client_test.go` 覆盖相同的 Go 行为基线。

## 扩展指南

新增 controller RPC 时，应同时完成以下局部修改：

1. 在对应领域 trait 增加方法；若是新领域，新增子 trait 并让 `Client` 组合它。
2. 在 `grpcClient` 的对应 impl 中构造生成的 protobuf 请求。凡改变租户工作负载状态的请求都应调用 `header()`，并逐字段保持 Go/协议类型与单位一致。
3. 使用 `Context::request` 保留超时传播，调用生成桩，并经 `mapTonicResponse("稳定方法名", ...)` 统一映射错误。
4. 在 `pkg/extworkload/real_client.rs` 增加同步桥接；若上层 manager 对该动作要求指标或超时，再在 `pkg/extworkload/manager.rs` 接线。不要把这些上层策略下沉到本文件。
5. 同步扩展独立测试 `pkg/extworkload/client/client_test.rs` 和迁移对齐测试 `migration_aster_unit_test.rs`，并核对 `client_test.go` 或新增的 Go 测试。Rust 测试应继续留在独立文件，不内嵌回生产源文件。

修改连接行为时重点评估：是否仍保持非阻塞构造、TLS scheme 是否与配置一致、显式默认端口/IPv6/userinfo 是否与 Go 一致、关闭是否幂等、拦截器顺序是否变化。修改错误类型时需保留调用方区分 `ControllerPaused` 的能力；修改 `Context` 时需评估 `RealController` 的同步桥接和取消语义。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/extworkload/client` 列出目标 Rust/Go/测试文件；`node --file pkg/extworkload/client/client.rs --offset 1 --limit 500` 读取完整 500 行并报告 55 个符号；`query normalizeAddr`、`query mapResponse`、`query mapTonicResponse` 区分 Go/Rust定义；`callees` 输出确认 `mapTonicResponse → mapResponse`。
- 目标实现：`pkg/extworkload/client/client.rs`，重点核对 `Option`、`Context`、四个 trait、`ChainInterceptor`、`New`、`normalizeAddr`、`grpcClient` 各 impl、`mapTonicResponse` 与 `mapResponse`。
- crate/生成边界：`pkg/extworkload/client/Cargo.toml`、`pkg/extworkload/client/lib.rs`、`pkg/extworkload/client/build.rs`、`pkg/extworkload/proto/externalworkload.proto`（由 build 脚本路径确认）。
- 生产调用链：`pkg/extworkload/real_client.rs::RealController` 和 `pkg/extworkload/manager.rs` 的转发方法；文本检索确认 `RealController` 直接依赖该 crate，并为全部 RPC 建立同步桥接。
- Go 对照：`pkg/extworkload/client/client.go`，核对接口、连接、地址规范化、header、请求字段和响应映射顺序。
- 独立测试：`pkg/extworkload/client/client_test.rs`、`pkg/extworkload/client/migration_aster_unit_test.rs`、`pkg/extworkload/client/client_test.go`，核对全量 RPC 往返、拦截器、错误映射、配置校验、IPv6 和空响应分支。
- 本任务只新增文档，按计划不运行 Cargo。交付前以任务给定命令验证本文恰有十一个固定二级标题，并人工复查未把测试写入生产文件、未声称构造时已连通远端、未把上层指标/超时策略误写成本文件职责。
