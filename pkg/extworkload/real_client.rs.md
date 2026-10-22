# `pkg/extworkload/real_client.rs`

## 文件定位

实现同步 `client::Client` 的全部方法；除同步 `Close` 外，每个方法都用私有运行时 `block_on` 等待对应异步 RPC。

## 核心职责

1. `RealController::connect` 将根层 `client::Option` 转为远端 `remote::Option`，复制控制器地址、keyspace ID/名称、TiDB pool 与可选 TLS 配置。
2. 创建一个固定两个 worker 线程、启用全部 Tokio 驱动的私有 `Runtime`，并在其 reactor 上下文中调用使用 `connect_lazy` 的 `remote::New`。
3. 实现同步 `client::Client` 的全部方法；除同步 `Close` 外，每个方法都用私有运行时 `block_on` 等待对应异步 RPC。
4. 把根层简化 `context::Context` 转成远端调用上下文，并把运行时构建、远端构造和 RPC 错误统一降格为字符串型 `client::ClientError`。

本文件不负责 Manager 的 30 秒超时注入、指标标签生成、TLS 文件读取或 RPC 请求头/响应业务错误解释；这些职责分别在 [`manager.rs`](manager.rs) 和 [`client/client.rs`](client/client.rs)。

## 主要符号

- `RealController { runtime: Runtime, inner: Box<dyn remote::Client> }`：crate 私有生产实现。同时拥有同步/异步桥所需运行时和远端 trait object；字段顺序不是对外协议。
- `RealController::connect(option: &client::Option) -> Result<Self, client::ClientError>`：唯一构造入口。它先映射选项，再构建运行时，在 `runtime.enter()` guard 存活期间创建惰性 tonic channel，最后把运行时与远端客户端一并保存。
- `RealController::context(context: &context::Context) -> remote::Context`：若根层上下文的 `Deadline()` 为真，生成固定 30 秒远端超时；否则生成无超时背景上下文。根层上下文只保存“是否有 deadline”，没有原始 deadline/剩余时长，所以这里无法逐值传播。
- `map_error(error: impl Display) -> client::ClientError`：保留错误的显示文本，但抹去远端错误枚举和错误源类型。
- `impl client::Client for RealController`：实现 `as_any`、`Close`、`Ping`，以及 GCv2、TTL、Auto Analyze 共九个状态 RPC。方法名保留 Go 风格大小写以匹配迁移接口。

## 执行流程

生产构造链为 `NewManagerWithTLS` → `manager::dialClient` → 根层 `client::New` → `RealController::connect` → 远端 `remote::New`。`dialClient` 先准备 keyspace、pool、地址、TLS 和根层指标拦截器；根层 `client::New` 仅在地址不以 `stub://` 开头时进入本文件。

连接阶段依次执行：以地址建立 `remote::Option`；复制身份与 TLS 字段；构建两线程 Tokio runtime；通过 `runtime.enter()` 提供 reactor 上下文；调用远端 `New` 创建惰性 channel；释放 enter guard；返回同时持有 runtime 和 client 的 `RealController`。因为远端使用 `connect_lazy`，真正网络建连通常发生在随后由 `dialClient` 发出的 `Ping`，而非 `connect` 返回前。

业务调用的共同路径是：Manager 派生带 deadline 的根层 context → 本实现按 deadline 有无创建远端 context → `runtime.block_on(inner.<RPC>(...))` → 远端客户端构造 protobuf 请求、附加 keyspace/pool 请求头并调用 tonic → 本文件用 `map_error` 转为根层错误。各方法不改写业务参数：safe point、GC 生命周期秒数、表 ID、TTL 开关、完成时间和 Auto Analyze task ID均原样转发。

`Close` 是例外：远端 trait 的关闭操作本身同步，因此直接调用 `inner.Close()`，不进入 `block_on`。`as_any` 返回自身，符合根层接口的下转型约定，但生产路径当前没有以它读取内部状态。

## 数据与状态

