# `pkg/inference/nvidia.rs`

## 文件定位

`pkg/inference/nvidia.rs` 是 `astersql-inference` crate 中的 NVIDIA NIM embedding provider 适配器。模块由 `pkg/inference/lib.rs` 公开为 `astersql_inference::nvidia`；`pkg/domain/domain.rs::init_inference_providers` 构造 `NvidiaEmbedder`，并以 provider 名 `nvidia_nim` 注册到 Domain 持有的 `EmbedFn`。因此它位于 SQL embedding 调用链的外部服务边界：上游负责 provider 选择、合批与缓存，本文件负责把一次批量 embedding 请求转换成 NVIDIA NIM 的 HTTP 协议并解码结果。

该 crate 的边界由 `pkg/inference/Cargo.toml` 定义，包名为 `astersql-inference`，`lib.rs` 是库入口。本文涉及的直接依赖包括 `reqwest`（HTTP client/URL）、`serde_json`（请求与响应 JSON）和 `base64`（经 `openai.rs` 的兼容解码器间接使用）；HTTP 客户端启用 `rustls-tls` 且关闭 `reqwest` 默认 feature。

## 核心职责

- `NvidiaEmbedder` 保存可复用的 `reqwest::Client` 和 `APIKeyProviderConfig`，实现统一的 `Embedder` trait（`nvidia.rs::NvidiaEmbedder`、`embed_fn.rs::Embedder`）。
- `new` 为 Domain 的常规接线提供动态 API key/base URL getter，以及面向系统变量的缺 key和鉴权失败提示；`with_config` 则提供测试和自定义接线入口。
- `endpoint` 将空配置映射到 `DEFAULT_BASE_URL`，并拒绝非绝对 HTTP(S) URL（`nvidia.rs::endpoint`、`base.rs::parse_http_url`）。NVIDIA 配置值已被视为完整 embeddings endpoint，不再追加模型或路径。
- `create_embeddings_with_context` 校验输入，构造 NVIDIA NIM JSON 请求、Bearer 鉴权和状态码映射，再把通用 HTTP 生命周期委托给 `base.rs::execute_json_embedding_call`。
- `decode_embeddings` 先校验 NVIDIA/OpenAI 兼容响应的外围字段，再复用 `openai.rs::decode_indexed_base64_embeddings` 还原按输入索引排列的 `Vec<Vec<f32>>`。

本文件不负责 provider 路由、批处理、缓存、SQL 函数语义或系统变量维护；这些分别属于 `EmbedFn`、表达式/Domain 层。本文件也不实现重试，当前通用执行器只执行一次 HTTP 调用。

## 主要符号

- `DEFAULT_BASE_URL: &str`：默认完整 endpoint，值为 `https://integrate.api.nvidia.com/v1/embeddings`。
- `pub struct NvidiaEmbedder`：provider 实例。`client` 为 crate 内可见，便于同 crate 测试或接线替换行为；`cfg` 私有，包含延迟求值的 API key/base URL getter、自定义错误和最大响应体大小。
- `NvidiaEmbedder::new(api_key, base_url) -> Self`：公开便捷构造器。两个 getter 均要求 `Send + Sync + 'static`，允许实例被 `Arc<dyn Embedder>` 跨线程共享。它安装 TiDB NVIDIA 系统变量相关的缺 key/未授权提示后转交 `with_config`。
- `NvidiaEmbedder::with_config(cfg) -> Self`：创建带 30 秒默认超时的 HTTP client，并通过 `APIKeyProviderConfig::with_defaults` 将非正的响应体上限替换为 32 MiB（`base.rs::http_client`、`DEFAULT_HTTP_TIMEOUT`、`DEFAULT_MAX_RESPONSE_BYTES`）。
- `NvidiaEmbedder::endpoint(&self, model) -> Result<reqwest::Url, ProviderError>`：读取并 trim base URL；为空时用默认值，否则原样解析完整 URL。`model` 当前不参与 URL 构造，显式的 `let _ = model` 表明模型只进入请求体。
- `Embedder::create_embeddings(...) -> Result<_, String>`：兼容旧调用者的入口；创建只有 `AtomicBool` 的 `ProviderContext`，调用上下文版本并把结构化错误转换为展示字符串。
- `Embedder::create_embeddings_with_context(...) -> Result<_, ProviderError>`：主要执行入口，可保留调用者取消原因和底层 cause。
- `decode_embeddings(body, expected)`：私有成功响应解码器。它验证 JSON 顶层、`object`、`usage` 以及可选 token 计数的类型，然后调用共享的索引/base64 解码器。

