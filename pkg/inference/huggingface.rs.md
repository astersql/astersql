# `pkg/inference/huggingface.rs`

## 文件定位

本文件是 `astersql-inference` crate 中 HuggingFace Feature Extraction API 的 Rust provider 适配器。crate 入口 `pkg/inference/lib.rs` 以 `pub mod huggingface` 暴露它，并从 `embed_fn` 重导出统一的 `Embedder`、`EmbedFn` 与 `Options` 接口。应用启动时，`pkg/domain/domain.rs` 的 embedding provider 初始化逻辑构造 `HuggingFaceEmbedder::new`，从全局变量 `tidb_exp_embed_huggingface_api_key` 动态读取密钥，并以名称 `huggingface` 注册到 Domain 持有的 `EmbedFn`。

文件本身不实现缓存、跨请求合批或 SQL 表达式接线；这些职责属于 `pkg/inference/embed_fn.rs`。它也不自行实现完整 HTTP 生命周期，而是把 HuggingFace 特有的 URL、请求字段、错误状态和响应解码规则交给 `pkg/inference/base.rs` 的共享调用框架。

所属 crate 由 `pkg/inference/Cargo.toml` 定义，包名是 `astersql-inference`。本实现直接使用 `reqwest` 的异步客户端类型、`serde_json` 和标准库原子取消标志；同步的 provider trait 由共享基座在单线程 Tokio runtime 上驱动该客户端。共享基座还使用 Cargo 中声明的 `tokio`、`hyper`/`hyper-util`、`hyper-rustls`、`rustls` 与 `logutil` 完成请求、特殊转义路径和错误日志处理。

## 核心职责

1. `HuggingFaceEmbedder::new` 将两个可并发调用且具有 `'static` 生命周期的配置读取闭包装入 `APIKeyProviderConfig`，并设置面向 TiDB 用户的缺失密钥和未授权提示。
2. `HuggingFaceEmbedder::with_config` 支持测试或高级调用者直接提供完整配置，为 provider 建立带共享默认超时的 HTTP client，并补齐默认响应体上限。
3. `HuggingFaceEmbedder::endpoint` 校验基础 URL，把模型名按 `/` 保留层级、逐段转义，再拼成 `/models/<model>/pipeline/feature-extraction`。
4. `Embedder::create_embeddings_with_context` 校验输入、解析密钥、合并请求选项并执行 JSON POST；固定的 `inputs` 字段拥有最终优先级，调用者不能通过 `Options` 替换真实文本。
5. `decode_embeddings` 将 HuggingFace 直接返回的二维 JSON 数组转换为 `Vec<Vec<f32>>`，并强制输出行数等于输入文本数。

因此，这个文件的边界是“统一 embedding provider 接口到 HuggingFace HTTP 协议的翻译”，而不是 embedding 的调度器或模型实现。

## 主要符号

- `DEFAULT_BASE_URL: &str`：默认值为 `https://router.huggingface.co/hf-inference`；只有配置闭包返回值去除首尾空白后为空时才采用它。
- `pub struct HuggingFaceEmbedder`：provider 实例。`client: reqwest::Client` 在 crate 内可见，供同 crate 测试或接线替换/观察；`cfg: APIKeyProviderConfig` 私有，保存动态密钥/基础 URL 读取器、自定义错误和响应体上限。
- `pub fn HuggingFaceEmbedder::new(...) -> Self`：常规构造入口。两个闭包都要求 `Fn + Send + Sync + 'static`，允许实例作为 `Arc<dyn Embedder>` 在线程间共享。
- `pub fn HuggingFaceEmbedder::with_config(cfg) -> Self`：完整配置入口，调用 `base::http_client("HuggingFace")` 和 `cfg.with_defaults()`。
- `pub(crate) fn HuggingFaceEmbedder::endpoint(&self, model) -> Result<ProviderEndpoint, ProviderError>`：crate 内端点生成器。它保留模型名中的 `/` 作为命名空间分隔，只转义每一个 segment；例如空格变为 `%20`、`?` 变为 `%3F`，完整 `..` segment 变为 `%2E%2E`。
- `impl Embedder for HuggingFaceEmbedder`：公开行为契约。兼容入口 `create_embeddings` 构造不带自定义 cause 的 `ProviderContext`，把结构化 `ProviderError` 降为字符串；增强入口 `create_embeddings_with_context` 则保留可检查的调用者或传输错误 cause。
- `fn decode_embeddings(body, expected)`：文件私有的成功响应解码器；先解析 JSON，再复用 `base::decode_float_rows` 处理数值、`null` 与 `f32` 溢出，最后验证结果数量。

