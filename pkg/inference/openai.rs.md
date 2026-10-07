# `pkg/inference/openai.rs`

## 文件定位

`pkg/inference/openai.rs` 是 `astersql-inference` crate 中的 OpenAI-compatible embedding provider。模块由 [`pkg/inference/lib.rs`](lib.rs) 公开为 `openai`，核心类型 `OpenAIEmbedder` 实现同 crate 的 `Embedder` trait。应用侧在 [`pkg/domain/domain.rs`](../domain/domain.rs) 的 `Domain::init_inference_providers` 中以名称 `openai` 注册该实现，并把 API key 与 API base 的 getter 分别绑定到全局系统变量 `tidb_exp_embed_openai_api_key` 和 `tidb_exp_embed_openai_api_base`。

本文件处于通用嵌入运行时与 OpenAI HTTP 协议之间：批处理、缓存、provider/model 分派位于 [`pkg/inference/embed_fn.rs`](embed_fn.rs)，HTTP 生命周期、取消、响应上限、日志和脱敏位于 [`pkg/inference/base.rs`](base.rs)；本文件只补齐 OpenAI 特有的配置提示、endpoint 规则、请求字段、401 映射和 indexed-base64 响应解码。其解码器还被 Jina、NVIDIA 和 TiDB Cloud provider 复用，因此它同时承担 crate 内的 OpenAI-compatible wire-format 基础能力。

## 核心职责

- 通过 `OpenAIEmbedder::new`、`with_config` 或 `with_provider_config` 构造可复用 provider，并保留每请求动态读取 key/base URL 的 getter。
- 用 `endpoint` 将空配置映射到 `https://api.openai.com/v1/embeddings`，并将自定义 HTTP(S) base 规范化为恰好带一个 `/embeddings` 后缀，同时保留 query 和 fragment。
- 实现 `Embedder` 的字符串错误兼容入口与结构化上下文入口，对空文本、空模型和缺失 key 做前置处理。
- 固定发送 `model`、`input`、`encoding_format: "base64"`，允许其他 `Options` 透传，但禁止同名 option 覆盖协议必需字段。
- 把 Bearer 鉴权、JSON POST、取消竞争、响应体限制、状态码处理、错误脱敏和日志记录委托给 `base::execute_json_embedding_call`。
- 用 `decode_indexed_base64_embeddings` 校验响应数量与索引，将乱序结果恢复为输入顺序，并把 base64 或 JSON byte-array 形式的小端 `f32` 字节解码为向量。

## 主要符号

- `DEFAULT_API_BASE_URL: &str`：默认 OpenAI API base，值为 `https://api.openai.com/v1`；`endpoint` 再附加 `/embeddings`。
- `MAX_RESPONSE_BYTES: u64`：`OpenAIConfig` 路径的默认响应体上限，32 MiB。
- `OpenAIEmbedder`：公开 provider 类型。长期持有可复用的 `reqwest::Client`、动态 key/base getter、响应体上限，以及可选的缺 key/401 定制错误。`client` 为 crate 内可见，独立测试可以替换超时配置。
- `OpenAIConfig`：公开基础配置。getter 和错误提示均可缺省；`max_response_bytes <= 0` 表示使用 32 MiB 默认值。
- `OpenAIEmbedder::new(api_key, base_url)`：应用常用构造器，为缺 key与 401 安装带 SQL `SET @@GLOBAL...` 指引的错误，并转交 `with_config`。
- `OpenAIEmbedder::with_config(config)`：本文件原生配置入口，创建 30 秒超时的 client，补齐空 getter和默认大小。
- `OpenAIEmbedder::with_provider_config(config)`：与其他 provider 共用 `base::APIKeyProviderConfig` 的入口；先调用 `with_defaults`，并用 `base::http_client("OpenAI")` 建立统一 client。共享 provider contract 测试通过此入口运行。
- `OpenAIEmbedder::endpoint(&self)`：crate 内 endpoint 解析器。每次调用 getter、trim 空白、校验绝对 HTTP(S) URL，去掉 path 尾斜杠并按需附加 `/embeddings`。
- `Embedder::create_embeddings`：接受传统 `AtomicBool` 取消标志，包装为 `ProviderContext`，最后把 `ProviderError` 映射为字符串。
- `Embedder::create_embeddings_with_context`：实际 provider 主入口，能够保留调用方取消 cause 和传输 cause。
- `decode_indexed_base64_embeddings(body, expected_count)`：crate 内共享成功响应解码器；执行 JSON/字段/数量/索引/字节/f32 校验并按索引排序结果。