## 执行流程

1. Domain 初始化时，`pkg/domain/domain.rs::init_inference_providers` 创建 `NvidiaEmbedder::new`，API key getter 每次从 `global_system_variables["tidb_exp_embed_nvidia_nim_api_key"]` 读取当前值，base URL getter 返回空串以采用默认 endpoint；随后 `EmbedFn::register("nvidia_nim", ...)` 完成路由接线。
2. 上游 `EmbedFn` 按 provider/model/options 选择并可能合批请求，再通过 `Embedder` trait 调用本适配器（`pkg/inference/embed_fn.rs`）。旧入口 `create_embeddings` 立即包装为 `ProviderContext`；带上下文入口直接进入主流程。
3. `create_embeddings_with_context` 对空 `texts` 立即返回空向量，不读取 key、URL 或发起网络请求；非空请求要求 `model` 非空。若 options 包含 `embedding_type`，其 JSON 值必须恰好等于字符串 `"float"`，否则也在读取配置前失败（`nvidia_test.rs::nvidia_protocol_covers_float_options_error_schemas_and_status_overrides`）。
4. 配置层延迟解析 API key 和 endpoint。请求固定字段为 `model`、`input`、`encoding_format: "base64"`；`base.rs::json_fields_with_options` 先复制用户 options、再覆盖固定字段，所以用户不能替换这三个协议不变量，但 `embedding_type`、`input_type` 等扩展字段会保留。
5. `base.rs::provider_auth_headers` 生成 `Authorization: Bearer <key>`。随后 `base.rs::execute_json_embedding_call` 建立 current-thread Tokio runtime，发送 JSON POST，以 5 ms 轮询取消状态，并限制响应体大小；只有 HTTP 200 进入成功解码。
6. 非 200 响应优先从 `detail`、`message`、`error` 中选择第一个非空消息。401/403 映射为配置的鉴权错误，404 映射为包含 model 的不存在/不可用错误；其他状态使用经过 secret 脱敏与长度限制的消息，缺少可解析消息时回退到 HTTP status text。
7. HTTP 200 时，`decode_embeddings` 做 NVIDIA 外层字段校验，再由 `openai.rs::decode_indexed_base64_embeddings` 检查 `data` 数量和每项索引，base64 解码 little-endian `f32` 字节，按 `index` 重排并返回与输入条数相同的向量列表。

## 数据与状态

`NvidiaEmbedder` 自身只有两个长期字段：不可变且可共享的 `reqwest::Client`，以及不可变的 `APIKeyProviderConfig`。配置中的 getter 是 `Arc<dyn Fn() -> String + Send + Sync>`，所以 key 与 URL不是构造时快照；特别是 Domain 的 key getter会在每次请求时重新读取全局变量。`with_defaults` 只归一化响应体大小，不提前调用 getter。

输入 `texts: &[String]` 和 `opts: &BTreeMap<String, Value>` 均为借用数据；请求构造会把它们序列化为临时 JSON `Value`。固定请求字段覆盖同名 options 是重要不变量：实际 model/texts 总是来自函数参数，编码格式总是 base64。返回值按输入位置排列；服务端可以乱序返回 `data`，但索引必须唯一且处于 `[0, expected)`，并且条目数必须等于输入条数（`openai.rs::decode_indexed_base64_embeddings`）。

取消状态不存放在 provider 内，而由每次调用的 `ProviderContext` 借用 `AtomicBool` 和可选 cancellation-cause 回调。API key作为 secret 传给通用执行器，仅用于 header、发送以及错误文本脱敏，不应进入成功结果或日志。

## 依赖与调用关系

上游主链为 `pkg/domain/domain.rs::init_inference_providers` → `EmbedFn::register("nvidia_nim", Arc<NvidiaEmbedder>)` → `pkg/inference/embed_fn.rs` 的 provider 调用 → `NvidiaEmbedder::create_embeddings_with_context`。RustCodeGraph 还确认本文件内部边为 `create_embeddings` → `create_embeddings_with_context`、`create_embeddings_with_context` → `endpoint`/`decode_embeddings`。

下游直接依赖如下：