本文件没有 trait、枚举、宏、条件编译项或模块级可变状态。

## 执行流程

典型调用链如下：

1. Domain 创建 provider，并调用 `EmbedFn::register("huggingface", Arc<...>)`。上层把模型写成 `huggingface/<实际模型名>` 时，`EmbedFn::request` 拆出 provider 名和模型名，按模型与选项组织合批，然后由 `run_batch` 经 `Arc<dyn Embedder>` 发起 provider 调用。
2. 兼容 trait 路径进入 `create_embeddings`，它用传入的 `AtomicBool` 建立 `ProviderContext`，再调用 `create_embeddings_with_context`。直接需要保留调用者 cause 的代码也可以使用后者。
3. 空 `texts` 立即返回空向量，不解析模型、密钥或 URL，也不发网络请求；非空文本而 `model` 为空时返回 `model name is required`。
4. `cfg.resolve_api_key` 每次调用配置闭包。密钥为空时返回配置的专用提示或 fallback 错误，不创建请求。
5. `endpoint` 每次调用基础 URL 闭包，去除首尾空白并选择默认 URL；`base::parse_http_url` 只允许带 host 的绝对 HTTP(S) URL。模型名以 `/` 切分，每段经 `base::escape_url_path_segment` 处理，然后接到基础路径之后。`ProviderEndpoint::with_path` 额外保留 escaped path，以避免 WHATWG URL 归一化吞掉 `%2E%2E` 这类语义。
6. 请求字段先由 `{"inputs": texts}` 构成 `Options`，再传给 `base::json_fields_with_options(fields, opts)`。该函数先复制 `opts`、后写固定字段，所以同名 `inputs` 选项会被真实文本覆盖，其余如 `normalize`、`truncate` 会保留。
7. `base::provider_auth_headers(..., false)` 创建 `Authorization: Bearer <key>`；随后 `base::execute_json_embedding_call` 以 JSON POST 执行请求，并以 `texts.len()` 作为期望响应行数。
8. HTTP 200 时调用 `decode_embeddings`。非 200 时先尝试从 JSON 的 `error` 字段提取并脱敏消息，再让本文件覆盖 401 和 404；其他状态采用共享格式 `HuggingFace: status code <n>, message: <message>`。
9. 成功结果返回上层；由 `EmbedFn` 发起的调用还会在共享层拆分合批结果、唤醒等待者，并按其自身规则缓存单文本结果。

## 数据与状态

`HuggingFaceEmbedder` 的持久状态只有一个可复用的 `reqwest::Client` 和一份 `APIKeyProviderConfig`。密钥与基础 URL 并非构造时快照：配置内保存 `Arc<dyn Fn() -> String + Send + Sync>`，所以每次请求都会重新解析当前值，Domain 中的密钥闭包会重新读取全局系统变量。自定义缺失密钥/401 错误使用可克隆的 `ProviderError`；`with_defaults` 将非正的 `max_response_bytes` 替换为共享默认值 32 MiB。

单次请求的文本切片、模型名和选项都只借用到调用结束。payload 是临时 `serde_json::Value`；它至少包含字符串数组 `inputs`，并可包含调用者提供的额外 JSON 选项。成功响应的业务表示为 `Vec<Vec<f32>>`，外层元素必须与输入文本一一对应。本文件不验证所有行的维度一致，也不施加维数上限；单文本经 `EmbedFn::embed_with_context_values` 返回时，上层另行拒绝超过 16,383 维的向量。

