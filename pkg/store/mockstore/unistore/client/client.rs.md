# `pkg/store/mockstore/unistore/client/client.rs`

## 文件定位

本文件属于 crate `astersql-store-mockstore-unistore-client`，crate 入口 `pkg/store/mockstore/unistore/client/lib.rs` 通过 `pub mod client` 挂载本模块，并用 `pub use client::*` 在 crate 根重新导出其公共项。`pkg/store/mockstore/unistore/client/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package` 分别确认了 crate 边界及其 Go 来源 `pkg/store/mockstore/unistore/client`。

它位于 mock UniStore 的 RPC 抽象层，只定义“关闭客户端”和“同步发送一次请求”所需的最小接口，不保存连接、不选择协议，也不执行请求。workspace 根 `Cargo.toml` 将该 crate 注册为 `facade_store_mockstore_unistore_client`，`pkg/lib.rs` 再从 `store::mockstore::unistore::client` 门面导出。TiKV crate 和 cophandler crate 的 Cargo 清单也声明了依赖（后者为 optional），但当前 Rust 生产源码检索不到对该 trait 的实现或调用；因此这些清单只能证明可见性/依赖边界，不能证明运行主链已经接入。

## 核心职责

- `Client` trait 把调用方依赖压缩为两个能力：`close` 负责显式释放实现所持资源，`send_request` 负责向指定地址发出一次受上下文和超时约束的请求。
- 四个关联类型把上下文、请求、响应和错误留给实现方选择，避免本 crate 依赖具体 RPC/protobuf 类型。这对应 Go 文件注释所说的“重新定义接口以避免循环导入”。
- trait 只规定类型和方法签名，不规定地址格式、超时执行方式、取消语义、重试、路由、序列化、连接池或关闭后的行为；这些都属于实现方契约。
- 当前文件不是门面桩，但它只是接口定义。可确认的 Rust 行为覆盖来自 `migration_aster_unit_test.rs` 的测试实现；仓库现有 `pkg/store/mockstore/unistore/rpc.rs::RPCClient` 有自己的固有 `send_request`/关闭逻辑，尚未实现本 trait。

## 主要符号

- `pub trait Client`：唯一的模块级业务符号，也是公共 API。它没有 supertrait，因此 trait 本身不强制实现类型为 `Send`、`Sync` 或 `'static`。
- `type Context: ?Sized`：请求上下文关联类型。`?Sized` 允许使用动态大小类型；方法以 `&Self::Context` 借用它，不取得所有权。
- `type Request: ?Sized`：请求体关联类型，同样允许动态大小类型并通过共享引用传入，所以 trait 层不要求克隆或移动请求。
- `type Response`：成功响应的拥有型返回值，没有额外 trait bound。
- `type Error: StdError + Send + Sync + 'static`：失败值必须实现标准错误接口，并可在线程间安全传递且不借用短生命周期数据。约束只施加于错误类型，不等价于整个客户端可跨线程共享。
- `fn close(&mut self) -> Result<(), Self::Error>`：显式资源释放入口。可变借用阻止同一安全 Rust 引用在关闭期间并发调用该实例，但签名没有要求幂等，也没有替代 `Drop`。
- `fn send_request(&self, context: &Self::Context, address: &str, request: &Self::Request, timeout: Duration) -> Result<Self::Response, Self::Error>`：同步请求入口；共享借用允许实现自行采用内部可变性或同步机制。`Duration` 只表达超时参数，不保证 trait 层会计时或取消。

## 执行流程

本文件没有方法体，真实控制流发生在 trait 实现中。按接口可观察的抽象流程是：

1. 调用方准备某一实现选定的 `Context` 和 `Request`，以及目标 `address`、`timeout`。
2. 调用 `send_request` 时，所有输入除 `Duration` 外均以共享借用传入；实现方负责解析地址、解释上下文、落实超时并执行传输。
3. 成功时实现方构造并转移 `Self::Response`；失败时返回满足跨线程错误约束的 `Self::Error`。trait 不包装、转换或重试错误。
4. 生命周期结束前，持有可变客户端的调用方可调用 `close`；资源释放范围和重复关闭行为由实现方定义。

独立测试 `migration_aster_unit_test.rs::MockClient::send_request` 展示了最小可行实现：失败开关命中时原样返回 `TestError`，否则把上下文 ID、地址、载荷和超时记录到 `RefCell<Vec<SentRequest>>` 并回显响应。`MockClient::close` 仅设置 `closed` 标志。这是接口契约的测试样例，不应被误认为生产网络流程。

