# `pkg/inference/cohere.rs`

## 文件定位

`pkg/inference/cohere.rs` 是 `astersql-inference` crate 中面向 Cohere Embed API 的 provider 适配器。模块由 [`pkg/inference/lib.rs`](lib.rs) 公开为 `cohere`，并通过同一文件再导出的 `Embedder` trait 接入通用嵌入运行时。应用侧在 [`pkg/domain/domain.rs`](../domain/domain.rs) 的 `Domain::init_inference_providers` 中以名称 `cohere` 注册 `CohereEmbedder`；因此模型名形如 `cohere/<model>` 时，真正的远端协议转换由本文件完成。

该文件不是批处理、缓存或 SQL 函数入口。批处理、缓存、请求合并和 provider 分派位于 [`pkg/inference/embed_fn.rs`](embed_fn.rs)，SQL 层再通过 Domain 持有的 `EmbedFn` 使用它。本模块只负责 Cohere 特有的配置提示、请求约束、HTTP 参数以及响应格式。

## 核心职责

- 用 `CohereEmbedder::new` 或 `CohereEmbedder::with_config` 构造一个持有共享 `reqwest::Client` 和 `APIKeyProviderConfig` 的 provider。
- 由 `endpoint` 将动态配置的完整 endpoint（不是仅 host/base path）规范化为 HTTP(S) URL；空配置退回 `DEFAULT_BASE_URL`。
- 实现 `Embedder`：对空输入、模型名和 `embedding_types` 做前置校验，动态读取 API key，合并固定字段与调用者选项，然后调用共享 HTTP 执行器。
- 用 `decode_embeddings` 同时兼容 Cohere 的普通数组响应和按类型分组的 `{ "float": ... }` 响应，并验证响应行数与输入文本数完全一致。
- 将鉴权失败、远端错误、取消、响应大小限制和敏感信息脱敏委托给 [`pkg/inference/base.rs`](base.rs) 的统一 provider 基础设施。

## 主要符号

- `DEFAULT_BASE_URL: &str`：默认完整端点 `https://api.cohere.com/v1/embed`。自定义 URL 为空或仅含空白时才使用它。
- `CohereEmbedder`：公开 provider 类型。`client` 为 crate 内可见，便于同 crate 测试/基础设施替换；`cfg` 私有，封装 API key、endpoint getter、自定义错误和最大响应大小。
- `CohereEmbedder::new(api_key, base_url) -> Self`：应用常用构造器。两个参数均为 `Send + Sync + 'static` 闭包，因此每次请求可以读取最新全局配置；同时安装面向 SQL 用户的缺 key和 401 提示。
- `CohereEmbedder::with_config(cfg) -> Self`：测试和高级接线入口。调用 `base::http_client("Cohere")` 创建带统一超时的客户端，再用 `APIKeyProviderConfig::with_defaults` 补齐默认响应大小。
- `CohereEmbedder::endpoint(&self, model) -> Result<Url, ProviderError>`：解析 endpoint。当前 `model` 参数仅为接口一致性而保留，不参与路径拼接；配置值必须已经是完整请求 URL。
- `Embedder::create_embeddings`：兼容只接收 `AtomicBool` 的 trait 入口，把取消标志包装为 `ProviderContext`，并把结构化 `ProviderError` 映射为字符串。
- `Embedder::create_embeddings_with_context`：主要执行入口，保留可检查的取消原因与底层错误来源。
- `decode_embeddings(body, expected)`：私有成功响应解码器；提取 `embeddings`，解析浮点行，并检查结果数量。

## 执行流程