`RealController` 只有两项长期状态：私有 Tokio `Runtime` 和远端客户端 trait object。远端客户端持有规范化后的 endpoint、TLS、请求身份与 tonic channel；本层不缓存业务任务状态，也不维护重试计数或连接状态机。

选项映射保留 `ControllerAddr`、`KeyspaceID`、`KeyspaceName`、`TiDBPool` 和 `TLSConfig`。当前代码没有复制根层 `client::Option::Interceptors`；远端 `Option::with_addr` 创建的拦截器列表保持为空。这意味着 `manager::dialClient` 放入根层选项的 `metricsInterceptor()` 在真实客户端路径不会到达 tonic，而 Go 版 `client.New` 会把 `Interceptors` 传给 `grpc.WithChainUnaryInterceptor`。这是当前实现差异，不应把 Manager 测试中验证的指标标签等同于生产 RPC 已打点。

上下文映射同样是有损的：根层 `Context` 仅暴露 deadline 布尔值，`RealController::context` 将任何“有 deadline”统一转换成 30 秒，不携带根层 context 中的类型化值。Manager 的标准路径本来就使用 30 秒 `requestTimeout`/`dialTimeout`，所以超时值在该路径一致；若未来调用方提供不同 deadline，本层仍会使用 30 秒。

## 依赖与调用关系

上游直接调用边：

- [`lib.rs`](lib.rs) 的 `client::New` 调用 `RealController::connect`；非真实地址由同函数分流到测试桩。
- [`manager.rs`](manager.rs) 的 `dialClient` 调用根层 `client::New`，随后立即 `Ping`；Manager 的各 GCv2、TTL、Auto Analyze 方法再经 `Box<dyn client::Client>` 动态分派到本实现。
- 更上层的 session、GC worker、DDL/TTL 与统计信息代码只依赖 `Manager`，不会直接看见 `RealController`；RustCodeGraph 将 `manager.rs` 的使用者列为 `pkg/session/runtime/session.rs`、`pkg/store/gcworker/gc_worker.rs` 及测试文件。

下游调用边：

- Tokio `Builder::new_multi_thread`、`worker_threads(2)`、`enable_all`、`build` 提供异步执行环境。
- `remote::Option::with_addr` 与 `remote::New` 建立远端客户端；远端实现位于 [`client/client.rs`](client/client.rs)。
- `remote::Client` 的 `Ping`、三项 GCv2、四项 TTL 和两项 Auto Analyze 方法负责实际 RPC；本层只等待并转发结果。

RustCodeGraph 索引识别出本文件 20 个符号，包括 `RealController`、`connect`、`context`、`map_error` 与全部 trait 方法；文件级关系显示它被 `lib.rs`、`manager.rs`、客户端实现及相关测试等文件关联。精确 `callers/callees` 对这些 trait/同名方法没有输出，因此具体边以索引源码和唯一构造点的文本检索交叉确认。

## 错误处理与边界

运行时构建失败、远端客户端构造失败及所有 RPC 失败都经 `map_error` 变成 `client::ClientError(error.to_string())`。这样保留面向日志和上层注解的文本，但丢失 `remote::ClientError::ControllerPaused` 等可匹配变体；上层无法再按远端具体类型分支。Manager 会继续把该错误装箱为 `ManagerError`，初始化阶段还会附加 `init external workload client` 或 `ping external workload controller` 前缀。

本层不做参数范围校验：例如 `u64::MAX` 是 Manager 用来回收全部 GCV2 任务的合法约定，负表 ID 或生命周期值是否有效由上游/协议端决定。地址为空、URL 非法、TLS endpoint 配置失败、空响应和控制器业务错误由远端 [`client/client.rs`](client/client.rs) 处理。

`connect` 只创建惰性 channel，因此其成功不证明控制器可达；标准 Manager 构造紧接着执行 `Ping` 才完成探活。直接调用根层 `client::New` 的其他代码若不 Ping，也应理解这一边界。