## 数据与状态

接口自身没有字段、全局变量或静态状态。请求期间的数据所有权由签名限定：上下文、地址和请求仅借用，响应与错误由实现方按值返回，超时值按值复制/移动进入调用。

`Context` 和 `Request` 的 `?Sized` 设计允许实现选择切片或 trait object 等动态大小输入；相对地，`Response` 与 `Error` 默认必须是 `Sized`，因为它们位于 `Result` 的按值返回位置。接口没有把“已关闭”编码成类型状态，也没有保存进行中请求数量，因此关闭状态、连接池、缓存和请求统计必须由具体实现管理。

测试实现的 `Cell<bool>` 和 `RefCell<Vec<SentRequest>>` 证明 `send_request(&self, ...)` 可通过内部可变性记录状态，但它们不是线程安全容器，且 `MockClient` 没有被要求跨线程使用。不能由 `Error: Send + Sync` 推导客户端本身具备并发安全性。

## 依赖与调用关系

- 直接标准库依赖只有 `std::error::Error`（别名 `StdError`）和 `std::time::Duration`；本 crate 的 Cargo 清单没有第三方 `[dependencies]`。
- 上游导出链为 `client.rs::Client` → `client/lib.rs` 的 glob re-export → workspace facade 依赖 → `pkg/lib.rs::store::mockstore::unistore::client`。这是名称可达性，不是运行时调用。
- `pkg/store/mockstore/unistore/client/migration_aster_unit_test.rs` 是已确认的 Rust 使用者：它导入 `super::Client`、为 `MockClient` 实现 trait，并直接调用两个方法。
- 直接检索显示 TiKV 和 cophandler Cargo 清单声明该 crate，但它们的 Rust 源码当前没有导入或实现目标 `Client`。`pkg/store/mockstore/unistore/rpc.rs::RPCClient::send_request` 是相邻的真实 Rust RPC 实现入口，却采用拥有型 `Request`、没有 `Context` 参数，也没有本 trait 的 `impl`，因此不能标成下游实现。
- Go 运行链已经接线：`pkg/store/mockstore/unistore/rpc.go::RPCClient` 以方法集合实现 Go `client.Client`；`tikv/server.go::Server.RPCClient`、`cophandler/cop_handler.go::MPPCtx.RPCClient` 和 `cophandler/mpp.go::MPPTaskHandler.RPCClient` 持有接口，`cophandler/mpp_exec.go::EstablishConnAndReceiveData` 调用 `SendRequest`。这条链是 Rust 迁移的对照证据，不是当前 Rust trait 的静态调用边。

## 错误处理与边界

两个方法都用同一个关联错误类型返回 `Result`，所以实现可保留传输层的具体错误分类；接口本身不增加上下文、不记录日志、不吞错，也不定义可重试性。`StdError + Send + Sync + 'static` 使错误适合跨线程传递或放入通用错误容器，但不要求 `Clone`、错误码或稳定文本。

边界条件均未由 trait 强制：空地址、零超时、已取消上下文、关闭后的发送、重复关闭、请求与响应协议不匹配都可能由不同实现作不同处理。测试 `close_releases_client_and_send_errors_are_preserved` 只验证测试实现会原样返回注入的发送错误并在 `close` 后置位；它没有验证关闭失败、幂等关闭或关闭后发送。测试 `send_request_preserves_context_address_request_and_timeout` 验证四项输入被完整透传，但不验证真实超时。

## 并发与资源生命周期

`send_request` 使用 `&self`，允许调用方持有多个共享引用，但 trait 没有 `Client: Send + Sync` 约束，因此不能承诺多线程并发调用。需要跨线程的实现应自行采用 `Mutex`、原子量或线程安全连接池，并在具体 API 上增加必要边界。错误值被明确要求 `Send + Sync`，仅保证错误传播能力。

`close` 需要 `&mut self`，在安全借用规则下与同一实例的共享调用互斥；如果客户端被包装在锁或内部共享指针中，实际停机协调仍由包装层负责。trait 没有异步方法、任务、通道或后台线程，也没有为在途请求规定 drain/cancel/join 顺序。实现若拥有这些资源，应让 `close` 明确处理并用独立测试覆盖；同时仍应实现 `Drop` 作为遗忘显式关闭时的兜底，而不能假设 trait 自动释放资源。

## 与 Go 版本的对应关系

