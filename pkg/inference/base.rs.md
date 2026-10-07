# `pkg/inference/base.rs`

## 文件定位

[`base.rs`](base.rs) 是 `astersql-inference` crate 的供应商适配公共层。crate 由 [`lib.rs`](lib.rs) 公开 `base` 模块，并在 [`Cargo.toml`](Cargo.toml) 中声明 `reqwest`、`tokio`、`regex`、`serde_json` 和 `logutil` 等直接依赖。它不负责 SQL 表达式、批处理或缓存；这些职责位于 [`embed_fn.rs`](embed_fn.rs)。本文件把 Cohere、Gemini、HuggingFace、JinaAI、NVIDIA NIM、OpenAI 和 TiDB Cloud 适配器重复需要的配置、URL、鉴权、请求、响应边界与安全错误处理集中起来。

公开 API 主要是字节解码、选项合并、错误脱敏、响应限长、URL 校验，以及 `ProviderError`、`ProviderContext`、`APIKeyProviderConfig`。HTTP 执行器和 JSON 解码辅助函数为 `pub(crate)`，只供当前 crate 内的具体供应商模块使用（`base.rs` 中相应可见性声明）。

## 核心职责

- 建立供应商无关的安全边界：`ProviderError` 将可展示消息与可检查的底层 cause 分离；`sanitize_error_text` 在记录远端错误前清除显式 secret、敏感 JSON 字段、Bearer token 和 OpenAI 风格 key。
- 统一 API-key 供应商配置：`APIKeyProviderConfig::with_defaults`、`resolve_api_key_error`、`configured_base_url`、`unauthorized_error` 处理动态 getter、自定义错误和 32 MiB 默认响应上限。
- 统一 JSON embedding HTTP 生命周期：`execute_json_embedding_call` 负责取消检查、同步到异步运行时桥接、POST、限长读取、严格的 HTTP 200 成功判断、错误体解析与脱敏日志，再把成功体交给供应商 decoder。
- 保持 Go 兼容的协议细节：`decode_float32_array_bytes` 使用小端 `f32`；`escape_url_path_segment` 特判完整的 `.`/`..` 段；`go_http_status_text` 修正 Go 与当前 HTTP reason phrase 的历史差异。
- 提供弱 schema 的 JSON 辅助：`string_field`、`decode_float_row(s)`、`ensure_json_object` 明确 null、类型错误和 float32 溢出的处理。

## 主要符号

- `decode_float32_array_bytes(&[u8]) -> Result<Vec<f32>, String>`：拒绝空输入和非 4 字节倍数，按 little-endian 逐元素恢复 `f32`。OpenAI base64 响应解码在 [`openai.rs`](openai.rs) 中调用它。
- `json_fields_with_options(fields, opts) -> Options`：先克隆用户选项，再 `extend(fields)`，因此协议固定字段覆盖同名用户选项，同时不修改原 `opts`。
- `sanitize_error_text(text, secrets)`：三个 `LazyLock<Regex>` 只初始化一次；先替换调用者提供的精确 secret，再处理通用凭据模式，最后按 UTF-8 字符边界截到 4096 字节并追加标记。
- `ProviderError`：`Display` 只输出安全 `message`，`Error::source` 暴露保存的 `Arc<dyn Error + Send + Sync>`；`redacted`、`has_cause` 与 `From<String/&str>` 分别用于保留 cause、按指针核验 cause 和构造纯消息错误。
- `ProviderContext<'a>`：借用取消用 `AtomicBool`，并可借用返回 `ProviderError` 的 cause getter；`cause()` 优先返回调用者 cause，否则以 Acquire 读取取消位并生成 `request canceled`。
- `APIKeyProviderConfig`：持有线程安全的 API key/base URL getter、自定义缺 key/未授权错误和响应上限；派生 `Clone, Default`，便于各适配器保存规范化配置。
- `read_response_body`：同步 `Read` 版本的“上限加一字节”读取器，负上限报错，底层 I/O 错误保存为 `ProviderError` cause。
- `parse_http_url`、`escape_url_path_segment`、`ProviderEndpoint`：校验绝对 HTTP(S) URL，生成 Go `url.PathEscape` 兼容的单段转义，并在 WHATWG URL 规范化改变转义路径时保留原始请求目标。
- `http_client`：构建带 `DEFAULT_HTTP_TIMEOUT`（30 秒）的 `reqwest::Client`；构造失败被视为初始化期不可恢复错误并 panic。
- `execute_json_embedding_call`：公共请求状态机；两个 decoder 通过 `Option<Fn>` 注入并在发请求前强制存在，`status_error` 可覆盖供应商特殊状态错误。
- `string_field`、`auth_headers`、`decode_float_row`、`decode_float_rows`、`ensure_json_object`、`provider_auth_headers`、`go_http_status_text`：crate 内部的 wire-format、鉴权和兼容辅助函数。
- 常量 `DEFAULT_MAX_RESPONSE_BYTES`（32 MiB）与 `DEFAULT_HTTP_TIMEOUT`（30 秒）限定默认资源开销；文件没有条件编译项、模块级可变状态或自定义 trait。

