# `pkg/inference/tidbcloud.rs`

## 文件定位

本文件是 `astersql-inference` crate 中 TiDB Cloud Free embedding 服务的 Rust provider 适配器。crate 由 [`Cargo.toml`](Cargo.toml) 定义，[`lib.rs`](lib.rs) 通过 `pub mod tidbcloud` 暴露本模块，并通过 `pub use embed_fn::{EmbedFn, Embedder, Options}` 提供它实现的统一接口。

应用侧的直接接线位于 `pkg/domain/domain.rs`：当 `hosted_embedding_enabled(...)` 为真时，Domain 构造 `TiDBCloudFreeEmbedder`，以 `tidbcloud_free` 名称注册到 `EmbedFn`。三个配置 getter 分别从集群配置计算 billing ID、读取 API key 文件、读取 hosted embedding API endpoint。因此本文件处于“SQL embedding 调度/批处理”与“TiDB Cloud HTTP API”之间，不负责决定何时启用 provider，也不负责上层缓存和批次聚合。

## 核心职责

- `TiDBCloudConfig` 保存延迟求值的 billing ID、API key、base URL，以及响应体大小上限；getter 允许调用时取得最新配置，而非构造时固化字符串。
- `TiDBCloudFreeEmbedder` 复用一个 `reqwest::Client`，把统一 `Embedder` 请求转换为 TiDB Cloud 的 JSON POST。
- `endpoint` 校验 base URL，选择默认 billing ID，安全转义 billing ID，并在保留原 query 的同时拼接 `/api/v1/inference/embeddings/<billing-id>`。
- `create_embeddings_with_context` 校验输入、合并固定请求字段与扩展 options、生成可选 Bearer 鉴权头，并把 HTTP、取消、响应限长、错误脱敏等共同工作委托给 `base::execute_json_embedding_call`。
- `decode_embeddings` 校验成功响应结构与数量，再把每个 Base64 编码的小端 `float32` 字节串解码成 `Vec<f32>`。

## 主要符号

- `pub struct TiDBCloudConfig`：配置载体。`billing_id`、`api_key`、`base_url` 均为 `Option<Arc<dyn Fn() -> String + Send + Sync>>`；`max_response_bytes: i64` 控制成功和失败响应的最大读取量。`Default` 会留下空 getter 和零上限，后者由构造过程补默认值。
- `pub struct TiDBCloudFreeEmbedder`：provider 实例。`client` 为 crate 内可见，便于独立测试替换或观察 HTTP 行为；`cfg` 为私有且构造后不在本文件内修改。
- `TiDBCloudFreeEmbedder::new(...) -> Self`：面向正常接线的公开构造器，把三个必备 getter 包装为 `Arc` 后转交 `with_config`。注意 getter 可以返回空串；是否允许为空由具体字段语义决定。
- `TiDBCloudFreeEmbedder::with_config(TiDBCloudConfig) -> Self`：面向完整配置和测试的公开构造器。非正的 `max_response_bytes` 被规范化为 `base::DEFAULT_MAX_RESPONSE_BYTES`（当前为 32 MiB），同时通过 `base::http_client` 建立默认 30 秒超时的客户端。
- `TiDBCloudFreeEmbedder::endpoint(&self) -> Result<base::ProviderEndpoint, ProviderError>`：crate 内端点生成器。base URL 为空时报错；billing ID 为空时使用 `default_billing_id`；路径段由 `base::escape_url_path_segment` 转义。
- `impl Embedder for TiDBCloudFreeEmbedder`：`create_embeddings` 是旧式 `AtomicBool` 边界，构造 `ProviderContext` 后委托给带上下文版本；`create_embeddings_with_context` 是实际 provider 流程并保留可检查的错误 cause。
- `fn decode_embeddings(body, expected)`：私有成功响应解码器，要求顶层为 JSON 对象、`embeddings` 为数组且数量与输入文本数相同。

本文件没有模块级常量、独立 trait、条件编译项或 `unsafe` 代码。

## 执行流程