- `crate::base::APIKeyProviderConfig`：延迟配置、缺 key/鉴权错误和响应体上限。
- `base::parse_http_url`、`http_client`、`provider_auth_headers`：URL、client 和 Bearer header 的公共规则。
- `base::json_fields_with_options`：合并扩展 options，同时保证固定字段优先。
- `base::execute_json_embedding_call`：实际 POST、超时/取消、响应体限额、错误 cause、脱敏日志和 status 分派。RustCodeGraph 查询显示 NVIDIA 是该共享执行器的七个 provider 调用者之一。
- `crate::openai::decode_indexed_base64_embeddings`：成功响应中 `data[index].embedding` 的共享解码逻辑；RustCodeGraph 明确解析到 `nvidia.rs::decode_embeddings` → 该函数的调用边。

`pkg/inference/Cargo.toml` 没有 NVIDIA 专用 feature；该模块随整个 crate 编译并由 `lib.rs` 公开。`pkg/inference/nvidia_test.rs` 由 `lib.rs` 的 `#[cfg(test)] mod nvidia_test` 独立装配，生产代码与测试未放在同一文件。

## 错误处理与边界

无需网络的边界按固定顺序处理：空 texts 成功返回空列表；随后检查空 model；随后检查 `embedding_type`；之后才读取 API key 和 endpoint。这保证无效参数不会触发 getter 或请求。`embedding_type` 的数字、其他字符串及其他 JSON 类型全部拒绝，只允许省略或字符串 `"float"`。

endpoint 会 trim 外层空白，只接受带 host 的绝对 `http`/`https` URL，并额外拒绝 Go `url.Parse` 不接受的畸形百分号转义（`base.rs::parse_http_url`）。空 API key返回自定义或默认 `ProviderError`。header 值非法、runtime/client/transport/read body 失败时，通用层保留底层 error cause，同时向外展示已规整的 provider 错误。

成功响应不是宽松地只读 `data`：顶层必须为 object 或 null；`object` 若存在必须是字符串；`usage` 必须为 object 或 null；`prompt_tokens`/`total_tokens` 若存在必须是整数。共享解码器继续验证 `model`/每项 `object` 的字符串类型（允许字段缺失为 null）、条数、索引范围与唯一性、base64/byte-array 格式，以及解码字节非空且长度可被 4 整除。任一失败都返回错误，不产生部分结果。

错误响应兼容 hosted/self-hosted 的三种消息字段，优先级为 `detail` > `message` > `error`。日志和普通 status 错误会对当前 API key、常见凭据 JSON、Bearer token 和 `sk-` key 形态脱敏，并将文本限制在 4096 字符；401/403/404 的 provider 特定错误覆盖通用消息。

## 并发与资源生命周期

本文件不创建共享锁、后台线程、队列或缓存。`NvidiaEmbedder` 满足 `Embedder: Send + Sync`，通常被 Domain 放入 `Arc<dyn Embedder>`；共享 `reqwest::Client` 复用连接资源。Domain 注入的 API key getter会短暂获取全局系统变量 `RwLock` 的读锁，provider 本身不持有该锁跨越网络调用。

每个实际调用在 `base::execute_json_embedding_call` 中创建一个 current-thread Tokio runtime，并在该调用返回时销毁。HTTP future 与取消 future在同一 runtime 中轮询；取消检测间隔为 5 ms，检测到 caller cause 或 `AtomicBool` 后返回该原因并丢弃请求 future。30 秒 client timeout 和配置的响应体字节上限约束外部资源占用。

