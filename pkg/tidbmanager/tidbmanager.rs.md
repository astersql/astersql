# `pkg/tidbmanager/tidbmanager.rs`

## 文件定位

本文件是 Cargo 包 `astersql-tidbmanager` 的业务实现，提供向 TiDB Manager 控制面发送“当前 Pod 已可回收复用”通知的阻塞式 HTTP 客户端。crate 入口 `pkg/tidbmanager/lib.rs` 通过 `pub mod tidbmanager` 和 `pub use tidbmanager::*` 导出这里的 API；根门面 `pkg/lib.rs` 又经 `facade_tidbmanager` 再导出该 crate。包清单 `pkg/tidbmanager/Cargo.toml` 声明运行时依赖 `reqwest`（仅启用 `blocking`、`rustls-tls`）、`url` 和 `thiserror`。

当前接线状态需要与实现能力区分：仓库内对 `ManagerClient`、`new_client` 和 `Client::free` 的直接 Rust 引用仅见本 crate 的两个独立测试文件。虽然 `pkg/standby/Cargo.toml` 声明了 `astersql-tidbmanager` 依赖，但 `pkg/standby/standby.rs` 目前定义的是另一个本地 `ManagerClient` trait，尚未为本文件的客户端提供适配实现；`cmd/tidb-server/main.rs::createMgrClientForStarter` 使用的也是 `cmd/tidb-server/stubs.rs::tidbmanager` 桩，而不是本文件。因此，本文件目前是可用且有测试覆盖的独立客户端实现，但尚未进入 Rust `tidb-server` 的实际 starter 主链。

## 核心职责

- `new_client` 根据是否提供 `reqwest::blocking::ClientBuilder` 选择 `http://` 或 `https://`，为客户端统一设置 `DEFAULT_TIMEOUT`（10 秒），并保存 Manager 地址和 Pod 身份。
- `ManagerClient::free` 组装 `PUT /api/tidb/free` 请求，在查询串中发送 `pod_name`、`pod_ip`、`ns` 和 `normal_restart_log` 四个与 Go 版本一致的字段。
- `ReqwestTransport` 执行真实的阻塞式 PUT；`HttpTransport` 与 `HttpResponse` 将传输和响应 body 抽象出来，使错误响应体读取失败等边界可以由独立测试精确注入。
- `ManagerError` 保留建 URL、建 HTTP client、发送请求、非 200 响应以及读取错误响应体失败这五类上下文。

该文件只负责一次通知请求，不负责重试、退避、日志记录、Pod 状态机或 Manager 服务发现。

## 主要符号

- `FREE_REQ_PATH: &str = "/api/tidb/free"`：Manager free API 的固定路径。
- `DEFAULT_TIMEOUT: Duration`：真实 `reqwest` 客户端的总请求超时，值为 10 秒。
- `Result<T>`：统一为 `std::result::Result<T, ManagerError>`。
- `ManagerError`：
  - `CreateRequest(url::ParseError)`：在 `free` 中解析完整 URL 失败；
  - `CreateClient(reqwest::Error)`：构造 reqwest client 失败；
  - `FreeRequest { source: BoxError }`：传输层发送失败；
  - `FreeResponse { status, body }`：收到非 200 且成功读完 body；
  - `ReadResponse { status, source, partial_body }`：非 200 响应的 body 未能读完，同时保留 I/O source 和已读部分。