## 执行流程

典型调用从 [`embed_fn.rs`](embed_fn.rs) 的 `Embedder::create_embeddings_with_context` 边界进入具体供应商的同名实现。RustCodeGraph 显示七个生产调用者：[`cohere.rs`](cohere.rs)、[`gemini.rs`](gemini.rs)、[`huggingface.rs`](huggingface.rs)、[`jina.rs`](jina.rs)、[`nvidia.rs`](nvidia.rs)、[`openai.rs`](openai.rs) 与 [`tidbcloud.rs`](tidbcloud.rs)。各实现大体按以下顺序运行：

1. 用 `APIKeyProviderConfig` 取得 key、配置 URL 与响应上限；用 `parse_http_url`、`escape_url_path_segment` 和（必要时）`ProviderEndpoint::with_path` 建立端点。
2. 用 `json_fields_with_options` 合并用户 options 与不可覆盖的协议字段；具体模块再通过 `serde_json::to_value` 形成 payload。
3. `provider_auth_headers` 生成 `Authorization: Bearer ...`，Gemini 则使用 `x-goog-api-key`；header 非法时将底层错误包装成不泄露凭据的供应商请求错误，并优先保留取消 cause。
4. `execute_json_embedding_call` 首先验证错误与成功 decoder 均已配置，再在任何网络操作前检查 `ProviderContext::cause()`。
5. 函数建立 current-thread Tokio runtime。普通 URL 走配置好的 `reqwest::Client`；若 `ProviderEndpoint::raw_url()` 表明 WHATWG 规范化会损失原始 escaped path，则转交 [`raw_http.rs`](raw_http.rs) 的 `post_json`，以保留代理 absolute target 和 dot segment。
6. 请求与一个每 5 ms 检查取消 cause 的 future 一起轮询；取消先 ready 时请求 future 被丢弃。响应分块读取，最多保留 `max_bytes + 1`，超限立即失败。
7. 状态恰为 200 时调用成功 decoder，并传入 `expected` 以便具体协议验证返回向量数。其他 2xx 状态仍走失败分支。
8. 非 200 响应先解析 JSON 错误消息；消息和解析错误均脱敏后写入后台错误日志。随后优先应用 `status_error` 覆盖，否则用脱敏消息或 `go_http_status_text` 生成统一错误。

## 数据与状态

本文件没有业务级持久状态。三个正则表达式通过 `LazyLock` 进程内惰性初始化且只读；`reqwest::Client` 存放在各供应商实例中，而不是全局单例。`APIKeyProviderConfig` 的 getter 使用 `Arc<dyn Fn() -> String + Send + Sync>`，因此每次请求都可读取最新配置；`with_defaults` 返回修改后的副本，不回写原配置。

`Options` 是 [`embed_fn.rs`](embed_fn.rs) 定义的 `BTreeMap<String, serde_json::Value>`，选项合并具有稳定键序且固定字段胜出。`ProviderEndpoint` 同时保存 `reqwest::Url` 与可选 `escaped_path`；只有规范化后的 `url.path()` 与原转义路径不同，`raw_url()` 才重建不含 fragment、保留 query 的原始请求地址。

响应体始终整体收集为 `Vec<u8>`，所以 `max_response_bytes` 是关键内存上界。`decode_float_row` 把 JSON null 行解释为空向量、行内 null 解释为 `0.0`；JSON number 转为 `f32` 后若不是有限值则报告溢出。`decode_float_rows` 仅逐行组合，不在公共层检查向量维度或返回数量，这些校验属于供应商 decoder。

## 依赖与调用关系

上游主链是 `EmbedFn`/`Embedder` → 具体供应商 `create_embeddings_with_context` → `base.rs` 公共工具。RustCodeGraph 的精确查询确认 `execute_json_embedding_call` 有七个生产供应商调用者，`provider_auth_headers` 被同一组供应商使用；`rg` 进一步核对了每个调用位置。`openai.rs` 还直接调用 `decode_float32_array_bytes`，其余常规 JSON 数组供应商使用 `decode_float_row(s)`。