本文件没有 trait 定义、条件编译项或文件内测试；测试全部位于独立文件 [`pkg/inference/openai_test.rs`](openai_test.rs)。

## 执行流程

1. `Domain::init_inference_providers` 创建 `EmbedFn`，以 `openai` 注册 `Arc<OpenAIEmbedder>`。传入的两个闭包在调用时读取 Domain 的全局系统变量，所以配置更新不要求重建 provider。
2. 通用 `EmbedFn` 根据 provider/model 标识找到 `Arc<dyn Embedder>`，完成缓存与批处理后调用 provider。直接调用旧 trait 方法时，`create_embeddings` 先把 `AtomicBool` 包成 `ProviderContext`；已有结构化上下文则直接进入 `create_embeddings_with_context`。
3. 主入口首先处理无需网络的分支：`texts` 为空立即成功返回空集合；非空输入要求 `model` 非空。随后调用 key getter，空 key 返回定制错误或通用 `API key is not configured for OpenAI`。
4. 请求字段先由 `Options` 构造：`model` 取函数参数，`input` 取完整文本切片，`encoding_format` 固定为 `base64`。`json_fields_with_options` 先复制调用者 options，再写入固定字段，因此 `dimensions` 等扩展项保留，而调用者伪造的 `model`、`input`、`encoding_format` 会被覆盖。
5. `endpoint` 每次读取当前 base URL；空白配置使用默认 base，否则使用 trim 后值。`parse_http_url` 拒绝相对地址、非 HTTP(S)、缺 host 和非法百分号转义；path 去尾斜杠后，如果尚未以 `/embeddings` 结尾则附加该段，query/fragment 不被丢弃。
6. `provider_auth_headers(..., false)` 生成 `Authorization: Bearer <key>`。`execute_json_embedding_call` 以 JSON POST 执行请求，最多读取 `max_response_bytes`，并让网络 future 与每 5 ms 检查一次的取消 future 竞争。只有 HTTP 200 进入成功解码；401 优先返回配置错误，其他状态尝试读取 `error.message`，日志与展示文本都会对 key 做脱敏。
7. 成功体传给 `decode_indexed_base64_embeddings`。解码器解析顶层对象，读取兼容性的 `model` 字段，并把 `data: null` 视为空数组；数据项数必须等于 `texts.len()`。
8. 每个数据项必须是对象；`index` 缺失/null 时按 Go JSON 零值处理为 0，否则必须是整数且落在 `[0, expected_count)`，并且不能重复。`embedding` 可为 base64 字符串（允许 CR/LF 和 trailing bits）或 JSON byte array（null 元素按 0）；null 则形成空字节并在下一步明确失败。
9. `decode_float32_array_bytes` 要求字节非空且长度为 4 的倍数，以 little-endian 转为 `f32`。结果先写入按 `expected_count` 分配的 `Vec<Option<Vec<f32>>>`，最后按 index 顺序取出，保证返回向量和输入文本对齐。

## 数据与状态

`OpenAIEmbedder` 的持久状态是一个 `reqwest::Client` 和不可变配置句柄。`api_key`、`base_url` 保存为 `Arc<dyn Fn() -> String + Send + Sync>`，保存的是动态 getter，不是构造时的字符串快照；Domain 当前的 getter 内部读取共享 `RwLock`。`missing_key_error` 与 `unauthorized_error` 是可克隆的 `ProviderError`，`max_response_bytes` 在构造时归一化。

每次请求的模型、文本、options、payload、headers、endpoint、响应字节和解码向量均为调用局部值。本文件不保存响应、不实现缓存、不创建 provider 私有队列；这些状态由 `EmbedFn` 管理。请求 options 类型为有序的 `BTreeMap<String, serde_json::Value>`，固定字段覆盖同名调用者值是明确不变量。

