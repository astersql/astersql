# `pkg/inference/gemini.rs`

## 文件定位

在 `Embedder::create_embeddings_with_context` 中为每条文本构造 Gemini `requests` 元素，并调用共享 HTTP 执行层。

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

## 扩展指南

- 新增 Gemini 请求选项时，优先保持 `Options` 透传，并确认 `json_fields_with_options` 的“必需字段最后写入”不变；在 `pkg/inference/gemini_test.rs` 增加独立回归，必要时同步 Go 对照测试。不应把 Rust 单元测试内嵌到本生产文件。
- 修改 endpoint 时聚焦 `GeminiEmbedder::endpoint`，必须同时验证 base URL 的 query/尾斜杠、模型名中的空格、`/`、`?`、`.`/`..` 和 raw URL 发送路径；错误的转义可能导致路由注入或模型不可达。
- 修改响应协议时聚焦 `decode_embeddings` 和错误解码闭包，保留输入/输出数量不变量、`f32` 溢出检查、错误体脱敏和 401 自定义错误。若 Gemini API 增加部分成功语义，需先明确 `EmbedFn` 对“每个输入必有一行”的依赖。
- 修改 key/base URL 来源时需同步 `GeminiEmbedder::new` 与 `pkg/domain/domain.rs` 的注册闭包，并评估动态全局变量、锁中毒 panic 及 secret 生命周期。
- 重试、并发限制、长期 runtime 复用或传输层改造不应只改本文件；它们属于 `base::execute_json_embedding_call` 或 `EmbedFn` 的共享契约，会影响其他 provider。