下游依赖包括：标准库的 `Read`、原子与 `Arc`；`regex` 做脱敏；`serde_json` 处理通用 JSON 值；`reqwest` 提供 URL、header、client 与普通请求；`tokio` 提供 current-thread runtime、计时和异步轮询；`logutil::log::BgLogger` 记录非 200 响应。转义路径例外分支调用 [`raw_http.rs`](raw_http.rs)，后者再使用 `hyper`、`hyper-util`、`hyper-rustls`、`rustls`、`http-body-util` 和 `tower-service`；这些依赖均由 [`Cargo.toml`](Cargo.toml) 声明。

`read_response_body` 当前主要由独立 Rust 测试直接覆盖；异步生产路径在 `execute_json_embedding_call` 与 `raw_http::post_json` 中实现同样的上限加一策略，而不是直接调用该同步函数。

## 错误处理与边界

- 空 embedding bytes 与非四字节倍数分别返回固定错误；合法字节不额外拒绝 NaN/Infinity，因为该函数按位恢复协议数据。
- URL 会先拒绝畸形 `%` 转义，再要求显式 scheme、仅允许 HTTP/HTTPS 且必须有 host。错误展示使用固定 `description`，解析器原错误只作为 cause 保存，避免输出含 secret 的原 URL。
- header value 解析失败不会回显 key。`provider_auth_headers` 若此时已取消，返回取消 cause；否则统一显示 `<provider> request failed`。
- 负响应上限在同步和两个异步读取分支中均被拒绝；恰好等于上限合法，超过一字节即可判定失败。`saturating_add(1)` 避免最大整数溢出。
- 请求错误调用 `reqwest::Error::without_url()` 后再保存为 cause；远端错误文本只有脱敏结果能够进入日志和最终普通状态错误。
- `execute_json_embedding_call` 只把 200 视为成功。错误 decoder 失败时记录脱敏的 `parse_error`，最终退回状态文本；供应商特殊状态覆盖在日志之后执行。
- `ProviderError::has_cause` 检查的是 `Arc` 指针身份而非结构相等；这用于验证自定义错误/cause 未在包装中丢失。
- `http_client` 的唯一 panic 点是 client builder 失败；正则构造也以静态常量模式 `unwrap`，属于开发期不变量，不处理运行时外部输入。

## 并发与资源生命周期

`ProviderContext` 不拥有取消标志或 cause 回调，只在一次调用期间借用它们；Acquire load 与上游对原子取消位的发布形成同步边界。`APIKeyProviderConfig` 的闭包以及 `ProviderError` 的 cause 均要求 `Send + Sync`，可随供应商对象跨线程共享。

每次 `execute_json_embedding_call` 都新建一个 current-thread Tokio runtime，并在同步函数返回时销毁；HTTP future 和取消 future 在同一 runtime 内被手工轮询。取消检查周期是 5 ms；取消获胜后请求 future 随作用域结束被 drop。普通 `reqwest::Response` 由所有权离开作用域释放，分块读取不持有无界缓存；特殊 escaped-path 分支的连接和超时生命周期由 [`raw_http.rs`](raw_http.rs) 管理。

本文件不创建工作线程、锁、通道、事务或长期任务。批处理线程、waiter 计数、缓存和 Domain 关闭语义在 [`embed_fn.rs`](embed_fn.rs)，不应误归因于本文件。

## 与 Go 版本的对应关系

直接 Go 对照是 [`embedding/base/base.go`](embedding/base/base.go)，测试为 [`embedding/base/base_test.go`](embedding/base/base_test.go)。Rust 保留了 Go 的 30 秒默认 HTTP timeout、32 MiB 默认体限、限长加一读取、API-key 配置优先级、固定字段覆盖 options、小端 float32 解码、凭据脱敏、严格 200 成功、供应商状态覆盖和通用错误格式。

主要形态差异如下：