响应解码的关键中间状态是长度为 `expected_count` 的 `embeddings: Vec<Option<Vec<f32>>>`。它同时用于检测重复 index 和恢复乱序响应；在“数据项数量等于 expected_count、每个 index 合法且不重复”的联合约束下，最终 `Option::unwrap` 是成立的不变量。字节到浮点的端序固定为 little-endian，与 Go 的共享 base64 解码语义一致。

## 依赖与调用关系

上游应用链路为 `Domain::init_inference_providers` → `EmbedFn::register("openai", ...)` → `EmbedFn` 的 provider 分派/批处理 → `OpenAIEmbedder` 的 `Embedder` 实现。`pkg/inference/lib.rs` 公开 `openai` 模块并再导出 `Embedder`/`Options`。RustCodeGraph 与源码搜索均确认生产接线点位于 `pkg/domain/domain.rs`；本文件本身不直接依赖 SQL session 类型。

主要下游依赖如下：

- `base::parse_http_url`：校验绝对 HTTP(S) endpoint。
- `base::json_fields_with_options`：实现“扩展项保留、固定字段优先”的 payload 合并。
- `base::provider_auth_headers`：构造 Bearer header，并在 header 值非法时返回安全错误。
- `base::execute_json_embedding_call`：管理 tokio current-thread runtime、网络请求、取消、限长读取、HTTP 状态、日志和 secret 脱敏。
- `base::{ensure_json_object, string_field}`：模拟 Go JSON 对对象与字符串零值/类型的检查。
- `base::decode_float32_array_bytes`：把 wire bytes 转成 little-endian `f32`。
- `base64`：解码 OpenAI-compatible 的 base64 embedding；`reqwest` 与 `serde_json` 分别提供 client/URL 和 JSON 值处理。

反向复用关系也很重要：[`pkg/inference/jina.rs`](jina.rs) 与 [`pkg/inference/nvidia.rs`](nvidia.rs) 直接调用 `decode_indexed_base64_embeddings`；[`pkg/inference/tidbcloud.rs`](tidbcloud.rs) 把单项响应包装成 indexed data 后调用它。因此修改该解码器会影响四个 provider，而不仅是 OpenAI。

crate 边界由 [`pkg/inference/Cargo.toml`](Cargo.toml) 定义：package 为 `astersql-inference`，库入口为 `lib.rs`；直接依赖包含 `base64`、带 blocking/json/rustls-tls 功能的 `reqwest`、`serde_json`，共享执行层还使用 `tokio`、`hyper`/`rustls` 与 `logutil`。Cargo 与源码中没有控制 `openai` 模块的 feature gate。

## 错误处理与边界

- 空 `texts` 总是成功返回空集合，发生在模型、key、endpoint 和取消检查之前；因此即使模型为空或取消标志已设置也不报错。非空输入的空模型返回 `model name is required`。
- key getter 返回空字符串时不发网络请求。`new` 提供带系统变量配置指引的消息；默认 `OpenAIConfig` 使用通用消息。
- endpoint 必须是带 host 的绝对 HTTP(S) URL。已有 `/embeddings` 后缀不会重复添加，尾斜杠会清理，query/fragment 保留；非法 scheme、相对地址和错误转义均返回 `ProviderError`。
- 固定请求字段不能由 options 覆盖。payload 的 `serde_json::to_value(...).expect("JSON fields")` 依赖 `BTreeMap<String, Value>` 必能编码为 JSON object 的本地不变量，不是远端可触发的常规错误分支。
- 只有精确 HTTP 200 被视为成功。401 使用专用配置错误；其余非 200 状态从 `error.message` 生成 `OpenAI: status code ..., message: ...`，JSON/字段解析失败则退回 Go-compatible HTTP status text。
- 共享执行器对成功与错误响应应用同一字节上限，超限返回明确错误。API key、Bearer token和常见 credential 字段在日志/展示消息中脱敏，错误日志不记录整个响应 body；传输错误移除 URL 并保留为可检查 cause。
- 成功响应的 `data` 数量必须与输入数一致。index 必须为非负整数、范围合法且唯一；否则拒绝结果，避免将向量错误配给输入。
- base64 字符串可含 CR/LF，并允许 trailing bits 以贴合 Go 解码兼容性；非法 base64、非法 byte-array 元素、空字节或非 4 字节倍数均报错。JSON byte array 中 null 按 Go `[]byte` 解码零值处理为 0。
- 顶层 `model` 和 item 的 `object` 通过 `string_field` 检查：缺失/null 对应空字符串，非字符串才失败。这些字段用于协议形态校验，不参与业务结果。