1. `Domain::init_inference_providers` 构造 `CohereEmbedder::new`，API key getter 每次从 `tidb_exp_embed_cohere_api_key` 读取值，base URL getter 当前返回空字符串，并把实例注册为 `cohere`。
2. [`EmbedFn::request`](embed_fn.rs) 把 `provider/model` 拆成 provider 名和模型名，找到注册的 `Arc<dyn Embedder>`；同模型、同选项的请求可以在通用层合并，随后由 `run_batch` 调用 provider trait。
3. `create_embeddings` 将旧式取消标志转成 `ProviderContext`，转入 `create_embeddings_with_context`。直接使用上下文入口时，调用方原始取消 cause 可继续向上传播。
4. 主入口先处理本地条件：`texts` 为空立即返回空向量集合；非空输入要求 `model` 非空；如果提供 `embedding_types`，其 JSON 值必须严格等于 `["float"]`。这些检查发生在 API key getter 和网络请求之前。
5. `cfg.resolve_api_key` 动态取 key；`endpoint` 动态取并校验完整 URL。随后固定构造 `model` 与 `texts`，再经 `base::json_fields_with_options` 合并。该辅助函数先复制 `opts`、再写固定字段，所以调用者不能用同名选项覆盖真实模型或文本，而其他选项（如 `input_type`）会原样保留。
6. `base::provider_auth_headers(..., false)` 生成 `Authorization: Bearer <key>`；`base::execute_json_embedding_call` 发出 JSON POST，执行取消竞争、30 秒默认客户端超时、响应体上限、错误日志与密钥脱敏。只有 HTTP 200 进入成功解码；401 被映射为配置中的专用未授权错误，其他状态优先采用响应 `message`。
7. `decode_embeddings` 解析整个 JSON：`embeddings` 若为数组，直接作为行集合；若为对象，只接受非空的 `float` 字段；其他形态报错。`base::decode_float_rows` 将各数值转为有限 `f32`，最后要求行数等于 `texts.len()`。

## 数据与状态

`CohereEmbedder` 的长期状态只有可复用的 `reqwest::Client` 和不可变配置对象。配置中的 key/base URL 是 `Arc<dyn Fn() -> String + Send + Sync>`，保存的是 getter 而不是构造时快照；这使全局 API key 更新后新请求无需重建 provider。`with_defaults` 会把非正的 `max_response_bytes` 设为共享默认值 32 MiB。

每次调用的 `model`、`texts`、`Options`、JSON payload、header 和解码结果均为请求局部数据。`Options` 是有序的 `BTreeMap<String, serde_json::Value>`；固定的 `model`/`texts` 覆盖同名选项。响应最终为 `Vec<Vec<f32>>`，其中外层顺序必须与输入文本顺序一致。本文件不缓存向量；单文本缓存及配置版本失效策略属于 `EmbedFn`。

## 依赖与调用关系

上游链路是 `Domain::init_inference_providers` → `EmbedFn::register("cohere", ...)` → `EmbedFn::request/run_batch` → `CohereEmbedder` 的 `Embedder` 实现。`pkg/inference/lib.rs` 公开模块和 trait；Domain 将 key getter 绑定到全局系统变量。RustCodeGraph 还显示 `cohere.rs` 被 `embed_fn.rs` 及其独立测试引用。

本文件的直接下游包括：

- `base::http_client`：创建统一超时的 `reqwest::Client`。
- `APIKeyProviderConfig::{with_defaults, resolve_api_key, configured_base_url, unauthorized_error}`：处理动态配置和稳定错误语义。
- `base::parse_http_url`：拒绝相对 URL、非 HTTP(S) scheme、缺 host 及非法百分号转义。
- `base::json_fields_with_options`：合并调用选项与不可覆盖的请求字段。
- `base::provider_auth_headers`：构造 Bearer header，并在 header 非法时保留安全的 cause。
- `base::execute_json_embedding_call`：承担网络、取消、响应大小、状态码、日志和脱敏生命周期。
- `base::{string_field, decode_float_rows}`：读取错误 `message` 并把 JSON 数值矩阵转换为 `f32`。

crate 边界由 [`pkg/inference/Cargo.toml`](Cargo.toml) 定义，crate 名为 `astersql-inference`、库入口是 `lib.rs`；本文件直接使用 `reqwest` 和 `serde_json`，共享执行器还依赖 `tokio` 与 `logutil`。Cargo 中没有控制 Cohere 模块的 feature 条件。

