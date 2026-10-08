# `pkg/store/mockstore/redirector.rs`

## 文件定位

本文件属于 `astersql-store-mockstore` crate。crate 入口 `pkg/store/mockstore/lib.rs` 通过 `pub mod redirector` 声明该模块，并通过 `pub use redirector::*` 再导出其公开类型和函数；`pkg/store/mockstore/Cargo.toml` 则表明该 crate 直接依赖提供嵌入式 RPC 类型的 `astersql-store-mockstore-unistore`。

它提供一个 RPC 客户端路由门面：带 `StoreTarget::TiDb` 的请求交给惰性创建的第二客户端，`TiKv` 和 `TiFlash` 请求继续交给 mock 客户端。需要注意的是，RustCodeGraph 对本文件给出的文件级结果为 `used by 0 files`，仓库内对公开符号的检索也只找到本文件及 `lib.rs` 的模块导出；因此这些 API 当前是“已公开、尚未在 Rust mockstore 构造链中接线”的迁移实现，不能据此声称完整应用已经使用它。

## 核心职责

- 用 `RoutedRequest` 将 `Request` 与逻辑目标 `StoreTarget` 绑定，避免依据请求体变体猜测目的端。
- 由 `ClientRedirector::send_request` 执行二路选择：`TiDb` 使用 `rpc_client()`，其余目标使用 `mock_client`。
- 由 `OnceLock` 保证 `rpc_factory` 至多成功初始化一次，并让并发的同步、异步请求共享同一个第二客户端。
- 将关闭全部连接、关闭指定地址和安装事件监听器这些客户端生命周期操作转发到正确的底层对象。
- 通过 `KvClient` 抹平具体客户端类型，使 mock 与第二客户端都能替换为测试替身或其他实现。

文件不负责创建 mock 集群、决定地址、解析协议请求或实现请求处理；实际的 `Request`、`Response`、`RpcError` 和嵌入式 `RPCClient` 来自 `crate::embedded_unistore::rpc`（实际源文件为 `pkg/store/mockstore/unistore/rpc.rs`）。

## 主要符号

- `StoreTarget::{TiKv, TiFlash, TiDb}`：可复制、可比较的路由标签。只有 `TiDb` 触发第二客户端；`TiKv` 与 `TiFlash` 在当前实现中走同一条 mock 分支。
- `RoutedRequest { target, request }`：按值持有路由标签和 RPC 请求体；发送后请求体被消费，不提供重试副本。
- `KvClient: Send + Sync`：本文件的最小客户端接口。`close`、`close_addr` 和 `send_request` 必须由实现者提供，`set_event_listener` 默认不做任何事。
- `impl KvClient for RPCClient`：把嵌入式 UniStore 的同名同步方法直接适配到 trait；没有覆写事件监听器，因此对该具体类型调用监听器接口是空操作。
- `ClientRedirector`：核心门面，持有 `mock_client: Arc<dyn KvClient>`、`rpc_client: OnceLock<Arc<dyn KvClient>>` 和线程安全的 `rpc_factory`。
- `ClientRedirector::new`：只保存 mock 客户端和工厂，不提前创建第二客户端。
- `ClientRedirector::rpc_client`：通过 `OnceLock::get_or_init` 调用工厂，然后克隆共享客户端的 `Arc`。
- `ClientRedirector::{close, close_addr}`：先操作 mock 客户端，成功后才操作已经初始化的第二客户端。
- `ClientRedirector::{send_request, send_request_async}`：分别提供同步路由和“新建线程后执行同步路由”的回调式异步入口。
- `ClientRedirector::set_event_listener`：仅把监听器安装到 mock 客户端。
- `newClientRedirector`：兼容 Go 命名的公开构造函数，返回 `Arc<ClientRedirector>`；crate 级 `allow(non_snake_case)` 允许这个名字。

## 执行流程

1. 调用者提供 `Arc<dyn KvClient>` mock 客户端和 `rpc_factory`，经 `ClientRedirector::new` 或 `newClientRedirector` 建立共享门面；此时 `rpc_client` 仍为空。
2. 同步请求进入 `send_request`。若 `routed.target == StoreTarget::TiDb`，则进入 `rpc_client()`：首个调用者执行工厂并把结果写入 `OnceLock`，后续调用者只克隆已保存的 `Arc`；随后请求被转发给该客户端。其他目标直接转发给 `mock_client`。
3. 异步请求进入 `send_request_async` 后，方法克隆重定向器的 `Arc`，新建一个 OS 线程，并在线程内复用同步 `send_request`；完成结果无论成功或失败都传给一次性 `callback`。
4. `set_event_listener` 不触碰第二客户端，只调用 `mock_client.set_event_listener`。即使第二客户端稍后初始化，监听器也不会自动复制过去。
5. `close` 与 `close_addr` 总是先处理 mock 侧。只有 mock 操作成功且 `rpc_client` 已初始化时，才继续处理第二客户端；从未发送 TiDB 请求时，关闭操作不会为了清理而初始化它。