## 并发与资源生命周期

`OpenAIEmbedder` 不自行创建长期线程、锁、通道或关闭钩子。它满足 `Embedder: Send + Sync`，由 Domain 以 `Arc<dyn Embedder>` 长期持有；`reqwest::Client` 在请求间复用连接。getter 的 `Send + Sync` 约束允许跨线程调用，其内部同步由传入方负责。

每次 HTTP 调用由共享执行器临时创建 current-thread tokio runtime，并在其中驱动 reqwest future。调用开始前会检查 `ProviderContext::cause()`；运行中取消 future 每 5 ms 观察 `AtomicBool`/自定义 cause，一旦取消先于请求完成，网络 future 被丢弃并返回取消原因。独立测试证明延迟响应不会迫使调用等待服务器完成。

批处理窗口、请求合并、缓存和等待者取消由 `EmbedFn` 生命周期管理，不应在本 provider 重复实现。响应体只在请求期间以内存 `Vec<u8>` 有界累积，最大值由配置决定；payload、headers、响应字节与解码临时状态在调用结束后按 Rust 所有权释放。provider 不持有 API key 字符串超过单次请求，但其 getter和定制错误与 provider 同寿命。

## 与 Go 版本的对应关系

直接 Go 对照为 [`pkg/inference/embedding/openai/openai.go`](embedding/openai/openai.go)，协议类型位于 [`pkg/inference/embedding/openai/protocol.go`](embedding/openai/protocol.go)，测试位于 [`pkg/inference/embedding/openai/openai_test.go`](embedding/openai/openai_test.go)。两端保持的核心契约包括：默认 base、`/embeddings` 规范化、空输入短路、模型必填、动态 key/base 配置、固定请求字段优先、Bearer header、响应体上限、仅 200 成功、401 专用错误、错误脱敏，以及 indexed-base64 响应的数量/索引校验与乱序恢复。

Go 的 `Embedder`/`base.APIKeyProviderConfig`/`base.ExecuteJSONEmbeddingCall` 分别对应 Rust 的 `OpenAIEmbedder`、`OpenAIConfig` 或 `base::APIKeyProviderConfig`、`base::execute_json_embedding_call`。Go 通过 `context.Context` 传播取消；Rust 同时保留 `AtomicBool` 兼容入口和可携带原始 cause 的 `ProviderContext`。Go 把 `Request`、`Response`、`ErrorResponse` 定义为静态 struct；Rust 用 `Options` 与 `serde_json::Value` 组合请求和读取响应。

解码器的位置存在结构差异：Go `openai.decodeEmbeddings` 将 `Response.Data` 交给 `embedding/base.DecodeIndexedBase64Embeddings`，因此共享实现位于 base 包；Rust 的共享 indexed-base64 解码器目前位于 `openai.rs`，由 Jina/NVIDIA/TiDB Cloud 反向依赖。Rust 还显式接受 embedding 的 JSON byte-array 形态及 null byte 元素，以复现 Go `encoding/json` 对 `[]byte` 的兼容零值行为。

Rust 原生 `with_config` 使用本文件的 32 MiB/30 秒默认值；`with_provider_config` 则走共享 provider 默认配置，更接近 Go 构造器。`new` 额外固化了面向 AsterSQL/TiDB 系统变量的用户指引，而 Go 构造器从调用方接收完整配置对象。

## 扩展指南

