# `pkg/inference/gemini.rs`

## 文件定位

`gemini.rs` 是 `astersql-inference` crate 中的 Gemini 文本嵌入提供者适配器。crate 由 `pkg/inference/Cargo.toml` 声明，库入口 `pkg/inference/lib.rs` 通过 `pub mod gemini` 公开此模块，并从 `embed_fn` 重导出共享的 `Embedder` 和 `Options`。

应用接线位于 `pkg/domain/domain.rs` 的 embedding provider 初始化段：它以名称 `"gemini"` 向 `EmbedFn::register` 注册 `GeminiEmbedder::new` 创建的对象，API key 闭包每次从 Domain 的全局系统变量 `tidb_exp_embed_gemini_api_key` 取值，base URL 闭包返回空字符串，因而使用本文件的 Gemini 默认地址。这个文件不负责 SQL 函数解析、跨调用合批、缓存或 provider 选择；这些由 `pkg/inference/embed_fn.rs` 的 `EmbedFn` 负责。

## 核心职责

- 用 `GeminiEmbedder::new` / `with_config` 构造可复用的 HTTP client 和 API-key provider 配置。
- 用 `GeminiEmbedder::endpoint` 将 base URL 与经 URL path-segment 转义的模型名拼成 `/<model>:batchEmbedContents`。
- 在 `Embedder::create_embeddings_with_context` 中为每条文本构造 Gemini `requests` 元素，并调用共享 HTTP 执行层。
- 用 `decode_embeddings` 校验响应顶层形状、嵌入数量及浮点行，保持输入与输出的一对一顺序。
- 把 key 缺失、HTTP 401、Gemini JSON 错误体和解码失败转换为 provider 边界的 `ProviderError`，并将 secret 交给共享层脱敏。

## 主要符号

- `DEFAULT_BASE_URL: &str`：默认为 `https://generativelanguage.googleapis.com/v1beta/models`；仅在配置的 base URL `trim()` 后为空时使用。
- `GeminiEmbedder`：公开 provider 类型。`client: reqwest::Client` 在 crate 内可见，便于独立测试和内部接线；`cfg: APIKeyProviderConfig` 为私有配置。
- `GeminiEmbedder::new(api_key, base_url) -> Self`：面向 Domain 的便捷构造器。它安装可动态取值的 `Send + Sync + 'static` 闭包，并预置包含 SQL 配置方式的 key 缺失/401 提示。
- `GeminiEmbedder::with_config(cfg) -> Self`：面向测试和自定义接线的构造器；通过 `base::http_client("Gemini")` 建立带 30 秒默认超时的 client，并调用 `APIKeyProviderConfig::with_defaults` 补齐默认响应体上限。
- `GeminiEmbedder::endpoint(&self, model) -> Result<ProviderEndpoint, ProviderError>`：校验 HTTP(S) base URL，保留其 query，拼接转义后的模型路径及 `:batchEmbedContents`。`.` 和 `..` 特别保留，最终仍由共享 endpoint/HTTP 路径处理。
- `Embedder for GeminiEmbedder`：对外实现同步 provider trait。`create_embeddings` 是兼容只接收 `AtomicBool` 的旧入口；`create_embeddings_with_context` 是保留取消 cause 的完整入口。
- `decode_embeddings(body, expected)`：文件私有的成功响应解码器，返回 `Vec<Vec<f32>>`。

## 执行流程

1. `EmbedFn` 根据 provider 名找到已注册的 `Arc<dyn Embedder>`，在其批处理任务 `run_batch` 中按最大 batch size 切分文本，再调用 trait 入口（`pkg/inference/embed_fn.rs`）。
2. `create_embeddings` 把 `AtomicBool` 包装成 `ProviderContext::new`，调用 `create_embeddings_with_context`，并把结构化错误降为字符串；直接使用 context 的调用者则可保留 cause。
3. 完整入口先处理两个无网络分支：`texts.is_empty()` 返回空结果；非空文本但 `model.is_empty()` 返回 `model name is required`。因此空文本优先于模型名校验。
4. 通过 `cfg.resolve_api_key` 取得 key，再用 `endpoint(model)` 解析地址。模型名在 URL path 中转义，但 JSON 中保留原值并加上 `models/` 前缀。
5. 对每个输入构造 `{"model":"models/<model>","content":{"parts":[{"text":...}]}}`。`base::json_fields_with_options` 先复制 `opts` 再写入必需字段，所以用户选项可新增 `outputDimensionality` 等字段，但不能覆盖 `model` 或 `content`。所有元素被包在顶层 `requests` 数组中。
6. `base::provider_auth_headers(..., true)` 构造 `x-goog-api-key` header，然后 `base::execute_json_embedding_call` 发出 JSON POST。共享层在发送前及等待中检查取消，限制响应体大小，仅把 HTTP 200 当作成功。
7. 成功时 `decode_embeddings` 解析 JSON，要求 `embeddings` 数量与输入数量完全相等，再按顺序将每个 `values` 解码为 `f32` 行。失败状态会从 `error.message` 提取信息；401 优先返回配置的 unauthorized 错误，其他状态返回含 provider、状态码和已脱敏消息的错误。

