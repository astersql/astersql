# `pkg/inference/raw_http.rs`

## 文件定位

`raw_http.rs` 是 `astersql-inference` crate 内部的特殊 HTTP 传输实现，由 [`lib.rs`](lib.rs) 以私有模块 `raw_http` 装配。它不替代常规 `reqwest::Client`：只有 [`base.rs`](base.rs) 中 `ProviderEndpoint::raw_url()` 发现预先转义的路径（特别是完整的 `%2E` / `%2E%2E` 点段）会被 WHATWG URL 规范化改变时，`execute_json_embedding_call` 才调用本文件的 `post_json`；普通端点仍走已配置的 `reqwest` 客户端。

该旁路位于 provider 适配器与远端推理 HTTP 服务之间。当前会生成这种 `ProviderEndpoint::with_path` 的 Rust provider 是 `gemini.rs`、`huggingface.rs` 和 `tidbcloud.rs`。文件只负责构造并发送 JSON POST、代理连接、超时和有界读取；状态码解释、错误响应解码、凭据脱敏日志及 embedding 解码仍由 `base.rs::execute_json_embedding_call` 负责。

## 核心职责

- 保留调用方传入的原始转义 URI，避免重新经过 `reqwest::Url` 后丢失或折叠点段；依据是 `post_json_with_matcher` 直接把 `endpoint` 解析为 `hyper::Uri`，随后以该 URI 构造请求。
- 根据 `hyper_util::client::proxy::matcher::Matcher` 选择直连、HTTP 正向代理或 HTTPS CONNECT 隧道，并传递代理 Basic 鉴权。
- 将 `serde_json::Value` 序列化为请求体，强制 `Content-Type: application/json`，同时保留 provider 传入的其他头。
- 在统一超时内发送请求并流式读取响应，最多保留 `max_bytes + 1` 字节以区分“恰好达到上限”和“超过上限”。
- 把 URI、建连、请求及超时错误包装成不暴露端点的 `ProviderError::redacted("<provider> request failed", cause)`；响应帧错误保留其文本和底层原因。

## 主要符号

- `type BoxError = Box<dyn Error + Send + Sync>`：统一 connector、代理和 Hyper 客户端的异步错误类型。
- `ForwardProxy<C> { connector, proxy }`：HTTP 正向代理 connector。其 `Service<Uri>::call` 忽略请求目标，仅连接配置的代理 URI；原始目标仍保留在 HTTP 请求行中。
- `ProxyStream<T>`：连接流的透明包装。`Read`、`Write` 全部委托给底层流；`Connection::connected` 在底层连接元数据上设置 `proxy(true)`，告知 Hyper 对 HTTP 请求使用 absolute-form target。
- `post_json(endpoint, payload, headers, max_bytes, provider)`：crate 内生产入口。它从环境变量构造代理 matcher，并采用 `base::DEFAULT_HTTP_TIMEOUT`，随后委托给可注入依赖的实现函数。
- `post_json_with_matcher(..., matcher, timeout)`：完整实现，也是独立 Rust 测试的直接入口。额外参数让测试无需修改进程代理环境即可覆盖代理与超时分支。

这些符号均为 crate 私有或文件私有，没有对 crate 使用者形成公开 API。

## 执行流程

1. `post_json` 创建 `Matcher::from_env()`，带上默认 30 秒超时调用 `post_json_with_matcher`。
2. 实现函数创建统一的 `failed` 错误包装闭包，将原始 endpoint 解析为 `hyper::Uri`，并让 matcher 判断是否代理。
3. 使用 Rustls ring provider 和 WebPKI roots 构造同时支持 HTTP/HTTPS、HTTP/1 和 HTTP/2 的 connector；把 payload 序列化为字节，并覆盖 `Content-Type`。
4. 若是带 Basic 认证的 HTTP 代理，把 `Proxy-Authorization` 放入发往代理的请求头。随后使用原始 `Uri` 生成 POST 请求，因此转义路径不会被第二次 URL 规范化。
5. 发送阶段分三路：HTTPS 代理用 `Tunnel` 先向代理发 CONNECT，再在隧道内建立 TLS；HTTP 代理用 `ForwardProxy` 连接代理并以 absolute-form 发送原目标；无代理则用基础 connector 直连目标。
6. 取得响应后先保存状态码。负数 `max_bytes` 立即报错；否则逐帧读取 data frame，每次只追加剩余配额（含用于探测越界的一个字节），一旦长度大于上限即报错。非 data frame 不加入结果。
7. 整个建连、发送和读取 operation 被 `tokio::time::timeout` 包裹。成功返回 `(StatusCode, Vec<u8>)`；HTTP 非 2xx 本身不在此层报错，由上层按 provider 规则解释。