1. `pkg/domain/domain.rs` 在 hosted embedding 功能开启时创建实例并注册为 `tidbcloud_free`。上层 `EmbedFn` 按 provider、model 和 options 聚合请求；其 `run_batch` 最终经 `Embedder::create_embeddings_with_values` 落到本实现。
2. `create_embeddings` 把 `&AtomicBool` 包装成 `ProviderContext`；若调用方已经提供带 cause 的上下文，则可直接进入 `create_embeddings_with_context`。
3. 空 `texts` 立即返回空向量，不解析配置、不发 HTTP；非空文本配空 `model` 则返回 `model name is required`。
4. 每次调用执行 API key getter。空 key 是合法值，`provider_auth_headers(..., false)` 因而不会添加 `Authorization`；非空 key 生成 `Bearer <key>`。
5. 固定字段 `model`、`texts` 与 `opts` 通过 `base::json_fields_with_options` 合并。实现先克隆 options 再写入固定字段，所以 options 中同名的 `model` 或 `texts` 不能覆盖真实调用参数，其他扩展字段会保留。
6. `endpoint` 每次执行 base URL 与 billing getter。base URL 经 `base::parse_http_url` 去除首尾空白并限制为带 host 的绝对 HTTP(S) URL；billing ID 为空时回退到 `default_billing_id`，否则作为单个 URL path segment 转义。原 base path 末尾斜杠被去除，原 query 被保留。
7. `base::execute_json_embedding_call` 发起 JSON POST，检查调用前/调用中的取消，限制响应体大小，只把 HTTP 200 当作成功，并对日志与最终错误中的 API key 做脱敏。非 200 响应从 JSON `error` 字段提取服务端消息。
8. HTTP 200 时调用 `decode_embeddings`。它逐项把 TiDB Cloud 的 Base64 字符串包装成 OpenAI indexed wire shape，复用 `openai::decode_indexed_base64_embeddings` 完成 Base64、字节长度和小端 `float32` 解码，最后保持输入顺序返回。

## 数据与状态

实例持有两类长期状态：可复用的 `reqwest::Client` 和不可变的 `TiDBCloudConfig`。配置中的字符串不缓存；闭包在每次请求时调用，因此 Domain 的 base URL、billing ID 和密钥来源可以在实例生命周期内变化。闭包由 `Arc` 持有且要求 `Send + Sync`，实例又满足 `Embedder: Send + Sync`，可由 `EmbedFn` 的工作线程共享。

请求数据使用 `Options = BTreeMap<String, serde_json::Value>`。固定 `model` 与 `texts` 拥有覆盖优先级，其余 options 原样进入 JSON。成功结果为与 `texts` 等长的 `Vec<Vec<f32>>`；每个 embedding 从 Base64 解码后的字节按 4 字节小端块解释，空字节串和非 4 倍数长度均被拒绝。代码不在本地维护配额、重试状态、连接池之外的缓存或事务状态。

## 依赖与调用关系

上游直接证据：

- `pkg/inference/lib.rs` 声明模块和测试模块。
- `pkg/domain/domain.rs` 的 hosted provider 注册代码构造 `TiDBCloudFreeEmbedder`，并将其放入 `EmbedFn`。
- `pkg/inference/embed_fn.rs` 定义 `Embedder` trait；`run_batch` 在独立工作线程中按最大批次大小调用 provider，并再次检查返回数量。
- RustCodeGraph 对目标符号的查询确认 `create_embeddings_with_context` 引用本文件的 `endpoint` 与 `decode_embeddings`，并调用共享的 `json_fields_with_options`、`provider_auth_headers`、`execute_json_embedding_call` 和 `string_field`。

下游直接依赖：

- `pkg/inference/base.rs`：HTTP client、URL 校验与转义、`ProviderEndpoint`、鉴权头、取消语义、响应读取上限、错误日志与脱敏。
- `pkg/inference/openai.rs::decode_indexed_base64_embeddings`：Base64/字节数组到小端 `float32` 的严格解码；本文件只负责把 TiDB Cloud 的每一项适配成该解码器所需的单元素 indexed 结构。
- `serde_json`：请求/响应 JSON；`reqwest`：HTTP client；`std::sync::Arc` 与 `AtomicBool`：共享 getter 和兼容取消接口。上述 crate 依赖均在 `pkg/inference/Cargo.toml` 声明；该 manifest 没有为本模块设置 feature gate。

Rust 生产代码中除 `pkg/domain/domain.rs` 的注册点外未检出其他 `TiDBCloudFreeEmbedder` 构造点；其他直接引用位于独立测试或模块声明中。

