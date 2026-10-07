# `pkg/inference/jina.rs`

## 文件定位

`jina.rs` 是 `astersql-inference` crate 中的 Jina AI 嵌入服务适配器。模块由 [`pkg/inference/lib.rs`](lib.rs) 公开为 `astersql_inference::jina`；应用启动时，`pkg/domain/domain.rs` 的 `Domain::init_inference_providers` 构造 `JinaEmbedder`，并以提供商名 `jina_ai` 注册到 Domain 持有的 `EmbedFn`。因此 SQL 侧使用 `jina_ai/<model>` 时，上层批处理器最终会通过 `Embedder` trait 进入本文件。

crate 边界由 [`pkg/inference/Cargo.toml`](Cargo.toml) 定义，crate 名为 `astersql-inference`，库入口是 `lib.rs`。本文件直接使用 `reqwest`、`serde_json` 和标准库同步原语；HTTP 生命周期、错误脱敏和响应大小限制由同 crate 的 `base.rs` 统一实现，Base64 向量解码复用 `openai.rs` 的公共内部函数。

## 核心职责

- `JinaEmbedder` 保存可复用的 HTTP 客户端和 `APIKeyProviderConfig`，在每次调用时动态读取 API key/base URL，而不是在构造时固化配置。
- `new` 设置面向 SQL 全局变量的缺失密钥与 401 提示；`with_config` 提供测试和内部接线所需的通用构造入口。
- `endpoint` 选择用户配置的完整 embeddings URL，空配置回退到 `DEFAULT_BASE_URL`，并要求绝对 HTTP(S) URL。
- `Embedder` 实现校验输入，合并固定请求字段与调用者选项，发送 JSON POST，并把 Jina 的成功/失败响应转换为统一结果或 `ProviderError`。
- `decode_embeddings` 将 Jina 的索引化 Base64 响应委托给 `openai::decode_indexed_base64_embeddings`，从而恢复输入顺序并统一检查数量、索引和字节布局。

本文件不是批处理、缓存或 SQL 表达式入口；这些职责分别位于 `embed_fn.rs`、Domain/表达式层。本文件也不定义独立协议结构体，而是使用 `serde_json::Value` 与共享解码器表达协议。

## 主要符号

- `DEFAULT_BASE_URL: &str`：默认完整端点 `https://api.jina.ai/v1/embeddings`。配置值不是“基础域名”，不会自动追加路径。
- `pub struct JinaEmbedder`：公开提供商类型。
  - `client: reqwest::Client` 为 `pub(crate)`，便于 crate 内测试/接线替换；由 `base::http_client("JinaAI")` 创建并应用共享超时。
  - `cfg: APIKeyProviderConfig` 为私有配置，包含动态 getter、定制错误和最大响应体大小。
- `JinaEmbedder::new(api_key, base_url) -> Self`：公共便捷构造器。两个闭包均须 `Send + Sync + 'static`；它写入 Jina 专用的 SQL 配置提示，然后委托 `with_config`。
- `JinaEmbedder::with_config(cfg) -> Self`：应用 `APIKeyProviderConfig::with_defaults`，使非正的响应上限回退到共享默认值，并创建 HTTP 客户端。
- `JinaEmbedder::endpoint(&self, model) -> Result<reqwest::Url, ProviderError>`：读取并去除 base URL 首尾空白，选择默认值后调用 `base::parse_http_url`。当前 `model` 参数不参与 URL 计算，保留它只是与其他 provider 的端点接口形状一致。
- `Embedder::create_embeddings`：兼容仅提供 `AtomicBool` 的旧边界；构造 `ProviderContext`，调用上下文版本，再将结构化错误降为字符串。
- `Embedder::create_embeddings_with_context`：核心入口，保留取消原因与错误 cause，执行校验、鉴权、请求和解码。
- `decode_embeddings(body, expected)`：私有成功响应解码适配器，直接调用 `openai::decode_indexed_base64_embeddings`。

文件没有条件编译项，也没有自建 trait、枚举或后台任务。