每个同步 RPC 都调用 `Runtime::block_on`。若未来从正在驱动 Tokio runtime 的同一线程直接调用这些同步方法，嵌套运行时可能引发 Tokio 运行时限制或阻塞执行器；当前接口设计假设它作为同步边界使用。这里没有重试、退避、熔断或取消信号的额外处理。

## 并发与资源生命周期

每个 `RealController` 独占一个两 worker 线程的多线程 Tokio runtime；构造多个 Manager 会相应创建多组线程。`client::Client` 要求 `Send`，但所有业务方法接收 `&mut self`，常规安全 Rust 调用不能并发借用同一实例；若上层自行加锁，共享调用会在锁和同步 `block_on` 边界上串行化。

`runtime.enter()` guard 只覆盖远端惰性 channel 的构造，随后显式 `drop`；运行时对象继续作为字段存活，为之后的 `block_on` 和 tonic reactor 提供生命周期。`Close` 释放远端 channel 资源；`RealController` 被丢弃时，`inner` 与 runtime 随结构体一起销毁。文件未实现自定义 `Drop`，因此显式关闭失败或调用方遗漏 `Close` 时没有本层补偿逻辑。

超时资源由 Manager 的 `WithTimeout`/cancel 生命周期管理；本层只把 deadline 标志翻译成远端 30 秒请求超时。远端 tonic request 会把该超时写入 `grpc-timeout`。根层 context 中的取消函数、指标值及其他类型化值不会穿过该边界。

## 与 Go 版本的对应关系

Rust 因远端客户端采用 async trait，而迁移后的根层 Manager 保持同步接口，才增加本文件作为语言运行时适配层。

- Go `client.New` 的地址、TLS、身份字段和惰性 `grpc.NewClient`，由 Rust 远端 `client/client.rs::New` 实现；本文件负责把根层选项送入它。
- Go `context.Context` 可携带精确 deadline、取消和值；本文件只根据根层 Rust context 的布尔 deadline 选择固定 30 秒或 background，属于迁移后的信息损失。
- Go 将 `Interceptors` 交给 `grpc.WithChainUnaryInterceptor`；当前适配器没有复制拦截器。远端 Rust 客户端本身支持拦截器且其独立测试验证调用，但真实根层链路未接线。
- Go 返回可由 `errors.Is` 匹配的 `ErrControllerPaused`；远端 Rust 有 `ClientError::ControllerPaused`，但本层字符串化后根层只能按文本观察，不能保持类型匹配。

## 扩展指南

新增控制器 RPC 时，应同时扩展根层 [`lib.rs`](lib.rs) 的 `client::Client`、本文件的适配实现、远端 [`client/client.rs`](client/client.rs) 的 async trait/tonic 调用、Manager 或其公开接口，以及独立测试；不要把测试放入本生产文件。若新增请求需要新的 option 或 context 信息，应在 `RealController::connect` 或 `RealController::context` 明确完成映射，而不是默认它会自动穿透。

修复指标拦截器接线时，需要设计根层 `grpc::UnaryClientInterceptor` 到远端 tonic `UnaryClientInterceptor` 的语义转换；两者签名不同，不能只复制 vector。应增加独立的同步适配层回归测试，启动本地 tonic stub，经根层 `client::New` 发起调用，并断言拦截器/指标确实执行。

若要保留精确 deadline 或取消，首先必须增强根层 `context::Context` 的数据模型，再修改 `RealController::context`；仅在本文件改常量不能恢复调用方 deadline。若要保留 `ControllerPaused` 的类型语义，应让根层 `ClientError` 成为可区分的枚举或保留 error source，并同步 Manager 错误传播测试。

性能与兼容风险主要是：每实例两线程的资源成本、同步 `block_on` 对调用线程的占用、嵌套 Tokio runtime 的使用限制、改变 30 秒超时对慢请求的影响，以及错误类型/文本变化对上层判断和日志的影响。扩展后应同步 [`manager_test.rs`](manager_test.rs) 验证根层参数与 deadline，另在独立的适配层测试验证真实桥接；远端协议字段和响应语义继续由 [`client/client_test.rs`](client/client_test.rs) 覆盖。