- 新增普通 OpenAI 请求 option 通常只需由调用方放入 `Options`，并在 `pkg/inference/openai_test.rs` 增加 wire payload 断言。协议必需字段仍应在 `fields` 中最后覆盖；若要允许覆盖，必须同步评估 Go 行为、批处理键和缓存一致性。
- 修改 endpoint 规则时集中调整 `OpenAIEmbedder::endpoint`，覆盖默认 base、已有后缀、尾斜杠、嵌套 path、query、fragment、非法 scheme/相对地址和错误转义。不要在调用者重复拼接路径。
- 新增状态码专用错误时扩展传给 `execute_json_embedding_call` 的 `status_error` 闭包，并同步 Go 的 `StatusErrors`、用户可操作提示、secret 脱敏和 cause 测试。
- 修改响应协议时优先扩展 `decode_indexed_base64_embeddings`，同时保持数量、唯一 index、范围和输入顺序不变量。由于 Jina、NVIDIA、TiDB Cloud 复用它，必须同步运行/更新各自独立 `*_test.rs`，避免 OpenAI 特例破坏兼容 provider。
- 若要支持非 base64 或不同数值类型，需同时修改固定 `encoding_format`、解码器和 `Embedder` 的 `Vec<Vec<f32>>` 契约；仅放宽 wire decoder 会造成请求/返回语义不一致。
- 若要改变超时或 response limit，应优先统一 `OpenAIConfig` 与 `APIKeyProviderConfig` 两条构造路径，并核对 Go `base.DefaultHTTPClientTimeout`/`MaxResponseBodyBytes`。性能风险集中在大批量 JSON、base64 临时分配和完整响应体内存占用。
- Rust 回归测试继续放在独立文件 `pkg/inference/openai_test.rs`，不得嵌入生产源文件；共享 provider 契约复用 `cohere_test::provider_contract`。Go 对照变更同步检查 `pkg/inference/embedding/openai/openai_test.go`。
- 批处理、缓存、并发或 provider 分派扩展应落在 `embed_fn.rs`；网络生命周期或脱敏共性应落在 `base.rs`，避免在 OpenAI adapter 内复制基础设施。

## 验证依据

- RustCodeGraph 索引检查：`rustcodegraph status` 报告 11,467 个已索引文件、307,296 个节点、1,848,419 条边；`files --filter pkg/inference` 确认目标源、crate 入口、独立 Rust 测试和 Go 对照均被覆盖。
- 目标源码查询：`rustcodegraph node --file pkg/inference/openai.rs --offset 1 --limit 260` 与后续尾段查询覆盖文件全部 268 行；符号查询确认 `OpenAIEmbedder`、`OpenAIConfig`、三个构造入口、`endpoint`、两个 trait 方法和 `decode_indexed_base64_embeddings`。
- 应用与反向调用证据：`pkg/domain/domain.rs` 的 `Domain::init_inference_providers` 注册 `openai` 并读取两个系统变量；源码/图查询确认 `jina.rs`、`nvidia.rs`、`tidbcloud.rs` 复用 indexed-base64 解码器。
- crate 与共享执行证据：`pkg/inference/Cargo.toml`、`pkg/inference/lib.rs`、`pkg/inference/base.rs`；后者验证固定字段覆盖、URL 校验、30 秒默认超时、限长读取、取消竞争、状态处理、脱敏和 little-endian `f32` 解码。
- Rust 独立测试证据：`pkg/inference/openai_test.rs` 覆盖 POST path/header/payload、乱序重排、options 与固定字段优先级、endpoint 变体、缺 key/空模型/空输入、定制错误、401/其他状态、日志脱敏、响应大小边界、超时、非法 base64/索引/字节长度、Go JSON 零值、取消以及共享 provider contract。
- Go 对照证据：`pkg/inference/embedding/openai/openai.go`、`protocol.go`、`openai_test.go`，覆盖相同成功路径、endpoint、options、index 校验、timeout、鉴权/远端错误和通用 contract。
- 本任务只产出文档，按总计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工检查变更范围仅包含本说明文档及按技能要求删除的编号任务文件，未修改 Rust、Go、Cargo 或只读 `plan.md`。