## 执行流程

1. `Domain::init_inference_providers` 创建 `JinaEmbedder::new`；API key getter 每次从全局变量 `tidb_exp_embed_jina_ai_api_key` 读取值，base URL getter 当前为 `String::new`，所以运行时走默认端点。实例以 `jina_ai` 注册到 `EmbedFn`。
2. `EmbedFn::request` 将 `provider/model` 拆分为提供商与模型；批次触发时，`run_batch` 通过 trait 调用 provider。`JinaEmbedder::create_embeddings` 再进入 `create_embeddings_with_context`。
3. 空 `texts` 立即返回空向量列表，不读取密钥、不解析端点，也不发网络请求。非空请求要求 `model` 非空，并拒绝布尔值为 `true` 的 `return_multivector`。
4. `cfg.resolve_api_key` 动态取得密钥；空密钥返回构造器配置的提示。`endpoint` 动态取得 URL，空白值使用 `DEFAULT_BASE_URL`，非法或非 HTTP(S) 绝对 URL在网络 I/O 前失败。
5. 固定字段为 `model`、`input`、`embedding_type: "base64"`。`base::json_fields_with_options` 先复制 `opts` 再写固定字段，因此调用者可以增加 `task` 等选项，但不能覆盖三个固定字段。
6. `base::provider_auth_headers(..., false)` 生成 `Authorization: Bearer <key>`；随后 `base::execute_json_embedding_call` 发送 JSON POST，执行取消竞争、响应体上限和错误脱敏。
7. HTTP 200 时，`decode_embeddings` 按 `data[*].index` 恢复输入顺序，解码 little-endian `f32` Base64 数据，并要求响应数量等于输入数量。非 200 时优先从 JSON 的 `detail` 字段取消息；401 映射为专用鉴权错误，其他状态形成 `JinaAI: status code ..., message: ...`。

## 数据与状态

`JinaEmbedder` 的长期状态只有 `reqwest::Client` 和 `APIKeyProviderConfig`。配置中的 API key/base URL 是 `Arc<dyn Fn() -> String + Send + Sync>`，所以全局配置变更可在后续请求中生效；本文件不缓存密钥、端点或向量。

单次请求的数据是 `Options`（即 `BTreeMap<String, serde_json::Value>`）以及序列化后的 JSON `Value`。固定字段覆盖同名 option 是重要不变量：外部调用不能把请求模型、输入或编码类型改成与解码路径不一致的值。成功结果是与 `texts` 等长的 `Vec<Vec<f32>>`，响应 `index` 决定每个向量回到哪个输入位置。

密钥仅用于请求头和共享执行器的 `secrets` 脱敏列表，不进入持久状态或返回结果。响应体受 `cfg.max_response_bytes` 限制，避免无界读取。

## 依赖与调用关系

上游关系：

- `pkg/inference/lib.rs`：公开 `jina` 模块和 `Embedder`/`Options`。
- `pkg/domain/domain.rs::Domain::init_inference_providers`：构造并以 `jina_ai` 注册实例；该初始化由 `Domain::start` 调用。
- `pkg/inference/embed_fn.rs::run_batch`：经 `Arc<dyn Embedder>` 调用 provider，并在本文件外负责批量合并、等待者取消、数量复核和缓存分发。
- `pkg/inference/jina_test.rs`：直接覆盖构造器、trait 调用、端点和协议边界。

下游关系：

- `base::http_client`、`APIKeyProviderConfig`、`ProviderContext`：提供客户端、动态配置、取消原因及结构化错误。
- `base::parse_http_url`：拒绝相对 URL、非 HTTP(S) scheme、无 host 和 Go `url.Parse` 同样不接受的坏百分号转义。
- `base::json_fields_with_options`、`base::provider_auth_headers`：实现固定字段优先合并及 Bearer 鉴权。
- `base::execute_json_embedding_call`：管理同步入口中的 Tokio current-thread runtime、HTTP POST、取消、响应上限、日志与秘密脱敏、状态错误和解码回调。
- `openai::decode_indexed_base64_embeddings`：验证 JSON 对象/数组、响应长度、索引范围与唯一性，并解码浮点数组。