## 错误处理与边界

- 空文本是无副作用成功；空 model 仅在有文本时失败。base URL 缺失、相对 URL、无 host URL、非 HTTP(S) scheme 和畸形百分号转义均拒绝。
- 空 billing ID 使用 `default_billing_id`。billing ID 被视为一个 path segment，`/`、`?` 和完整的 `.`/`..` 段被转义，避免改变路径层级、query 或触发 URL 规范化。`ProviderEndpoint` 必要时保留 escaped raw path，实际 HTTP 请求不会把 `%2E%2E` 还原成路径跳转。
- API key 可选；空 key 不发 Authorization。非空但不能构造合法 HTTP header 的 key 会形成带 cause 的脱敏 provider 错误。
- response 上限在 `with_config` 中将非正值归一为默认值，因此通过本配置无法请求“零字节上限”；共享执行器仍防御负数。超限、transport、读取和 runtime 构建错误向上传播，敏感 key 在消息和日志中被替换。
- 非 200 响应尝试读取 JSON `error` 字符串；字段类型错误或无法解析时退回 HTTP 状态文本。目标 provider 没有自定义状态码覆盖，400、403 等都走共享格式。
- 成功响应必须是 JSON 对象。缺失或 `null` 的 `embeddings` 按空数组处理，随后通常因数量不匹配失败；非数组、数量不符、非字符串/非法 Base64、空解码结果、非 4 字节倍数均失败，并在逐项错误中把合成的 index 0 替换为真实项目下标。
- 本文件只校验 embedding 数量和二进制表示，不强制所有向量维度一致，也不拒绝字节可表示的 NaN/Infinity；若服务契约需要这些约束，应在协议或共享解码层明确增加并补测试。

## 并发与资源生命周期

`TiDBCloudFreeEmbedder` 自身不创建线程、锁、channel 或后台任务。`reqwest::Client` 可安全共享并复用连接；配置 getter 由 `Arc<dyn Fn + Send + Sync>` 共享，getter 自身若访问外部锁或文件，其同步和失败策略由注入方负责。Domain 的 API key getter会在调用时读取文件，读取失败目前转为空 key。