`decode_float_rows` 的共享语义值得保留：顶层 JSON `null` 解为零行，行 `null` 解为空向量，元素 `null` 解为 `0.0`；非数组、非数字和转成 `f32` 后非有限的数值会失败。随后 `decode_embeddings` 的数量检查通常会使非空请求对应的顶层 `null` 失败。

## 依赖与调用关系

上游直接接线与调用证据：

- `pkg/inference/lib.rs` 公开 `huggingface` 模块并只在 `cfg(test)` 下装入独立的 `huggingface_test`。
- `pkg/domain/domain.rs` 构造 `HuggingFaceEmbedder::new`，将其注册为 `huggingface` provider，并把 `tidb_exp_embed_huggingface_api_key` 的动态读取闭包交给它。
- `pkg/inference/embed_fn.rs` 定义 `Embedder` trait、provider registry、缓存与合批。`run_batch` 在 worker 线程中通过 trait object 调用 provider；本类型满足 `Embedder: Send + Sync`。

本文件的主要下游依赖：

- `base::APIKeyProviderConfig`：动态配置、默认响应上限及可定制错误。
- `base::parse_http_url`、`escape_url_path_segment`、`ProviderEndpoint`：严格 URL 校验与保真路径构造。
- `base::json_fields_with_options`：选项合并，保证固定字段覆盖调用者同名字段。
- `base::provider_auth_headers`：Bearer 鉴权头及无效 header/cancellation cause 映射。
- `base::execute_json_embedding_call`：current-thread Tokio runtime、HTTP POST、取消轮询、响应体上限、错误消息脱敏、日志和状态分派。
- `base::string_field`、`decode_float_rows`：错误字段和成功二维浮点数组的协议解码。
- `serde_json`：payload 构造及响应解析；`reqwest`：client、URL、header 与网络请求。

RustCodeGraph 将 `pkg/inference/huggingface.rs` 标为被 Domain、共享 provider 合约测试与本文件测试等使用；精确 Rust 文本引用进一步确认生产构造点位于 `pkg/domain/domain.rs`。图索引对重名的 provider 方法不能完整区分，因此方法级下游边同时以源码中的显式调用和相邻共享基座核验，未把模糊的全仓库同名结果当成证据。

## 错误处理与边界

- 空输入是无副作用成功；空模型仅在存在输入时失败。这一顺序与 Go 实现一致。
- 空 API key 在请求前失败。`new` 提供带 `SET @@GLOBAL.TIDB_EXP_EMBED_HUGGINGFACE_API_KEY=...` 指引的消息；`with_config` 可覆盖该错误。
- 基础 URL 必须是合法绝对 HTTP(S) URL且包含 host；非法百分号、相对路径和非 HTTP(S) scheme 被共享解析器拒绝。配置值会 trim，但原 URL 的 query 可由 `ProviderEndpoint` 保留。
- 模型名不被当成未转义路径整体拼接：`/` 是层级分隔，其他保留风险字符按 segment 转义。此规则防止空格、查询字符和 `.`/`..` 被 URL 解析器重新解释。
- 认证 header 无法编码时，错误包装为 `HuggingFace request failed` 并保留底层 cause；若此时调用者已经取消，则优先返回调用者的 cause。
- 401 返回配置的 unauthorized 错误，404 返回包含原始模型名的“不存在或不可用”错误。状态覆盖在响应 JSON 解码之后执行，但即便 404 body 不是合法 JSON，仍由状态闭包决定 404 专用错误。
- 其他非 200 状态从 `error` 字段取消息；解析失败或字段类型错误时退回 HTTP status text。日志和外显消息都经共享脱敏逻辑处理，显式密钥、Bearer token 和常见凭据字段不会原样泄露。
- 成功 body 必须是可解码的二维数值数组，且外层长度必须与请求文本数相同。JSON 语法、数组形状、数值类型、`f32` 溢出或数量不匹配都会返回错误。
- 响应读取受 `max_response_bytes` 限制；负值、超限、传输失败和 runtime 建立失败均由共享基座返回结构化错误。仅 HTTP 200 被视为成功。