## 数据与状态

本文件没有全局可变状态。每次调用都会创建 connector、Hyper client、请求和响应缓冲区；输入的 `HeaderMap` 按值传入并在本地补充/覆盖头。`ForwardProxy` 可克隆，保存一个底层 connector 和匹配所得代理 URI；`ProxyStream` 只拥有底层连接流。

响应体累积在 `Vec<u8>` 中，不预分配 `max_bytes`，并通过至多读取 `max_bytes + 1` 字节限制内存增长。实现使用 `saturating_add` / `saturating_sub`，避免上限加一或剩余量计算溢出。返回状态与原始响应字节，不保存 provider 响应的业务结构。

## 依赖与调用关系

上游链路是 provider（`gemini.rs`、`huggingface.rs`、`tidbcloud.rs`）构造 `ProviderEndpoint::with_path` → `base.rs::execute_json_embedding_call` → `ProviderEndpoint::raw_url` → `raw_http::post_json`。`base_test.rs` 直接调用 `post_json_with_matcher` 注入 matcher 和短超时。

下游依赖由 [`Cargo.toml`](Cargo.toml) 声明：`hyper` 提供 URI、请求及 IO trait，`hyper-util` 提供 legacy client、代理 matcher、CONNECT tunnel 和 Tokio executor，`hyper-rustls` / `rustls` 提供 TLS，`http-body-util` 提供响应 frame 流，`tower-service` 提供 connector 的 `Service` 契约，`tokio` 提供超时，`reqwest` 复用 HeaderMap、StatusCode 和 Body 类型，`serde_json` 负责请求序列化。crate 内依赖 `base::{DEFAULT_HTTP_TIMEOUT, ProviderError}`。

RustCodeGraph 能定位文件及 `post_json`、`post_json_with_matcher`、`ForwardProxy`、`ProxyStream`，但对这些符号返回的 callers/callees 为空；上述直接调用边由仓库精确搜索补证，而不是据空图推断“无人使用”。

## 错误处理与边界

- endpoint 不是合法 `hyper::Uri`、请求构造失败、connector/Hyper 请求失败或超时：外显消息统一为 `"{provider} request failed"`，底层错误作为 `ProviderError` 的 source，避免把可能含凭据的 URL 放进显示文本。
- JSON 序列化失败：外显固定消息 `unexpected marshal request error`，保留序列化错误原因。当前入口接收 `serde_json::Value`，通常可序列化；该分支仍是防御性边界。
- `max_bytes < 0`：返回 `maximum response body size must not be negative`。恰好等于上限允许，超过一个字节即返回 `response body exceeds maximum size of N bytes`；该限制对成功与错误状态响应同样生效。
- 响应 frame 读取失败：以底层错误文本作为显示消息并保留 source。只有 data frame 被聚合；trailers 等非 data frame 被忽略。
- 本层不要求 200，也不重试、不解析 JSON 响应、不执行 provider 状态映射。调用者必须继续走 `execute_json_embedding_call` 的后续生命周期。
- `HttpsConnectorBuilder::with_provider_and_webpki_roots(...).expect(...)` 假定内置 ring/WebPKI 配置可构造；若这一静态 TLS 配置失效会 panic，而不是返回 `ProviderError`。

## 并发与资源生命周期

两个入口都是 `async fn`，自身不创建后台任务、线程、锁或通道；socket、TLS 隧道、请求体与响应体都由局部所有权和 future drop 管理。`tokio::time::timeout` 到期会丢弃 operation，从而取消尚未完成的连接、请求或读取；测试确认 source 是 `tokio::time::error::Elapsed`。

每次特殊请求新建 client/connector，因此本文件内没有跨请求连接池复用。生产入口由 `base.rs` 创建的 current-thread Tokio runtime 驱动；上层还负责在调用前检查取消原因，并在旁路返回错误时优先恢复调用方取消原因。connector 的 `Send + 'static` 和底层流的 `Read + Write + Connection + Unpin + Send` 约束保证 boxed future 可在 Tokio 调度契约下安全轮询。

## 与 Go 版本的对应关系