RustCodeGraph 对 `execute_json_embedding_call` 的调用边明确指回 `jina.rs::create_embeddings_with_context`，对共享解码器的调用边明确指回 `jina.rs::decode_embeddings`；Domain 注册关系则由 `init_inference_providers` 源码确认。

## 错误处理与边界

- 空文本是成功的空结果；空模型是 `model name is required`。
- 只有 `return_multivector` 恰为 JSON 布尔 `true` 时被拒绝，避免多向量响应进入仅支持单个 dense embedding 的解码器；其他类型或 `false` 不触发该分支。
- 缺失密钥在 `new` 路径返回包含 `TIDB_EXP_EMBED_JINA_AI_API_KEY` 配置方法的错误；通用 `with_config` 可注入不同错误。
- URL 会 trim，且必须是带 host 的绝对 HTTP(S) URL。当前实现不根据 `model` 拼路径，也不追加 `/v1/embeddings`。
- 认证头无法编码、网络失败、响应读取失败和 runtime 创建失败由共享层包装为安全错误并保留底层 cause；取消时优先返回调用者提供的原因。
- 响应超过配置上限立即失败。非成功响应中的 `detail` 会进行 API key 脱敏；无法解析消息时使用 HTTP 状态文本回退。
- 401 始终优先采用 `cfg.unauthorized_error`。其他状态携带解析后的 `detail`。
- 成功响应必须包含与输入等量的 `data` 项；乱序允许并按 `index` 归位，但重复索引、越界索引、非法 Base64、非 `f32` 字节长度、缺失/null/空 dense embedding 都失败。

`pkg/inference/jina_test.rs` 覆盖上述主要分支，并通过共享 `provider_contract` 复核响应大小限制、传输错误 cause、取消和脱敏。最终批处理层还会再次检查 provider 返回数量，并限制单向量最多 16,383 维；那是 `embed_fn.rs` 的上层契约，不是本文件独自实施的校验。

## 并发与资源生命周期

`JinaEmbedder` 满足 `Embedder: Send + Sync`：`reqwest::Client` 可跨调用复用，配置 getter 也被约束为 `Send + Sync`。本文件没有可变集合、锁、线程或任务所有权，因此多个批次可以共享同一实例；配置闭包内部如何同步由其提供者负责，Domain 的 getter 通过 `RwLock` 读取全局变量。

每次核心调用创建临时 payload 和 header。共享执行器为一次调用创建 current-thread Tokio runtime，HTTP future 与每 5ms 检查一次的取消 future 竞争；请求完成、报错或取消后，runtime、响应体和临时密钥字符串随调用释放。取消标志的批次生命周期、工作线程和 join 由 `EmbedFn` 管理。

本文件没有显式 `Drop`、连接关闭或重试逻辑。HTTP 连接池生命周期绑定到 `JinaEmbedder.client`；实例随 Domain 持有的 `EmbedFn` 一起释放。

## 与 Go 版本的对应关系

直接对照为 `pkg/inference/embedding/jina/jina.go`、`protocol.go` 和 `jina_test.go`。Rust 保留了 Go 的关键语义：默认完整端点、空文本短路、空模型错误、拒绝 `return_multivector=true`、固定字段覆盖 options、Bearer 鉴权、401 专用提示、`detail` 错误消息、响应大小限制，以及按 index 解码 Base64 `f32` 向量。

结构差异如下：