- `HttpResponse`：内部保存 `reqwest::StatusCode` 与 `Box<dyn Read + Send>`；公开构造器 `HttpResponse::new` 主要服务于传输实现和测试替身，字段本身不公开。
- `HttpTransport: Send + Sync`：单方法抽象 `put(Url) -> Result<HttpResponse>`。这既是 Go `http.RoundTripper` 测试边界的 Rust 对应物，也是并发共享传输所需的线程安全约束。
- `ReqwestTransport`：私有默认实现，持有 `reqwest::blocking::Client`，调用 `.put(url).send()` 并把响应包装为 `HttpResponse`。
- `Client`：对外能力 trait，目前只有 `free(&self, exit_reason: &str) -> Result<()>`。
- `ManagerClient`：具体客户端，持有 `Arc<dyn HttpTransport>`、规范化后的 Manager 地址及 Pod 名称、IP、namespace。
- `status_text`：把状态码格式化为 `"<code> <canonical reason>"`；没有标准 reason 时只返回数字。
- `new_client`：生产构造入口，创建 reqwest transport，并按 TLS builder 是否存在确定 scheme。
- `new_client_with_transport`：传输注入入口；去除地址末尾 `/` 后构造 `ManagerClient`。它当前没有会失败的分支，但返回 `Result` 以保持与构造链一致。

## 执行流程

1. 调用者执行 `new_client(addr, tls_config, pod_name, pod_ip, namespace)`。函数先记录 builder 是否存在，再用传入 builder或 `HttpClient::builder()` 创建阻塞式 client，覆盖其 timeout 为 10 秒。builder 存在即选 `https://`，否则选 `http://`。
2. `addr.trim_end_matches('/')` 去除所有尾随斜杠，前置 scheme 后交给 `new_client_with_transport`；后者再次去尾斜杠并复制 Pod 三元组，真实传输以 `Arc` 保存。
3. 调用 `Client::free(exit_reason)` 时，`ManagerClient` 将 `manager_addr` 与 `FREE_REQ_PATH` 拼接并用 `Url::parse` 校验。注意地址合法性是在这里而不是构造时确认。
4. `query_pairs_mut().append_pair` 依次追加四个查询参数；`url` crate 负责必要的百分号编码，所以空格等特殊字符不会直接破坏 URL。
5. `HttpTransport::put` 发出无请求体的 PUT。默认 `ReqwestTransport` 把 reqwest 发送错误映射为 `ManagerError::FreeRequest`。
6. 响应恰为 `StatusCode::OK`（200）时立即成功，不读取 body。
7. 其他所有状态（包括其他 2xx）都视为失败。代码先生成稳定的状态文本，再用 `Read::read_to_end` 读取全部响应体：成功则返回 `FreeResponse`；失败则返回 `ReadResponse`，其中包含 I/O source 和已读取字节按 UTF-8 lossy 转换后的文本。

RustCodeGraph 的 callee 结果确认 `new_client` 进入 `new_client_with_transport`，并在构造/传输路径调用 `HttpResponse::new`；仓库文本检索则确认生产代码尚无本文件入口的直接调用者。

## 数据与状态

`ManagerClient` 的状态在构造后不可变：传输对象通过 `Arc` 共享，地址和 Pod 身份均为拥有所有权的 `String`。每次 `free` 都从这些字段新建一个 `Url`，不会缓存请求或修改客户端状态。`exit_reason` 只在本次调用期间借用，并被编码到 `normal_restart_log` 查询参数。

`HttpResponse` 拥有响应 body reader。成功状态下它随局部变量离开作用域而释放；失败状态下 body 被同步读到 `Vec<u8>`，再用 `String::from_utf8_lossy` 转换。因此任意字节响应都可进入错误文本，无效 UTF-8 会被替换字符表示。当前实现不限制错误 body 大小，也不会保留原始字节。

## 依赖与调用关系

上游边界如下：

- `pkg/tidbmanager/lib.rs` 导出本文件全部公开符号；根 `Cargo.toml` 以 `facade_tidbmanager` 指向该 crate，`pkg/lib.rs::tidbmanager` 再导出门面。
- `pkg/tidbmanager/tidbmanager_test.rs` 和 `pkg/tidbmanager/migration_aster_unit_test.rs` 是已确认的直接调用者，覆盖 `new_client`、`new_client_with_transport` 和 `free`。
- `pkg/standby/Cargo.toml` 有 crate 依赖，但其当前源文件没有消费本文件符号；若要接入 standby，需要显式实现/适配 `pkg/standby/standby.rs::ManagerClient`。
- `cmd/tidb-server/main.rs` 当前从 `crate::stubs` 导入 `tidbmanager`，所以同名 `NewClient` 不构成本文件的调用边。