## 错误处理与边界

- 空 `texts` 是成功的空结果，而且不会读取模型、key 或 endpoint；非空 `texts` 配空模型返回 `model name is required`。
- `embedding_types` 缺省时允许 Cohere 返回二维数组；一旦提供，必须在 JSON 语义上严格等于 `["float"]`。字符串、混合数组、多类型或其他单类型均在取 key 前失败。
- API key 为空时，`new` 返回包含 `SET @@GLOBAL.TIDB_EXP_EMBED_COHERE_API_KEY` 指引的定制错误；`with_config(Default::default())` 使用调用点提供的通用 fallback。
- endpoint 会先 trim；非法 URL 错误通过 `ProviderError` 保存安全展示文本，且测试确认查询参数中的 secret 不泄漏。
- 只把 200 当作成功。401 使用定制鉴权错误；其他状态从 JSON `message` 生成 `Cohere: status code ..., message: ...`，解析不到消息时退回 HTTP 状态文本。
- 成功体必须包含 `embeddings`。对象形态必须含非 null 的 `float`；行必须是数组、元素必须为可表示的有限 `f32`。响应行数不等于输入数时明确报错，防止批处理结果错位。
- `execute_json_embedding_call` 在读取超过上限时失败，并对 API key、Bearer token 及常见敏感字段脱敏。传输错误移除 URL 后再作为 cause 保存。
- `serde_json::to_value` 的两个 `expect` 建立在本地 `BTreeMap<String, Value>` 必可编码为对象这一不变量上；它们不是远端输入导致的常规错误路径。

## 并发与资源生命周期

`CohereEmbedder` 本身不创建线程、不持有锁，也不拥有关闭方法。它满足 `Embedder: Send + Sync`，共享的 `reqwest::Client` 可被 Domain 中的 `Arc<dyn Embedder>` 跨批次复用；配置 getter 也被类型约束为 `Send + Sync`，其内部同步责任由提供者承担（Domain 当前用 `RwLock` 读取全局变量）。

请求并发、100 ms 默认批窗口、每批上限 16、单文本缓存和 worker 回收由 `EmbedFn` 管理。provider 收到的是批次级 `ProviderContext`：共享执行器在发起请求前检查取消，并在网络 future 与每 5 ms 轮询的取消 future 之间竞争；若全部等待者取消或 Domain 关闭，`EmbedFn` 设置批次的 `AtomicBool`。结构化上下文还允许原始取消 cause 穿过 provider 边界。HTTP 响应体在内存中按 `max_response_bytes` 有界累积；请求结束后 payload、header、响应字节和向量由 Rust 所有权自动释放。

## 与 Go 版本的对应关系

直接 Go 对照为 [`pkg/inference/embedding/cohere/cohere.go`](embedding/cohere/cohere.go)，协议结构位于 [`pkg/inference/embedding/cohere/protocol.go`](embedding/cohere/protocol.go)。核心语义保持一致：默认完整 endpoint、空输入短路、模型必填、只接受 float 类型、固定字段不能被选项覆盖、Bearer 鉴权、数组/typed-float 两种响应、401 专用提示、远端 `message` 以及输入输出数量校验。

Rust 将 Go 的 `Embedder`、`APIKeyProviderConfig` 和共享 `ExecuteJSONEmbeddingCall` 分别映射到 `CohereEmbedder`、`base::APIKeyProviderConfig` 与 `base::execute_json_embedding_call`。Go 通过 `context.Context` 取消；Rust 同时保留 `AtomicBool` 兼容入口和可携带原始 cause 的 `ProviderContext`。Go 把响应协议拆为 `Response`/`ErrorResponse` 类型；Rust 直接用 `serde_json::Value` 读取字段。Go 的 `validateEmbeddingTypes` 接受运行时的 `[]string` 或 `[]any`，Rust 的调用边界本来就是 JSON 值，因此以与 `["float"]` 的 JSON 等值比较实现同一约束。