Go 没有单独的 `raw_http.go`；对应行为分布在 `pkg/inference/embedding/base/base.go`：`SetEscapedURLPath` 同时设置 `url.URL.Path` 与 `RawPath`，`PostJSON` 构造 JSON 请求，`DoRequest` 使用标准 `http.Client`（自然支持环境代理、HTTP absolute-form 和 HTTPS CONNECT）并调用 `ReadResponseBody` 做有界读取。

Rust 的 `ProviderEndpoint` 保存 `escaped_path`，当 `reqwest::Url` 的当前 path 与它不一致时才转到本文件，弥补 Rust 常规客户端对 WHATWG 点段规范化的差异。两版保持的语义包括：JSON Content-Type 覆盖调用方冲突值、返回原始状态码和响应体、负上限报错、上限恰好可接受、超限报错、默认 30 秒请求超时以及安全的 provider 请求错误。Rust 特有代码显式实现了代理 connector 和 TLS 隧道；这是传输栈差异，不代表 Go 缺少代理能力。

Go 的直接测试位于 `pkg/inference/embedding/base/base_test.go`，覆盖 `SetEscapedURLPath`、`PostJSON`、`DoRequest` 与 `ReadResponseBody`。Rust 的代理 absolute target、CONNECT 认证和超时细节则由 `pkg/inference/base_test.rs` 独立覆盖。

## 扩展指南

- 新增会被 URL 规范化破坏的 provider 路径时，应继续通过 `ProviderEndpoint::with_path` 接入，而不是让 provider 直接调用本文件；这样普通 URL 仍可复用配置好的 `reqwest::Client` 和上层取消/错误生命周期。
- 修改代理行为时，优先调整 `ForwardProxy::call`、`ProxyStream::connected` 或 HTTPS `Tunnel` 分支，并在 `base_test.rs::escaped_paths_preserve_proxy_absolute_targets_and_connect_authentication` 增加对应 HTTP/HTTPS 和鉴权断言。注意代理凭据不得进入错误显示或普通目标服务器请求头。
- 修改体积限制或流式读取时，应同步 Rust 测试 `escaped_request_timeouts_and_body_bounds_keep_the_real_error_lifecycle`，并对照 Go 的 `TestReadResponseBody`；必须保留“等于上限成功、超过上限失败”和极大上限不溢出的不变量。
- 修改超时或错误包装时，应检查 `ProviderError` 的 source 链及上层 `ProviderContext` 取消优先级，避免将 endpoint、query token 或代理凭据暴露到错误文本。
- 若希望复用连接池或统一普通/特殊传输，需要先证明不会重新规范化原始 URI，并评估每次新建 TLS/client 的性能与环境代理兼容性；这属于跨 `raw_http.rs`、`base.rs` 及 provider 测试的行为变更。
- 测试逻辑必须继续放在独立 `base_test.rs`（或新的独立 `*_test.rs`）中，不应内嵌到生产源文件。

## 验证依据

- 源码：`pkg/inference/raw_http.rs` 全部 205 行；`pkg/inference/base.rs` 中 `ProviderEndpoint::{with_path, raw_url}`、`DEFAULT_HTTP_TIMEOUT`、`ProviderError` 与 `execute_json_embedding_call`；`pkg/inference/lib.rs` 的私有模块声明。
- crate/依赖：`pkg/inference/Cargo.toml` 的 `[lib]`、HTTP/TLS/Tokio/JSON 依赖和 `package.metadata.porting.go-package = "pkg/inference"`。
- Rust 入口与测试：`pkg/inference/gemini.rs`、`huggingface.rs`、`tidbcloud.rs` 对 `ProviderEndpoint::with_path` 的调用；`pkg/inference/base_test.rs::escaped_paths_preserve_proxy_absolute_targets_and_connect_authentication` 与 `escaped_request_timeouts_and_body_bounds_keep_the_real_error_lifecycle`。
- Go 对照：`pkg/inference/embedding/base/base.go::{SetEscapedURLPath, NewJSONRequest, DoRequest, PostJSON, ReadResponseBody}`；`pkg/inference/embedding/base/base_test.go::{TestReadResponseBody, TestHTTPHelpers}`。
- RustCodeGraph：索引状态为已初始化（查询时 11,467 个文件）；`node --file pkg/inference/raw_http.rs` 读取完整源码，`query` 唯一定位两个函数和两个结构体；精确 callers/callees 查询为空，故再以 `rg` 验证 `base.rs`、`base_test.rs` 和 provider 的实际引用。
- 本任务是纯文档分析，未运行 Cargo；交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工检查唯一新增生产物、链接、符号名和边界描述。