Go 源 `pkg/store/mockstore/unistore/client/client.go` 同样只声明 `Client` 接口，包含 `Close() error` 和 `SendRequest(context.Context, string, *tikvrpc.Request, time.Duration) (*tikvrpc.Response, error)`，目的也是避开与 TiKV client 包的循环依赖。Rust 逐项保留了关闭、上下文、地址、请求、超时、响应和错误这七个语义位置。

主要差异是 Rust 将 Go 的固定协议类型泛化为四个关联类型，并用借用表达输入生命周期；Go 的 `context.Context` 和 `tikvrpc` 请求/响应由编译期固定，Rust trait 仅靠实现方解释。Go 接口方法集合可由 `rpc.go::RPCClient` 隐式满足，Rust 必须显式写 `impl Client for ...`。Go `Close` 不要求可变接收者，而 Rust 的 `&mut self` 在类型层表达独占关闭。Go `error` 没有等价的 `Send + Sync + 'static` 声明；Rust 额外约束错误便于跨线程传播。

迁移状态不是完全等价接线：Go 消费者已经通过接口调用生产 `RPCClient`，Rust 目标 trait 当前只在迁移单元测试中实现。Rust 相邻 `rpc.rs::RPCClient` 的签名也尚不匹配，不能声称 Go 的生产调用链已经通过此 trait 移植完成。

## 扩展指南

- 新增生产实现时，优先在实现类型所在源文件编写显式 `impl Client`，为四个关联类型选择真实协议类型；不要把协议依赖反向加入这个边界 crate，除非确认不会恢复 Go 侧刻意规避的依赖环。
- 若要让 `pkg/store/mockstore/unistore/rpc.rs::RPCClient` 实现本 trait，需要先决定 `Context` 如何映射、现有拥有型 `Request` 如何改为/适配借用输入、响应和 `RpcError` 的对应关系，以及关闭入口如何暴露。这些是必要设计，不应只为满足签名而丢弃上下文或复制请求。
- 增加重试、异步发送、按地址关闭、事件监听等能力前，应确认它们是否属于所有客户端的最小共同边界；实现特有能力宜留在扩展 trait 或具体类型，避免扩大依赖面。
- 修改 `Client` 的关联类型约束或方法签名时，应同步更新独立测试 `pkg/store/mockstore/unistore/client/migration_aster_unit_test.rs`，并检索 facade、TiKV/cophandler Cargo 使用者及所有 `impl Client for`。Rust 测试逻辑应继续放在独立测试文件，不嵌入 `client.rs`。
- 兼容风险主要是 trait 实现破坏和 facade 公共 API 变化；正确性风险集中在超时/取消被实现忽略及关闭与在途请求竞态；性能风险来自为适配借用接口而进行不必要复制或在共享 `send_request` 上加入粗粒度锁。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点，目标目录列出 `client.rs`、`lib.rs`、`client.go` 和独立迁移测试；目标文件节点显示 54 行及唯一 `Client` trait。对精确符号 `pkg/store/mockstore/unistore/client/client.rs::Client` 的 callers/callees 查询未得到可用静态调用边，因此用直接引用检索补齐未覆盖的 Cargo、门面和测试证据。
- 已读生产/装配文件：`pkg/store/mockstore/unistore/client/client.rs`、`client/lib.rs`、`client/Cargo.toml`、workspace `Cargo.toml`、`pkg/lib.rs`、`pkg/store/mockstore/unistore/rpc.rs`，以及 TiKV/cophandler 的 Cargo 清单。
- 已读 Go 对照与消费证据：`pkg/store/mockstore/unistore/client/client.go`、`pkg/store/mockstore/unistore/rpc.go`、`pkg/store/mockstore/unistore/tikv/server.go`、`pkg/store/mockstore/unistore/cophandler/cop_handler.go`、`mpp.go`、`mpp_exec.go`。
- 已读独立 Rust 测试：`pkg/store/mockstore/unistore/client/migration_aster_unit_test.rs`。其中 `send_request_preserves_context_address_request_and_timeout` 覆盖成功透传，`close_releases_client_and_send_errors_are_preserved` 覆盖错误保留与显式关闭标志。Go 同目录没有 `client_test.go`；全目录测试检索只发现 `pd_test.go` 持有具体 `RPCClient`，没有直接测试该接口声明。
- 人工核对结论：该文件存在是为了提供避免具体协议依赖的最小 RPC 客户端边界；它的运行行为完全由实现决定；当前 Rust 生产主链尚未通过该 trait 接线。未运行 Cargo，符合本纯文档任务的明确限制。