Rust `new` 接收 key/base URL 两个 getter 并内置 TiDB 系统变量提示，而 Go 的 `NewCohereEmbedder` 接收完整配置对象；Rust 的 `with_config` 对应后者的测试/高级用法。当前两端都把自定义 base URL 解释为完整 endpoint，模型不会附加到 URL。

## 扩展指南

- 新增 Cohere 请求选项通常无需改本文件：调用者可经 `Options` 传入，并应在 [`pkg/inference/cohere_test.rs`](cohere_test.rs) 增加请求 payload 断言。若选项影响响应形态，必须同步调整前置校验和 `decode_embeddings`。
- 若 Cohere 增加新的 embedding 类型，先明确数据库内部仍是否只消费 `f32`。扩展 `embedding_types` 时必须同时设计返回类型映射，不能仅放宽校验，否则 `decode_float_rows` 和 `Embedder` 的 `Vec<Vec<f32>>` 契约会不一致。
- 若 endpoint 规则变化，应修改 `endpoint`，并保持“完整 URL”与“base URL + path”语义清晰；同步覆盖默认值、空白、query、非法 scheme、相对路径和 secret 脱敏。当前 `model` 参数未参与 URL，改变这一点属于协议兼容变更。
- 若增加状态码映射，在 `execute_json_embedding_call` 的 `status_error` 闭包中添加，并与 Go 的 `StatusErrors` 保持一致；同时验证用户可操作提示及 cause/secret 行为。
- 若改请求字段，继续通过 `json_fields_with_options` 保证协议必需字段覆盖用户同名选项。需要允许用户覆盖时，应先评估批次键、缓存键和 Go 行为，而不是交换合并顺序。
- 测试应继续放在独立文件 `pkg/inference/cohere_test.rs`，不要嵌入生产文件；协议共性可复用其中的 `provider_contract`、`protocol_call` 和 `invalid_endpoints_and_missing_keys`。Go 语义变更还应同步检查 `pkg/inference/embedding/cohere/cohere_test.go`。
- 性能风险主要在响应体大小、批量文本数和 JSON/f32 转换；并发、缓存或批量策略应优先在 `embed_fn.rs` 扩展，避免在 provider 内重复实现。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件；`files --filter pkg/inference` 确认目标、模块入口、独立 Rust 测试及 Go 对照均在索引中。
- RustCodeGraph 源码与调用查询：`node --file pkg/inference/cohere.rs` 列出 13 个文件级符号；对 `create_embeddings_with_context` 的 callees 查询确认其调用 `endpoint`、`json_fields_with_options`、`provider_auth_headers`、`execute_json_embedding_call`，并引用 `decode_embeddings`；对解码器的查询确认下游为 `decode_float_rows`。
- 应用接线证据：`pkg/domain/domain.rs` 的 `Domain::init_inference_providers` 注册 `cohere` 并动态读取 `tidb_exp_embed_cohere_api_key`；`pkg/inference/embed_fn.rs` 的 `EmbedFn::request`、`run_batch` 证明 provider/model 分派、批处理和取消边界。
- crate 与基础设施证据：`pkg/inference/Cargo.toml`、`pkg/inference/lib.rs`、`pkg/inference/base.rs`。
- Rust 独立测试证据：`pkg/inference/cohere_test.rs` 覆盖 POST/Bearer/payload、typed float 与普通数组、固定字段优先级、非法类型、缺 key、URL 校验、401/404、响应长度、响应体上限、取消 cause、传输 cause 和 secret 脱敏。
- Go 对照证据：`pkg/inference/embedding/cohere/cohere.go`、`protocol.go`、`cohere_test.go`；其成功、选项、错误、endpoint、长度和通用 provider contract 与 Rust 测试意图对应。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时另执行任务指定的 11 章节结构检查，并人工复核只新增本说明文档且未修改源文件、Cargo 或总计划。