## 数据与状态

`ClientRedirector` 自身没有可变路由表。路由状态完全来自每次调用携带的 `StoreTarget`，而持久状态只有两个共享客户端和一个工厂。`Arc` 负责跨调用及跨线程共享所有权；`OnceLock` 从“未初始化”单向转为“已初始化”，没有重置或替换接口。

`RoutedRequest`、`Request` 和 `Response` 都按值流动。`send_request` 将请求体移动给选中的客户端，`send_request_async` 又把地址、请求、超时和回调整体移动进新线程。底层 `RPCClient` 的当前实现会检查关闭状态、可选 failpoint 超时、请求拦截器和地址，再分派请求；这些行为属于 `pkg/store/mockstore/unistore/rpc.rs`，重定向器只原样传播其结果。

重要不变量是：成功初始化后所有 TiDB 目标共享同一个 `Arc<dyn KvClient>`；TiKv/TiFlash 请求永不触发工厂；关闭和监听器调用也不会触发工厂。

## 依赖与调用关系

上游模块边界是 `pkg/store/mockstore/lib.rs`，它公开 `redirector` 模块并再导出全部公开符号。RustCodeGraph 的 `explore`/文件节点结果及仓库检索均未发现 `ClientRedirector`、`RoutedRequest`、`StoreTarget` 或 `newClientRedirector` 的 Rust 调用者，因此当前没有可证实的 Rust 应用主链。

Go 对照实现已有真实接线：`pkg/store/mockstore/tikv.go::newMockTikvStore` 和 `pkg/store/mockstore/unistore.go::newUnistore` 把 `newClientRedirector(client)` 传给 client-go 的测试 TiKV store 构造函数。这个 Go 调用链可以说明文件的设计目的，但不能当作 Rust 已接线的证据。

下游依赖如下：

- `crate::embedded_unistore` 是 `lib.rs` 对 `astersql_store_mockstore_unistore` 的别名再导出。
- `crate::embedded_unistore::rpc::{RPCClient, Request, Response, RpcError}` 提供具体客户端、协议载荷和错误类型。
- `std::sync::{Arc, OnceLock}` 提供共享所有权和一次初始化；`std::thread::spawn` 提供异步入口的线程执行环境。
- `std::time::Duration` 被不加修改地传递给底层 `send_request`。

## 错误处理与边界

所有同步失败都使用 `RpcError` 原样返回，重定向器不包装、不记录也不重试。`send_request` 只做路由，底层错误直接成为结果；异步版本把同一结果交给回调。

`close` 和 `close_addr` 使用 `?` 保持明确的短路顺序：mock 侧失败时，不再尝试关闭第二客户端；mock 侧成功而第二客户端失败时，返回第二个错误。由此不能把一次错误返回解释为“两侧均已尝试清理”。尚未初始化的第二客户端不产生关闭错误。

`KvClient::set_event_listener` 的默认实现静默忽略监听器，因而实现者若需要事件可见性必须主动覆写。当前 `RPCClient` 适配没有覆写它。`send_request_async` 不返回 `JoinHandle`，调用者无法等待线程或直接观察线程 panic；若线程创建失败，标准库 `spawn` 会 panic。若工厂或回调 panic，panic 留在工作线程（同步调用工厂时则发生在调用线程），本文件没有恢复逻辑。地址合法性、超时语义和请求支持范围均由具体 `KvClient` 决定。

## 并发与资源生命周期

`KvClient: Send + Sync`、字段中的 `Arc` 以及工厂闭包的 `Send + Sync` 约束，使一个 `ClientRedirector` 可以被多个线程共享。`OnceLock::get_or_init` 对竞争初始化进行同步：只有一个成功产出的客户端被保存，其他线程取得同一个实例；若初始化闭包 panic，值不会被设置，之后的调用仍可再次尝试初始化。

每次 `send_request_async` 都创建一个独立 OS 线程，没有线程池、排队、取消、背压或并发上限。线程通过克隆重定向器 `Arc` 保证执行期间门面及其客户端存活，回调完成后该克隆释放。大量异步请求可能产生大量线程，这是扩展时必须评估的性能边界。