## 并发与资源生命周期

构造参数和 `APIKeyProviderConfig` 的闭包要求 `Send + Sync`，`reqwest::Client` 可共享，因此 `HuggingFaceEmbedder` 可被包装成 `Arc<dyn Embedder>` 供 `EmbedFn` worker 使用。本文件没有内部锁、线程、缓存或可变全局量；配置闭包自身若访问共享状态，锁与一致性由闭包提供者负责。Domain 的密钥闭包使用 `RwLock` 读锁，锁中毒会按其接线中的 `expect` panic。

每次网络调用由共享基座建立 current-thread Tokio runtime，并在同步 trait 方法内 `block_on`。请求 future 与每 5 ms 检查一次的取消 future 被共同轮询；取消会中止等待，并优先返回 `ProviderContext` 提供的原始 cause。`create_embeddings` 只携带 `AtomicBool`，因此取消信息退化为 `request canceled`；直接调用 `create_embeddings_with_context` 可以保留 `Arc` 指向的原始 cause。

正常 Domain 调度经过 `EmbedFn::run_batch`：它在独立标准线程中按 `max_batch_size` 分块调用 provider，并用 batch 级 `AtomicBool` 取消没有等待者的批次。由于 `Embedder::create_embeddings_with_values` 的默认实现转调本类型的字符串型 `create_embeddings`，常规批处理路径仍保留取消动作，但不会携带任意调用者的结构化 cause。HTTP response 和临时 runtime 在单次调用结束时释放；可复用 client 随 provider 实例释放。provider 自身没有显式 `Drop`。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/inference/embedding/huggingface/huggingface.go` 与同目录 `protocol.go`。Rust 对应关系如下：

- Go `DefaultAPIBaseURL`、`Embedder`、`NewHuggingFaceEmbedder` 分别对应 Rust `DEFAULT_BASE_URL`、`HuggingFaceEmbedder`、`with_config`；Rust 另有面向 Domain 系统变量接线的便利构造器 `new`。
- Go `featureExtractionEndpoint` 对应 Rust `endpoint`；二者都会 trim 基础 URL、选择相同默认值、逐段转义模型路径，并保留基础 URL query。Rust 通过 `ProviderEndpoint`/`raw_http` 额外对抗 `reqwest::Url` 对完整点段的归一化。
- Go `decodeErrorMessage` 的语义内联为传给共享调用器的 `base::string_field(&value["error"])`；Go `decodeEmbeddings` 对应 Rust 私有函数 `decode_embeddings`。
- Go `CreateEmbeddings` 对应 Rust 两个 trait 方法。空输入、空模型、动态 key、固定 `inputs` 覆盖 options、Bearer header、最大响应体、密钥脱敏、401/404 映射与期望数量检查均保持一致。
- Rust 的共享 `ProviderError`/`ProviderContext` 明确保留可检查的传输或调用者 cause；兼容字符串入口再通过 `to_string()` 降级。这是 Rust 接口形态上的补充，不改变普通错误文案契约。

`pkg/inference/embedding/huggingface/huggingface_test.go` 验证成功请求、Bearer header、401、404、503 JSON 错误、缺失/自定义错误、数量不匹配、options 合并、端点转义以及 provider 通用合约。`pkg/inference/huggingface_test.rs` 对应覆盖这些语义，并增加真实请求路径中 `%2E%2E` 保真和结构化调用者取消 cause 的验证。

## 扩展指南

- 增加 HuggingFace 专用请求字段时，应在 `create_embeddings_with_context` 构造固定字段的位置决定它是否允许被 `Options` 覆盖；安全关键或由参数决定的字段应像 `inputs` 一样后写。同步扩展 `pkg/inference/huggingface_test.rs` 的 payload 断言，并核对 Go `huggingface.go`/测试是否需要同语义变更。
- 修改 endpoint 路由或模型标识规则时，应集中改 `endpoint`，不要绕过 `escape_url_path_segment` 或 `ProviderEndpoint::with_path`。必须覆盖空格、`?`、`.`、`..`、多段模型名、基础路径和 query；否则可能出现路径穿越式归一化或请求到错误模型。
- 增加专用状态码时，在传给 `execute_json_embedding_call` 的 `status_error` 闭包中扩展，并同时测试合法 JSON、非法 JSON和包含密钥的 body。通用状态处理、日志和脱敏仍应留在 `base.rs`。
- 改变响应形状时，优先局部修改 `decode_embeddings`；若共享二维数值规则也变化，则修改 `base::decode_float_rows` 并同步其独立测试。必须继续验证输入/输出数量不变量，除非统一 `Embedder` 合约也被明确修改。
- 需要不同超时、代理或 transport 时，当前公开 Rust 配置只暴露 API key、base URL、错误和响应体大小；应先评估扩展共享 `APIKeyProviderConfig`/HTTP client 工厂，而不是在本文件复制请求循环。
- 并发或取消语义变化需要同步检查 `pkg/inference/embed_fn.rs` 的 `run_batch`、本类型的两个 trait 入口和 `pkg/inference/huggingface_test.rs`。Rust 单元测试继续放在独立的 `huggingface_test.rs`，不要嵌入生产源文件。
- 兼容性风险主要是 URL 字节级形式、错误文案、固定字段优先级及状态码映射；性能风险主要是每次调用创建 Tokio runtime，以及超大批次/响应的内存占用。任何优化都应保留响应上限、取消检查和 secret 脱敏。

## 验证依据

已核对的直接材料：

- `pkg/inference/huggingface.rs`：完整 134 行源文件，确认常量、结构体、构造器、端点、trait 实现和解码器。
- `pkg/inference/Cargo.toml` 与 `pkg/inference/lib.rs`：确认 crate 名、模块公开面及 `reqwest`、`serde_json`、Tokio/HTTP/TLS/logging 依赖。
- `pkg/inference/embed_fn.rs`：确认 `Embedder` 合约、provider 注册/查找、合批 worker、取消、结果数量复核、缓存和资源关闭边界。
- `pkg/inference/base.rs`：确认配置默认值、URL/segment 规则、`ProviderEndpoint`、Bearer header、HTTP/取消生命周期、响应上限、脱敏、状态处理和浮点数组解码。
- `pkg/domain/domain.rs`：确认生产注册名、构造调用和全局 API key 变量来源。
- `pkg/inference/huggingface_test.rs`：确认真实路径转义、payload 合并、成功值、共享 provider 合约、取消 cause、401/404/503、数量检查和默认 endpoint。
- `pkg/inference/embedding/huggingface/huggingface.go`、`protocol.go`、`huggingface_test.go`：确认 Go 原实现的数据模型、请求/错误协议和回归边界。

RustCodeGraph 证据：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/inference` 确认本 crate 的 Rust/Go 对照文件集合；`node --file` 读取了目标源、Rust 测试、共享 `embed_fn.rs`/`base.rs`、Domain 接线和 Go 对照文件；`query HuggingFaceEmbedder`、`query decode_embeddings` 识别目标结构和私有解码函数。对 `new`、`endpoint`、`create_embeddings_with_context` 等全仓库重名符号，`explore`/`callers` 结果存在消歧不足，故生产构造点与方法调用以精确 Rust 文本引用和源码显式调用为补充证据，没有据此推断额外调用者。

本任务是纯文档分析，按计划未运行 Cargo 或代码测试。结构验收使用任务指定命令，要求本文恰好具有上述 11 个固定二级章节；人工复核重点是本文能回答该文件为何存在、请求如何运行、错误/取消/资源边界以及安全扩展时应修改和同步验证的位置。