下游依赖如下：

- `reqwest::blocking::{Client, ClientBuilder}`：阻塞 HTTP、超时与 rustls TLS 配置；
- `url::Url`：基础 URL 解析与查询参数编码；
- `thiserror::Error`：错误展示文本和 source 链；
- 标准库 `Read`：完整读取错误响应体；`Arc`：共享传输对象；`Duration`：超时常量。

## 错误处理与边界

错误分类保留了发生阶段，调用者可以匹配 `ManagerError` 变体，而不必解析字符串。`CreateRequest` 与 `CreateClient` 直接保留具体第三方错误；`FreeRequest` 使用 `Box<dyn Error + Send + Sync>`，允许自定义 transport 返回非 reqwest 错误；`ReadResponse` 保留可由 `Error::source` 追踪的 `io::Error`。

重要边界包括：

- 只有 200 成功，201、202、204 等均按 `FreeResponse` 处理。
- `new_client` 只按 builder 是否存在决定 scheme，不检查传入 `addr` 是否已经含 scheme；若传入 `http://host`，会形成 `http://http://host` 并在 `free` 时失败。因此生产调用约定是传裸 `host:port`。
- `new_client_with_transport` 面向已经带 scheme 的完整基础地址；测试使用 `http://manager.example.com`。它不验证地址，解析仍延迟到 `free`。
- 非 200 body 采用无上限 `read_to_end`，不可信 Manager 若返回巨大响应可能增加内存占用；扩展时应评估大小上限，但不能在不更新 Go 兼容预期和测试的情况下悄然改变错误内容。
- trait API 没有 Go 版 `context.Context` 参数。取消能力仅来自 reqwest client 的固定超时，调用者无法逐请求取消或设置 deadline，这是当前 Rust/Go 行为差异。
- 成功路径不读取响应 body；资源释放依赖拥有 body 的 `HttpResponse` 被 drop。失败路径同样没有显式 `close` 方法，由 Rust 所有权释放底层响应。

## 并发与资源生命周期

该实现不创建线程、异步任务、锁、通道或重试定时器；每次 `free` 都在调用线程上阻塞直到响应、错误或 10 秒超时。`HttpTransport` 强制 `Send + Sync`，并存于 `Arc` 中，因此具体 `ManagerClient` 的不可变字段和默认 reqwest client 可由外部并发共享；不过公开 `Client` trait 自身没有声明 `Send + Sync`，需要 trait object 跨线程时应显式补充约束或使用具体类型。

请求与响应均为调用级临时资源：`Url`、响应包装和错误 body buffer 在一次 `free` 内创建；`ManagerClient` 长期持有并复用底层 reqwest connection pool。错误返回前 body reader 被读完或因读取失败而释放，成功返回时响应直接释放。测试中的一次性 HTTP server 线程属于测试设施，不是生产生命周期的一部分。

## 与 Go 版本的对应关系

`pkg/tidbmanager/tidbmanager.go` 是直接语义基准：

- `freeReqPath`/`DefaultTimeout` 对应 `FREE_REQ_PATH`/`DEFAULT_TIMEOUT`，值分别为 `/api/tidb/free` 和 10 秒。
- Go `Client.Free(ctx, exitReason)` 对应 Rust `Client::free(exit_reason)`；HTTP 方法、路径、四个查询字段、仅接受 200、非 200 时携带状态和 body、读取失败时携带 partial body 的行为一致。
- Go `NewClient` 在 TLS config 非空时新建启用 HTTP/2 的 `http.Transport`；Rust 接受整个 `ClientBuilder`，使用 rustls TLS，具体 TLS/HTTP 配置由调用者在 builder 上设置。两者都以 TLS 配置是否存在选择 HTTPS。
- Go 通过 `context.Context` 支持每次请求取消，且 `defer resp.Body.Close()` 显式关闭响应；Rust 没有 per-request context，依赖固定 timeout 和 drop。
- Go `NewClient` 返回接口且构造不返回错误；Rust `new_client` 返回具体 `ManagerClient` 的 `Result`，因为 reqwest client 构建可能失败。
- Go 测试通过替换 `http.Client.Transport` 注入 `errorRoundTripper`；Rust 用公开 `HttpTransport` 与 `new_client_with_transport` 达到同一测试边界。