## 数据与状态

`GeminiEmbedder` 的持久状态只有可复用的 `reqwest::Client` 和一份 `APIKeyProviderConfig`。配置中的 API key/base URL 是 `Arc<dyn Fn() -> String + Send + Sync>`，因此不在构造时快照 secret，而是每次调用时解析当前值。`max_response_bytes` 非正值在 `with_defaults` 中变为 32 MiB。

每次调用的 `requests`、`payload`、endpoint、key 和响应矩阵都是局部值，本文件没有自身缓存、全局可变状态或返回向量维度限制。输出的顺序不另行重排，由 Gemini 响应数组顺序决定；长度校验只证明数量对应，不能验证服务端是否交换了同数量元素。

## 依赖与调用关系

上游调用链的直接证据是：

- `pkg/domain/domain.rs` 构造 `GeminiEmbedder::new` 并注册到 `EmbedFn`。
- `pkg/inference/embed_fn.rs::run_batch` 对 provider trait object 调用 `create_embeddings_with_values`；trait 的默认实现转入本文件实现的 `create_embeddings`。
- `pkg/inference/lib.rs` 负责模块公开与 trait/options 重导出。

下游主要依赖 `pkg/inference/base.rs`：`APIKeyProviderConfig`/`ProviderContext`/`ProviderError`、`http_client`、`parse_http_url`、`escape_url_path_segment`、`ProviderEndpoint`、`json_fields_with_options`、`provider_auth_headers`、`execute_json_embedding_call`、`ensure_json_object`、`string_field` 和 `decode_float_row`。直接外部类型来自 `reqwest` 、`serde_json` 和标准库 `Arc`/`AtomicBool`；`pkg/inference/Cargo.toml` 明确声明了 `reqwest` blocking/JSON/rustls 功能、`serde_json`、`tokio` 及共享执行层使用的 HTTP/TLS/日志依赖，本文件没有条件编译分支。

## 错误处理与边界

- 空 `texts` 立即成功返回空向量，不解析 key、model 或 endpoint；非空请求必须有非空 model。
- 空 key 返回配置的 `missing_key_error` 或 fallback。base URL 只允许 HTTP(S) 绝对 URL；模型名在 URL 中按单个 path segment 转义，防止 `/` 或 `?` 改变路由语义。
- 选项不能覆盖 provider 必需的 `model` 和 `content`；其他选项未由本文件做 schema 校验，是否有效由 Gemini API 决定。
- 错误 JSON 解码器允许 `error` 为 null 或 object，但 `status`/`message` 非 null 时必须是字符串，`code` 非 null 时必须是 i64。无法解码时共享层使用 HTTP status text。
- 成功体必须是 JSON object。缺失或 null `embeddings` 按空数组处理，因而只有预期数量也为零时能通过；数量不等立即报错。`values` null 解码为空行，数组中的 null 数值解码为 `0.0`，非数值或转成 `f32` 后非有限值会失败（共享 `decode_float_row` 语义）。
- 共享执行层限制响应大小，脱敏 key，在运输错误中保留可检查 cause，并用配置的错误覆盖 401。本文件没有重试或退避逻辑。

## 并发与资源生命周期

`GeminiEmbedder` 满足 `Embedder: Send + Sync`；它持有可克隆、内部管理连接池的 `reqwest::Client`，配置 getter 也必须 `Send + Sync`。本文件不建立线程、锁或持久 Tokio runtime；合批线程、共享请求、缓存与关闭/join 生命周期属于 `EmbedFn`。