- Go 用 `Request`、`Response`、`ErrorResponse` 结构体；Rust 用动态 JSON 构造请求，并复用 OpenAI 的共享索引解码器。两者仍要求相同的 wire 字段。
- Go 的 `NewJinaEmbedder` 接收完整 `APIKeyProviderConfig`；Rust 的 `new` 接收两个 getter 并预设 SQL 友好错误，同时保留 `with_config` 作为等价的通用入口。
- Go 直接接收 `context.Context`；Rust 通过 `ProviderContext` 同时支持遗留 `AtomicBool` 和可保留 cause 的取消路径。
- Go 把 `decodeErrorMessage`、`validateOptions` 和 `embeddingsEndpoint` 分成独立函数；Rust 将前两项内联到核心流程，保留 `endpoint` 方法，并把成功解码压缩成委托函数。
- Rust Domain 当前给 `base_url` 传入空 getter，因此生产注册默认使用 Jina 官方 URL；Go 配置对象仍允许调用方供应完整 URL。Rust 的 `with_config` 同样保留自定义 URL 能力，测试已覆盖。

## 扩展指南

- 新增 Jina 请求选项时，通常只需由上游放入 `Options`；若选项会改变响应形状，必须先在 `create_embeddings_with_context` 增加显式校验或新解码路径，不能让它落入现有单 dense-vector 解码器。
- 若 Jina 端点规则改为按模型拼接，修改 `endpoint`，并同步 `jina_endpoints_validate_configuration_and_keep_default_protocol_routes`；必须继续验证完整自定义 URL、trim、非法 scheme/相对 URL和默认路径。
- 若固定协议字段变化，修改构造 `fields` 的位置，并在 `jina_protocol_restores_indices_and_rejects_missing_dense_embeddings` 中断言最终 payload；保持固定字段覆盖同名 options，除非 Go 行为也明确改变。
- 若响应协议与 OpenAI 索引格式分叉，应在本文件实现专用解码函数，而不是弱化共享解码器；独立测试应放在 `pkg/inference/jina_test.rs`，不要嵌入生产文件。
- 若新增状态码映射或错误字段，修改 `execute_json_embedding_call` 的 `decode_message`/`status_error` 回调，并同步 401、普通非 2xx、脱敏及 cause 保留测试。
- 若生产需要可配置 base URL，接线应落在 `Domain::init_inference_providers` 的 getter 和对应全局变量定义；本文件已支持动态 getter，不应另建缓存。

兼容风险主要是 provider/model 命名、请求 JSON 优先级和错误文本被 SQL 用户依赖；性能风险主要来自额外序列化、响应体大小及共享执行器每次创建 runtime。任何扩展都应同时检查批处理层的等长结果契约、取消语义和 16,383 维上限。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点；目标 `pkg/inference/jina.rs` 共 117 行、13 个索引符号。
- 读取的 Rust 生产路径：`pkg/inference/jina.rs`、`pkg/inference/lib.rs`、`pkg/inference/embed_fn.rs`、`pkg/inference/base.rs`、`pkg/inference/openai.rs`、`pkg/domain/domain.rs::init_inference_providers`。
- 读取的 crate 配置：`pkg/inference/Cargo.toml`。
- 读取的独立 Rust 测试：`pkg/inference/jina_test.rs`。关键用例为 `go_merge_43_jina_provider_posts_base64_and_rejects_multivector`、`go_merge_43_jina_provider_reports_error_detail`、`jina_shared_contract_preserves_limits_validation_and_causes`、`jina_protocol_restores_indices_and_rejects_missing_dense_embeddings`、`jina_endpoints_validate_configuration_and_keep_default_protocol_routes`。
- 读取的 Go 对照：`pkg/inference/embedding/jina/jina.go`、`protocol.go`、`jina_test.go`；对照覆盖请求、协议、索引校验、鉴权、端点和共享 provider contract。
- RustCodeGraph 关键边：`Domain::start -> Domain::init_inference_providers -> JinaEmbedder::new`（注册名 `jina_ai`）；`EmbedFn::run_batch -> Embedder` provider 调用；`JinaEmbedder::create_embeddings_with_context -> base::execute_json_embedding_call`；`jina.rs::decode_embeddings -> openai::decode_indexed_base64_embeddings`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证本文恰有 11 个固定二级章节，并人工检查没有把上层批处理能力误写成本文件内部实现。