- Go 用 `context.Context`/`context.Cause`；Rust 用借用 `AtomicBool` 加可选 cause getter 的 `ProviderContext`，并以轮询 future 将同步 trait 边界桥接到异步 HTTP。
- Go 将调用参数聚合在 `JSONEmbeddingCall` 结构体中；Rust 的 `execute_json_embedding_call` 直接接收参数和闭包，仍保持“公共生命周期、供应商 schema decoder”分工。
- Go 的 `NewJSONRequest`、`DoRequest`、`PostJSON` 是分层公开辅助；Rust 将常规请求内联于公共执行器，并为 WHATWG dot-segment 规范化另设 `ProviderEndpoint`/`raw_http.rs` 路径。
- Go 的 `IndexedBase64Embedding` 与 `DecodeIndexedBase64Embeddings` 位于公共 base；Rust 的索引、去重、数量检查在 [`openai.rs`](openai.rs) 的供应商响应解码中完成，公共层只保留原始 float bytes 解码器。
- Go 的 `SetEscapedURLPath` 显式维护 `url.URL.RawPath`；Rust 用 `ProviderEndpoint::with_path/raw_url` 保存原 escaped path，并在必要时改走 raw HTTP 实现。
- Rust `sanitize_error_text` 在 4096 字节处回退到 UTF-8 字符边界；Go 直接按 byte slice 截断。这是为保证 Rust `String` 有效 UTF-8 所需的实现差异。
- Rust 的 `decode_float_row(s)`/`ensure_json_object` 是多个 Rust 供应商共享的弱 schema 工具，在当前 Go base 文件中没有同名公共函数。

## 扩展指南

新增常规 JSON embedding 供应商时，应在独立供应商文件实现 `Embedder`，复用 `APIKeyProviderConfig`、`parse_http_url`、`json_fields_with_options`、`provider_auth_headers` 和 `execute_json_embedding_call`，只把错误 schema、特殊状态与成功 schema 留给闭包。不要在公共执行器中加入单一供应商字段或响应结构。

若扩展 URL 行为，必须同时验证普通 reqwest 路径与 escaped dot-segment 的 [`raw_http.rs`](raw_http.rs) 路径，尤其是 query、HTTP proxy absolute target、HTTPS CONNECT 与代理鉴权。若改变取消语义，应同步检查 `ProviderContext::cause`、`Embedder::create_embeddings_with_context` 和批处理层“最后 waiter 才取消 provider”的契约。

若新增敏感字段或 key 格式，应修改 `sanitize_error_text` 并在独立 [`base_test.rs`](base_test.rs) 增加泄漏回归；不可把测试嵌入生产源文件。响应解析扩展应保持 null/type/overflow 的显式规则，并在相应供应商 `*_test.rs` 覆盖数量、顺序、维度及错误 cause。改变默认 timeout/body limit、状态文本或错误格式会影响 Go/Rust 兼容与用户可观察行为；改变每次调用创建 runtime 的策略则需单独评估吞吐、连接复用和取消时延。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含 11,467 个文件、索引时间戳为 `1791342965170`；`files --filter pkg/inference` 确认目标及相关模块已索引；`node --file pkg/inference/base.rs` 阅读完整 557 行和 47 个符号；`explore "ProviderContext create_embeddings_with_context execute_json_embedding_call pkg/inference"` 确认七个供应商到公共执行器、鉴权辅助的调用边。单独的宽泛 `explore` 产生同名误配，精确 `callers` 对部分自由函数为空，因此调用者集合又以 `rg` 精确复核，未把误配结果用于结论。
- Rust 源码：[`base.rs`](base.rs)、[`lib.rs`](lib.rs)、[`embed_fn.rs`](embed_fn.rs)、[`raw_http.rs`](raw_http.rs)，以及七个供应商实现 [`cohere.rs`](cohere.rs)、[`gemini.rs`](gemini.rs)、[`huggingface.rs`](huggingface.rs)、[`jina.rs`](jina.rs)、[`nvidia.rs`](nvidia.rs)、[`openai.rs`](openai.rs)、[`tidbcloud.rs`](tidbcloud.rs)。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的 `[lib]`、依赖列表和 `package.metadata.porting.go-package = "pkg/inference"`。
- 独立 Rust 测试：[`base_test.rs`](base_test.rs) 覆盖 little-endian 解码、options 覆盖、脱敏与 UTF-8 安全截断、body 上限/I/O cause、配置错误身份、URL/转义、代理/超时、decoder 缺失、成功 decoder cause 及 Go 状态文本兼容；供应商 `*_test.rs` 覆盖这些公共入口在具体协议中的组合。
- Go 对照：[`embedding/base/base.go`](embedding/base/base.go) 与 [`embedding/base/base_test.go`](embedding/base/base_test.go)，用于核对公共职责、边界条件、错误优先级和协议兼容差异。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前另以任务给定命令验证本文恰有 11 个固定二级章节，并以 `git diff --check` 检查文档差异格式。