更上层的合批、等待者取消和 worker `JoinHandle` 生命周期由 `pkg/inference/embed_fn.rs::EmbedFn` 管理，不应在本文件复制。扩展本适配器时须保持函数无跨调用可变状态，避免破坏 `Arc` 共享和上层并发假设。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/inference/embedding/nvidia/nvidia.go`。类型映射为 Go `Embedder` ↔ Rust `NvidiaEmbedder`，`NewNvidiaEmbedder` ↔ `new`/`with_config`，`embeddingsEndpoint` ↔ `endpoint`，`validateEmbeddingType` ↔ `create_embeddings_with_context` 的内联校验，`decodeErrorMessage` ↔ 错误消息 closure，`decodeEmbeddings` ↔ Rust 私有同名函数，`CreateEmbeddings` ↔ `Embedder` 实现。

两版保持的行为包括：默认完整 endpoint；空 texts短路；model 必填；只允许 float embedding type；固定字段覆盖同名 options；Bearer key；401/403/404 特定错误；多种 NVIDIA 错误 schema；按 index 重排并解码 base64 little-endian float32。Rust 的 `pkg/inference/nvidia_test.rs` 以独立测试覆盖这些协议，并复用 `cohere_test::provider_contract` 验证取消、响应体上限、transport cause 和 secret 脱敏；Go 的 `pkg/inference/embedding/nvidia/nvidia_test.go` 覆盖相同主干及错误边界。

可见差异是 Rust `decode_embeddings` 在进入共享 `data` 解码前显式检查 `object`、`usage` 和 token 字段类型，而当前 Go `decodeEmbeddings` 直接反序列化 `Response.Data`。Rust 使用 `ProviderContext` 保留取消 cause，并在共享 async 执行器中轮询 `AtomicBool`；Go 直接使用 `context.Context`。这些是运行时表达方式与额外校验差异，不应在后续改动中擅自删减以追求表面一致。

## 扩展指南

- 调整 NVIDIA 请求字段时，修改 `create_embeddings_with_context` 中的固定 `fields` 或 options 校验；先决定字段能否被用户覆盖。协议不变量应继续放在固定 fields 中，保证其覆盖 options。
- 支持新的 NVIDIA 错误 schema 时，扩展传给 `execute_json_embedding_call` 的消息提取 closure；保持 `detail`/`message`/`error` 的既有优先级，并确保新消息仍经过通用脱敏。
- 新增状态码语义时，扩展 `status` closure；不要把 provider 特定规则塞入 `base.rs`，除非其他 provider共享同一契约。
- 改动 endpoint 语义时重点修改 `endpoint`，并确认配置仍表示完整 endpoint。当前 `model` 不进入路径；若服务协议改变，必须同步说明路径转义、query 保留及 Go 对照。
- 改动响应格式时，优先在私有 `decode_embeddings` 加 NVIDIA 外层验证；只有确实共享的 index/base64 格式才修改 `openai.rs::decode_indexed_base64_embeddings`，因为 Jina、TiDB Cloud 等调用者也受影响。
- 测试必须继续放在独立的 `pkg/inference/nvidia_test.rs`，不要嵌入生产源文件。同步覆盖请求 payload、校验先后、所有 status/schema、响应字段类型、索引/字节边界、取消 cause、响应体上限和 secret 脱敏；若改变跨语言契约，还应同步核对 `pkg/inference/embedding/nvidia/nvidia_test.go`。
- 兼容性风险主要是配置 URL 含义、错误字符串和 option 覆盖顺序；性能风险主要来自每请求 runtime、响应体大小与上层批量尺寸。任何优化都应保留取消、超时、限额、脱敏和精确条数/索引验证。

## 验证依据

- RustCodeGraph `status`：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录及 `nvidia.rs` 均在索引内。
- RustCodeGraph 完整读取：`pkg/inference/nvidia.rs`（143 行），确认 `DEFAULT_BASE_URL`、`NvidiaEmbedder`、两个构造器、`endpoint`、两个 trait 方法与 `decode_embeddings`。
- RustCodeGraph 精确查询/调用证据：`NvidiaEmbedder` 位于 `nvidia.rs:22`，`decode_embeddings` 位于 `nvidia.rs:131`；查询确认 `create_embeddings_with_context` 调用 `base.rs::execute_json_embedding_call`，并确认 `nvidia.rs::decode_embeddings` 调用 `openai.rs::decode_indexed_base64_embeddings`。
- 上游接线：`pkg/inference/lib.rs`；`pkg/domain/domain.rs::init_inference_providers` 中 `nvidia_nim` 注册及 `tidb_exp_embed_nvidia_nim_api_key` getter；`pkg/inference/embed_fn.rs` 的 `Embedder` trait、`EmbedFn::register` 及请求入口。
- 下游实现：`pkg/inference/base.rs` 的配置、URL/header、HTTP/取消/限额/脱敏生命周期；`pkg/inference/openai.rs::decode_indexed_base64_embeddings` 的长度、索引和 float32 解码规则。
- crate 边界：`pkg/inference/Cargo.toml`；模块/测试装配：`pkg/inference/lib.rs`。
- 独立 Rust 测试：`pkg/inference/nvidia_test.rs`，覆盖请求、float option、共享 provider contract、状态码/错误 schema、默认与非法 endpoint。
- Go 对照与测试：`pkg/inference/embedding/nvidia/nvidia.go`、`pkg/inference/embedding/nvidia/nvidia_test.go`。
- 本任务只新增说明文档；依照计划不运行 Cargo。交付前另以任务给定命令验证本文恰有十一个固定二级标题，并人工检查所有行为陈述均可回溯到上述符号或文件。