`pkg/tidbmanager/tidbmanager_test.rs` 逐项复刻 Go 测试的成功请求、503 状态/body 和读取失败场景；`migration_aster_unit_test.rs` 以原始 TCP server 再验证请求行及 error source 链。两套 Rust 测试都与生产源码分文件，符合仓库测试布局要求。

## 扩展指南

- 新增 Manager API 时，优先在 `Client` trait 增加语义方法，在 `ManagerClient` impl 中组装请求，并复用或有意识地扩展 `HttpTransport`。若新 API 不仅是 PUT，应把传输 trait 扩成明确的方法/请求结构，而不是绕过注入边界直接调用 reqwest。
- 改动 URL、参数名、成功状态或错误文本时，必须同时核对 `pkg/tidbmanager/tidbmanager.go`、`tidbmanager_test.go`，并更新独立 Rust 测试 `tidbmanager_test.rs`；迁移契约相关覆盖还应同步 `migration_aster_unit_test.rs`。
- 接入 starter/standby 主链时，不应仅依赖根门面已导出这一事实。需要移除或绕开 `cmd/tidb-server/stubs.rs::tidbmanager` 的同名桩，并为 `pkg/standby/standby.rs::ManagerClient` 提供适配；同时验证阻塞调用不会占用不允许阻塞的执行线程。
- 添加重试时应明确哪些错误可重试、最大尝试次数和幂等性；当前 free 是 PUT，但不能仅凭 HTTP 方法假定 Manager 端任意重复调用都无副作用。
- 若增加响应体上限、逐请求取消或异步实现，应把 Go 兼容性差异写入测试，并保留 `ManagerError` 的 source 链和 partial body 诊断价值。
- 生产源码修改后应按仓库规则先执行 `cargo fmt --all`；测试逻辑继续放在同目录独立测试文件，不要内嵌进 `tidbmanager.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/tidbmanager/tidbmanager.rs`（15 个符号）；`files --filter pkg/tidbmanager` 列出实现、crate 入口、Go 对照及两份 Rust 测试；`node --file pkg/tidbmanager/tidbmanager.rs --offset 1 --limit 260` 覆盖源文件全部 205 行；`query TiDBManager --limit 20` 定位本文件公开/内部符号；`callees new_client --limit 30` 确认本文件构造调用边。`callers` 查询未返回可用调用边，因此按技能规则用仓库文本检索补证。
- 实现与装配：`pkg/tidbmanager/tidbmanager.rs`、`pkg/tidbmanager/lib.rs`、`pkg/tidbmanager/Cargo.toml`、根 `Cargo.toml` 的 `facade_tidbmanager`、`pkg/lib.rs::tidbmanager`。
- Go 对照：`pkg/tidbmanager/tidbmanager.go`、`pkg/tidbmanager/tidbmanager_test.go`。
- Rust 测试：`pkg/tidbmanager/tidbmanager_test.rs`、`pkg/tidbmanager/migration_aster_unit_test.rs`。
- 当前接线核验：`pkg/standby/Cargo.toml`、`pkg/standby/standby.rs::ManagerClient`、`cmd/tidb-server/main.rs::createMgrClientForStarter`、`cmd/tidb-server/stubs.rs::tidbmanager`，以及对 `ManagerClient|HttpTransport|FREE_REQ_PATH|astersql_tidbmanager` 的全仓 Rust 检索。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付验证只检查固定章节、链接/路径事实和最终差异。