每次 provider 请求在 `base::execute_json_embedding_call` 中创建 current-thread Tokio runtime，并在同步调用内 `block_on` 网络 future。该 future 与每 5 ms 轮询 `ProviderContext::cause()` 的取消 future 竞争；取消获胜时请求 future 被丢弃并返回 caller cause 或 `request canceled`。HTTP client 跟随 `GeminiEmbedder` 存活，单次 payload、runtime 和 response body 在调用结束时释放。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/inference/embedding/gemini/gemini.go`。Rust 的 `GeminiEmbedder`/`new`/`endpoint`/`decode_embeddings`/`create_embeddings_with_context` 分别对应 Go 的 `Embedder`/`NewGeminiEmbedder`/`batchEmbeddingsEndpoint`/`decodeEmbeddings`/`CreateEmbeddings`。两者共享以下语义：默认 URL，`batchEmbedContents` 路由，模型 path 转义，每文本一个 request，必需字段不可被 options 覆盖，`x-goog-api-key` 认证，响应数量严格匹配，以及错误 message 提取。

Rust 版将 Go `context.Context` 的取消语义映射为 `ProviderContext` + `AtomicBool`，并用 `ProviderError` 单独保留 cause；兼容 trait 入口仍返回字符串。Rust 的 JSON 解码是基于 `serde_json::Value` 的显式形状/数值检查，Go 则解码到 `BatchResponse`/`ErrorResponse` 结构。Rust 还统一继承 `base.rs` 的响应体上限、secret 脱敏、transport cause 和取消检查。

Go 独立测试 `pkg/inference/embedding/gemini/gemini_test.go` 覆盖成功请求、options、路径转义、无效 key/model、缺失 key、endpoint 校验、数量不匹配和共享 provider contract。Rust 独立测试 `pkg/inference/gemini_test.rs` 保留了这些协议要点，并通过 `pkg/inference/cohere_test.rs` 的共享帮助器校验限额、取消、cause 和端点错误。

## 扩展指南

- 新增 Gemini 请求选项时，优先保持 `Options` 透传，并确认 `json_fields_with_options` 的“必需字段最后写入”不变；在 `pkg/inference/gemini_test.rs` 增加独立回归，必要时同步 Go 对照测试。不应把 Rust 单元测试内嵌到本生产文件。
- 修改 endpoint 时聚焦 `GeminiEmbedder::endpoint`，必须同时验证 base URL 的 query/尾斜杠、模型名中的空格、`/`、`?`、`.`/`..` 和 raw URL 发送路径；错误的转义可能导致路由注入或模型不可达。
- 修改响应协议时聚焦 `decode_embeddings` 和错误解码闭包，保留输入/输出数量不变量、`f32` 溢出检查、错误体脱敏和 401 自定义错误。若 Gemini API 增加部分成功语义，需先明确 `EmbedFn` 对“每个输入必有一行”的依赖。
- 修改 key/base URL 来源时需同步 `GeminiEmbedder::new` 与 `pkg/domain/domain.rs` 的注册闭包，并评估动态全局变量、锁中毒 panic 及 secret 生命周期。
- 重试、并发限制、长期 runtime 复用或传输层改造不应只改本文件；它们属于 `base::execute_json_embedding_call` 或 `EmbedFn` 的共享契约，会影响其他 provider。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/inference` 确认 `gemini.rs`、`gemini_test.rs`、`lib.rs`、`base.rs` 和 `embed_fn.rs` 均在索引中。
- RustCodeGraph `node --file pkg/inference/gemini.rs` 读取了全部 161 行；`query Gemini` 定位 `GeminiEmbedder`、`new`、`with_config`、`endpoint`、`create_embeddings`、`create_embeddings_with_context` 和 `decode_embeddings`。
- RustCodeGraph `query create_embeddings_with_context --json`、`query execute_json_embedding_call --json`、`query provider_auth_headers --json`、`query json_fields_with_options --json`、`query APIKeyProviderConfig --json` 和 `query ProviderContext --json` 确认了关键符号及所在文件。`callers`/`callees` 尝试未在 30 秒内返回结果，因此未把其当作调用边证据。
- RustCodeGraph 文件节点读取了 `pkg/inference/lib.rs`、`pkg/inference/embed_fn.rs`、`pkg/inference/base.rs`、`pkg/domain/domain.rs`、`pkg/inference/gemini_test.rs`、`pkg/inference/embedding/gemini/gemini.go` 和 `pkg/inference/embedding/gemini/gemini_test.go`。其中 Domain 注册、`run_batch` trait 调用、共享 HTTP 生命周期与独立测试共同构成调用链和边界证据。
- 直接读取 `pkg/inference/Cargo.toml` 核对 crate 名、`lib.rs` 入口、依赖和 `package.metadata.porting.go-package = "pkg/inference"`；该目录下没有 `doc.go`。
- 仓库引用搜索补足了 RustCodeGraph 未及时返回的边：`pkg/domain/domain.rs` 是生产代码中 `GeminiEmbedder::new` 的注册点，`pkg/inference/gemini_test.rs` 是 Rust 独立测试面，Go 生产接线在 `pkg/inference/sqlembed.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` + 11 个固定二级标题计数命令做结构验证。