真正的并发由 `pkg/inference/embed_fn.rs` 管理：批次 worker 在线程中调用 provider，多位调用者可共享一次 in-flight 结果；当所有等待者取消或 `EmbedFn::close` 执行时，批次的 `AtomicBool` 被置位。共享 HTTP 执行器在 current-thread Tokio runtime 中执行请求，并以约 5 ms 的轮询 future 观察 `ProviderContext` 的取消 cause；取消与请求完成竞争时优先返回已观察到的 cause。每次调用创建的 runtime、payload、header 和响应缓冲区均在调用返回时释放，client 与配置则随 embedder/Domain 生命周期释放。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/inference/embedding/tidbcloud/tidbcloud_free.go` 与同目录 `protocol.go`。Rust 的 `TiDBCloudFreeEmbedder`/`TiDBCloudConfig` 分别对应 Go 的 `Embedder`/`EmbedderConfig`；`endpoint` 对应 `embeddingsEndpoint`；`decode_embeddings` 对应 `decodeEmbeddings`；trait 实现对应 `CreateEmbeddings`。两版共同保持以下语义：默认响应上限、空文本短路、空 model 报错、base URL 必配、默认 billing ID、billing path 转义、可匿名请求、固定字段覆盖 options、错误体 `error` 字段、返回数量检查和 Base64 小端 float32 解码。

实现形态存在局部差异：Go 构造器接收一个 config，Rust 的常用 `new` 接收三个 getter，并另提供 `with_config`；Go 用 `Response`/`ErrorResponse` 强类型协议结构，Rust 直接使用 `serde_json::Value`；Go 逐项调用 `base.DecodeFloat32ArrayBytes`，Rust 通过构造单元素 OpenAI 响应复用共享解码器；Go 直接使用调用方 `context.Context`，Rust 同时保留旧 `AtomicBool` 入口和带 cause 的 `ProviderContext`。这些差异没有改变目前测试覆盖的 wire contract。

Go 的应用入口 `pkg/inference/sqlembed.go::NewEmbedFn` 注册 `tidbcloud_free`；Rust 的对应接线已迁移到 `pkg/domain/domain.rs`。Go 测试 `pkg/inference/embedding/tidbcloud/tidbcloud_free_test.go` 与 Rust 测试 `pkg/inference/tidbcloud_test.rs` 分别覆盖成功请求、options、端点、错误、取消 cause、响应上限/脱敏、响应数量和无效字节。Rust 测试还明确覆盖实际请求中的 `%2E%2E` 保留，以及 null/非法 Base64 等协议边界。

## 扩展指南

- 增加配置项时，优先扩展 `TiDBCloudConfig`，并同步 `new`/`with_config` 的默认规则以及 `pkg/domain/domain.rs` 的注入点；不要在请求之间缓存应动态读取的 getter 结果。
- 调整 URL 路径时修改 `endpoint`，继续使用 `parse_http_url`、`escape_url_path_segment` 和 `ProviderEndpoint::with_path`，避免直接字符串拼接破坏 query 或 dot-segment 安全。同步更新 Rust 的 `cloud_endpoints_validate_configuration_and_preserve_escaped_billing_queries`、`billing_dot_segments_remain_escaped_in_the_actual_http_request`，以及 Go 的 `TestTiDBCloudFreeEmbedderEndpoint`。
- 增加请求字段时在 `create_embeddings_with_context` 的固定 `fields` 或 options 合并点接入，并明确调用参数与 options 的覆盖优先级。同步更新 Rust `cloud_protocol_covers_default_billing_fixed_options_errors_and_byte_validation` 和 Go `TestTiDBCloudFreeEmbedder_WithOptions`。
- 协议响应变化应集中修改 `decode_embeddings`；若不再是 Base64 小端 float32，不应继续伪装成 OpenAI indexed 结构，而应引入语义明确的共享解码函数。数量、空值、非法编码、字节长度和多项目错误下标都应在独立 `pkg/inference/tidbcloud_test.rs` 中回归，测试逻辑不要内嵌到生产文件。
- 修改鉴权、错误或取消语义时复用 `base.rs` 的共享设施，并运行 provider contract 覆盖；同时检查 Go 版本是否需要保持一致。兼容风险主要是 wire path/header/JSON 变化，正确性风险主要是输入输出错位和错误 cause 丢失，性能风险主要是响应上限放宽、重复读取动态配置，以及每请求同步 runtime/HTTP 调用的成本。

## 验证依据

- Rust 源与边界：`pkg/inference/tidbcloud.rs`、`pkg/inference/lib.rs`、`pkg/inference/Cargo.toml`、`pkg/inference/embed_fn.rs`、`pkg/inference/base.rs`、`pkg/inference/openai.rs`、`pkg/domain/domain.rs`。
- Rust 独立测试：[`tidbcloud_test.rs`](tidbcloud_test.rs)，覆盖 billing path、可选 key、固定字段、默认 billing、服务错误、响应体上限、transport/cancellation cause、脱敏和无效响应。
- Go 对照：[`embedding/tidbcloud/tidbcloud_free.go`](embedding/tidbcloud/tidbcloud_free.go)、[`embedding/tidbcloud/protocol.go`](embedding/tidbcloud/protocol.go)、[`embedding/tidbcloud/tidbcloud_free_test.go`](embedding/tidbcloud/tidbcloud_free_test.go)、[`sqlembed.go`](sqlembed.go)。
- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/inference/tidbcloud.rs` 识别目标文件及 14 个符号。对 `TiDBCloudFreeEmbedder`、`decode_embeddings`、`create_embeddings_with_context`、`NewTiDBCloudFreeEmbedder` 和 `NewEmbedFn` 的 `explore/query/node/callers/callees` 查询确认了上述注册边、目标文件内部调用边和 Go 对照调用边；同名符号查询出现的跨模块候选均以文件路径消歧。
- 人工复核结论：本文件存在的原因是把统一 embedding provider 契约适配到 TiDB Cloud Free wire protocol；安全扩展必须同时维护 endpoint 转义、固定字段优先级、可选鉴权、数量对应、响应边界和独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的精确正则检查本文恰含 11 个固定二级章节，并检查变更范围只新增本文、删除完成后的编号任务文件，不修改 Rust、Go、Cargo 或只读 `plan.md`。