`ClientRedirector` 没有 `Drop` 实现，因此丢弃最后一个 `Arc` 不会显式调用 `close`；需要关闭底层资源的调用者必须主动调用 `close`。同时，`close` 与发送之间没有本文件级互斥，竞态结果由底层客户端的线程安全与关闭语义决定。嵌入式 `RPCClient::close` 自身用原子标志实现幂等关闭，但 `KvClient` trait 没有要求所有实现都具备该性质。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/redirector.go`：Go 的 `clientRedirector` 保存 `mockClient`、`sync.Once` 和可选 `rpcClient`；`SendRequest`/`SendRequestAsync` 在 `req.StoreTp == tikvrpc.TiDB` 时用全局集群安全配置惰性创建 client-go 网络客户端，其余请求走 mock。两版都保持了“一次初始化、TiDB 特判、mock 优先关闭、监听器只安装到 mock”的核心语义。

Rust 版存在以下已验证差异：

- Rust 通过调用者注入的 `rpc_factory` 创建第二客户端，不读取 `config.GetGlobalConfig().Security.ClusterSecurity()`，也不在本文件内保证产物一定是真实网络客户端。
- Rust 将目标从请求体拆成 `RoutedRequest::target`；Go 直接读取 `tikvrpc.Request.StoreTp`。
- Rust `send_request_async` 自建线程、接收显式 `timeout` 并最终调用同步方法；Go 直接调用底层客户端原生的 `SendRequestAsync`，其签名没有该超时参数。
- Rust API 使用自定义 `KvClient` 和嵌入式 UniStore RPC 类型，不是 client-go `tikv.Client` 的逐类型翻译。
- Go 版已被 `tikv.go` 与 `unistore.go` 使用；Rust 版目前只有模块导出，没有对应构造链调用者。

相关 Rust 测试目录中没有引用本文件公开符号的独立测试；Go 测试同样没有直接点名 redirector，现有覆盖至多来自通过 mockstore 构造路径的间接行为，不能视为针对路由分支、一次初始化或关闭次序的专门回归测试。

## 扩展指南

新增目标或改变路由策略时，应优先修改 `StoreTarget` 和 `ClientRedirector::send_request`，并在独立测试文件（建议同目录 `redirector_test.rs`，再由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入）覆盖每个目标；不要把测试内嵌到生产源文件。至少应使用可记录调用的 `KvClient` 替身验证：TiKv/TiFlash 不执行工厂、并发 TiDb 请求只成功初始化一次、请求/地址/超时完整转发、监听器只到 mock、两类关闭的顺序与错误短路。

若要把该实现接入 Rust mockstore 主链，必须先在 Rust 的 store 构造位置确认与 Go `tikv.go`/`unistore.go` 等价的客户端接口和安全配置来源，再调用 `newClientRedirector`；不能仅因 `lib.rs` 已再导出就认为迁移完成。若第二客户端需要事件监听，需明确改变 `set_event_listener` 的兼容语义并同步 Go 差异说明。

若异步调用量可能较大，应将每请求一线程替换为受控执行器或底层原生异步接口，并保留超时、回调恰好一次及关闭竞态的测试。改变 `close` 的“mock 失败即短路”策略可能影响兼容性和资源回收，必须先决定是否与 Go 保持一致。所有新增测试仍应放在独立 Rust 测试文件中。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；本次查询时索引可用。
- RustCodeGraph `explore "pkg/store/mockstore/redirector.rs redirector Redirector"`：返回 Rust/Go 两版完整源码、内部调用摘要，并报告相关符号没有覆盖测试。
- RustCodeGraph `node --file pkg/store/mockstore/redirector.rs --offset 1 --limit 220`：读取目标文件 177 行全貌，并报告文件 `used by 0 files`。
- `pkg/store/mockstore/redirector.rs`：核对全部枚举、结构体、trait、适配实现、构造函数、同步/异步路由、关闭和监听器逻辑。
- `pkg/store/mockstore/lib.rs` 与 `pkg/store/mockstore/Cargo.toml`：核对模块公开方式、crate 边界及 UniStore 依赖。
- `pkg/store/mockstore/unistore/rpc.rs` 与 `pkg/store/mockstore/unistore/Cargo.toml`：核对 `Request`、`Response`、`RpcError`、`RPCClient` 的实际定义，以及发送、异步发送和关闭行为。
- `pkg/store/mockstore/redirector.go`、`pkg/store/mockstore/tikv.go`、`pkg/store/mockstore/unistore.go`：核对 Go 对照语义及真实构造链调用点。
- 对 `pkg/store/mockstore` 执行公开符号和测试引用检索：除 `redirector.rs`、`redirector.go`、`lib.rs` 的导出及 Go 构造点外，未发现 Rust 调用者或直接 Rust/Go 测试。
- 本任务只创建说明文档；按计划不运行 Cargo。交付前另执行固定十一章节的结构验证。
