# `pkg/domain/affinity/http_client.rs`

## 文件定位

本文件是 `astersql-domain-affinity` crate 面向真实 PD 的同步 HTTP 传输实现。模块由 `pkg/domain/affinity/lib.rs` 以 `pub mod http_client` 暴露；上层在 `pkg/session/runtime/create_table_resources.rs::pd_client` 中读取存储返回的 PD endpoints 和全局 TLS 配置后构造 `HttpClient`。它一方面实现 `pkg/domain/affinity/manager.rs::PdClient`，供 `PdManager` 完成 affinity group 的创建、删除和查询，另一方面公开通用的 `HttpClient::request_json`，供同一调用方访问 placement rule、TiFlash rule 与 accelerate-schedule 等 PD HTTP API。

该文件只负责传输、协议编解码和基础端点故障切换，不负责 affinity 的幂等创建、旧 PD 兼容回退、重试或结果过滤；这些策略位于 `manager.rs::PdManager` 与 `interface.rs`。crate 边界由 `pkg/domain/affinity/Cargo.toml` 定义，当前没有 feature 条件；运行时依赖 `reqwest`（blocking/json/rustls-tls）、`serde_json`、`base64` 和 `url`。

## 核心职责

- `HttpClient::new` 校验 endpoint 列表、配置固定 30 秒的客户端默认超时、按需装入 CA 与客户端证书/私钥，并规范化 endpoint 的 scheme 和尾部斜杠。
- `HttpClient::request_json` 在 endpoints 间按配置顺序尝试请求，传播 `Context` 的取消和截止期，序列化可选 JSON 请求体，并把 HTTP/传输/响应体转换为本 crate 的 `AffinityError` 或 `serde_json::Value`。
- `HttpClient::states` 将 PD 的 `affinity_groups` JSON 对象转成 `HashMap<String, AffinityGroupState>`，强制要求每项存在无符号整数 `range_count`。
- `impl PdClient for HttpClient` 把四个领域操作映射到 `/pd/api/v2/affinity-groups`：创建、批量删除、按 ID 查询和全量查询。

## 主要符号

- `API: &str`：affinity group API 根路径，值为 `/pd/api/v2/affinity-groups`。
- `HttpClient { client, endpoints }`：持有可复用的 `reqwest::blocking::Client` 和规范化后的 PD endpoint 列表。字段私有，构造必须经过 `new`。
- `HttpClient::new(endpoints, tls) -> Result<Self, AffinityError>`：公开构造函数。`tls` 为 `(CA PEM, certificate PEM, private-key PEM)` 的借用字节切片；存在 TLS 配置时默认补 `https://`，否则补 `http://`。显式带 `://` 的 endpoint 保留原 scheme。
- `HttpClient::request_json(ctx, method, path, body) -> Result<Value, AffinityError>`：公开的同步 JSON 请求原语，也是 affinity 之外 PD API 的直接扩展入口。
- `HttpClient::states(value)`：私有响应适配器；外层 key 是结果 map 的 key，内部 `id` 缺失时回退为外层 key。
- `err(e)`：把证书、identity、client 构建或响应读取错误压平为普通 `AffinityError`。
- `impl PdClient`：实现 `create_affinity_groups`、`batch_delete_affinity_groups`、`get_affinity_groups` 和 `get_all_affinity_groups`，其接口定义在 `manager.rs::PdClient`。

## 执行流程

1. `create_table_resources.rs::pd_client` 从 `Domain` 获取 PD endpoints，并读取 `cluster_ssl_ca/cert/key`；随后调用 `HttpClient::new`。空 endpoint 立即失败。构造器先建立带 30 秒默认 timeout 的 blocking client；若提供 TLS，则解析根证书，并在 cert 或 key 任一非空时把二者拼接成 PEM identity。
2. 每次领域操作先组装协议数据，再进入 `request_json`。创建操作把每个 `AffinityGroupKeyRange` 的起止 key 用标准 Base64 编码，形成 `{"affinity_groups": {id: {"ranges": [...]}}}`，并根据 `skip` 决定是否追加 `?skip_exist_check=true`。
3. `request_json` 按 endpoint 顺序迭代。每次尝试前检查 `ctx.is_cancelled()`；如有绝对 deadline，则计算相对 timeout，deadline 已过时不发请求并返回 `context deadline exceeded`。
4. 请求使用克隆的 HTTP method 和 `{endpoint}{path}`；有 body 时由 reqwest 以 JSON 发送。网络/服务层发送错误被记为最后错误并继续尝试下一个 endpoint；一旦收到 HTTP 响应，即停止 endpoint 轮询。
5. 收到响应后先完整读取 bytes。非 2xx 响应转换为带状态码和宽松 UTF-8 响应文本的 `AffinityError::http_status`；成功且空 body 返回 `Value::Null`；非空 body 优先解析 JSON，解析失败则保留为宽松 UTF-8 `Value::String`。
6. 创建和查询路径将返回值交给 `states`。它提取 `affinity_groups` 对象和每项 `range_count`，构造领域状态。删除路径忽略成功响应值。`get_all_affinity_groups` 通过空 ID 列表复用查询实现，因此不会添加问号。

## 数据与状态

`HttpClient` 的长期状态只有不可变的 reqwest client 与 endpoint 向量；文件中没有全局可变状态。请求 body 和响应均以 `serde_json::Value` 作为通用边界，而 `PdClient` 的领域方法在该边界两侧转换 `HashMap<String, Vec<AffinityGroupKeyRange>>` 与 `HashMap<String, AffinityGroupState>`。

key range 的字节序列不按 UTF-8 或十六进制解释，而使用 `base64::STANDARD` 编码。`http_client_test.rs::normal_ddl_plan_create_table_affinity_http_wire` 以 `[0, 255]` 和 `[255, 0]` 验证 wire 值分别为 `AP8=` 和 `/wA=`。查询 ID 使用 `url::form_urlencoded::Serializer` 重复追加 `ids` 参数，因而会正确转义且保留输入顺序；空列表产生空 query。`range_count` 从 JSON `u64` 直接 `as usize` 转换，在窄于 64 位的平台可能截断，当前代码没有显式溢出检查。

## 依赖与调用关系

上游生产调用者是 `pkg/session/runtime/create_table_resources.rs`：`create_affinity` 构造该客户端并包装为 `new_pd_manager(Arc<dyn PdClient>)`；`delete_affinity` 同样通过 manager 删除；`put_bundles`、`configure_replica` 等路径直接调用 `request_json`。mock-storage 分支绕过此传输，走包级 mock/infosync 行为。

下游依赖关系为：`HttpClient::new` 调用 reqwest 的 blocking client、证书与 identity 构造；`request_json` 调用 `Context::{is_cancelled, deadline}`、reqwest request/send/bytes 和 serde_json 解析；领域方法调用 Base64、form-urlencoded serializer、`AffinityError::{new,http_status,http_service}` 与 `AffinityGroupState::new`。`manager.rs::PdManager` 消费这些错误的状态码/服务错误标记，以决定是否执行旧 PD 兼容回退，因此错误分类是跨文件契约，不能随意统一成普通错误。

RustCodeGraph 的符号查询定位到 `pkg/domain/affinity/http_client.rs::HttpClient`（第 11 行），但索引未返回 `request_json` 方法节点或 callers/callees；上述调用边由精确 `rg` 与对应源码段直接核验。

## 错误处理与边界

- 空 endpoint 是构造错误；endpoint 字符串不做 URL 完整合法性预检，实际 URL 错误在发请求时表现为服务错误并参与 endpoint 轮询。
- CA PEM、拼接后的 client identity 或 reqwest client 构建失败使用普通 `AffinityError::new`；它们不是 `http_service`，不会被 manager 当成兼容性错误。
- 取消只在每个 endpoint 尝试开始前检查；blocking `send` 期间不会再次轮询 `Context::is_cancelled`。deadline 则被转为单次请求 timeout，可限制在途请求。
- 只有发送错误会切换到下一 endpoint。收到任意 HTTP 响应后，读取 body 失败、非成功状态、无效领域结构都会立即返回，不再尝试其他 endpoint。
- 成功响应的非 JSON body 不报解析错误，而返回 `Value::String`；对于 affinity 创建/查询，这随后会被 `states` 拒绝为缺少 `affinity_groups`。通用 `request_json` 调用者则可能接收到字符串。
- `states` 要求顶层 `affinity_groups` 为对象、每项 `range_count` 为 `u64`；内部 `id` 可缺省或非字符串，此时使用 map key。它不解析 ranges 或其他字段。
- TLS identity 通过简单拼接 cert 与 key 构造，调用者必须提供 reqwest/rustls 可识别的 PEM；若 CA 存在但 cert/key 都为空，则只配置服务端证书校验，不配置客户端身份。

## 并发与资源生命周期

`HttpClient` 没有内部锁或可变字段；`reqwest::blocking::Client` 管理连接池并被各请求共享。类型通过其字段具备 manager 所要求的 `Send + Sync` 能力，生产代码将其放入 `Arc<dyn PdClient>`。每个 `request_json` 调用同步阻塞当前线程，endpoint 故障切换也是串行的；最坏耗时可包含多个 endpoint 各自的发送超时。客户端默认单次请求上限为 30 秒，有 context deadline 时该请求覆盖为剩余时长。

请求 body 在每个 endpoint 尝试中以借用的 `Value` 交给 reqwest，因此故障切换不会消耗原值。response bytes 在返回前完整读入内存，没有流式上限。文件不创建后台任务、线程、channel 或显式连接关闭逻辑；client drop 时由 reqwest 释放资源。测试中的 TCP server 线程属于 `http_client_test.rs` 的线协议夹具，不是生产生命周期的一部分。

## 与 Go 版本的对应关系

Go 同目录没有自有 `http_client.go`；`interface.go` 与 `manager.go` 直接依赖上游 `github.com/tikv/pd/client/http.Client`。Rust 的 `manager.rs::PdClient` 对齐该外部接口中本包使用的四个方法，而本文件补齐了 Rust 侧真实 HTTP 实现，因此是 Go 外部 PD client 的本地替代层，并非逐文件翻译。

协议语义保持与 Go manager 的调用预期一致：创建支持 `WithSkipExistCheck` 对应的 `skip_exist_check=true`，删除传递 `force=true`，按 ID 查询与全量查询分别映射到 `GetAffinityGroups`/`GetAllAffinityGroups`。Go `manager.go` 中的幂等回退、URI 长度阈值、旧 PD 返回全量结果后的过滤不在本文件实现，而在 Rust `manager.rs` 对齐。Rust 额外暴露 `request_json` 给 session 的其他 PD API 调用；Go 侧这些能力由既有 PD HTTP client 或其他包路径承载。

Go `context.Context` 能直接驱动底层请求取消；Rust 这里只抽象取消标记和 deadline。当前实现对预先取消及 deadline 有明确处理，但在 blocking send 期间不能响应之后发生的纯取消，这是需要保持可见的语义差异。

## 扩展指南

- 新增 affinity API 操作时，先在 `manager.rs::PdClient` 增加领域方法，再在本文件用 `request_json` 实现协议转换，并在独立的 `http_client_test.rs` 增加 method/path/body/response 的线协议断言；不要把测试内嵌进生产文件。
- 修改 endpoint 切换、timeout 或取消语义时，应重点覆盖多 endpoint、首端点发送失败、已过 deadline、在途取消与 HTTP 非 2xx。必须确认 `AffinityError::http_status/http_service` 分类仍满足 `manager.rs::should_fallback_*` 的兼容策略。
- 扩充响应字段时应集中修改 `states`，明确缺失、类型错误、超大 `range_count` 的处理，不要让通用 `request_json` 承担领域校验。
- 改动 TLS 时需同时检查仅 CA、完整 mTLS、残缺 cert/key、显式 `http://` 与 `https://` endpoint 的组合。目前 scheme 由“是否传入 TLS tuple”决定，而显式 scheme 优先；改变规则可能影响已有部署兼容性。
- 将 `request_json` 用于新 PD API 前，应确认该 API 是否允许成功的空 body 或文本 body，以及是否需要响应大小限制、重试或幂等保证。这里的 endpoint 轮询只处理发送失败，不等同于业务重试。
- 性能风险主要来自 blocking I/O、串行 endpoint 尝试、完整缓冲响应和每次领域调用的 JSON 中间值；若引入批量大响应，应先评估内存与线程占用。

## 验证依据

- 生产源码：`pkg/domain/affinity/http_client.rs`，核对 `API`、`HttpClient::{new,request_json,states}`、`err` 和 `impl PdClient for HttpClient` 的完整实现。
- crate 与模块：`pkg/domain/affinity/Cargo.toml`、`pkg/domain/affinity/lib.rs`，核对 crate 名、依赖、无 feature 条件以及公开模块接线。
- 领域契约：`pkg/domain/affinity/manager.rs`、`pkg/domain/affinity/interface.rs`，核对 `Context`、`PdClient`、`AffinityError`、状态类型、manager 回退策略及包级调用边界。
- 生产调用：`pkg/session/runtime/create_table_resources.rs`，核对 endpoint/TLS 来源、`new_pd_manager` 接线，以及 `request_json` 在 placement、TiFlash 和调度 API 中的直接复用。
- Rust 独立测试：`pkg/domain/affinity/http_client_test.rs::normal_ddl_plan_create_table_affinity_http_wire`，核对 POST create、GET by IDs、POST delete、GET all 四个请求，Base64 key 编码、`skip_exist_check`、`force` 与状态解析。该测试未覆盖 TLS、多 endpoint、错误状态、取消和 deadline。
- Go 对照：`pkg/domain/affinity/interface.go`、`pkg/domain/affinity/manager.go`、`pkg/domain/affinity/manager_test.go`，确认 Go 依赖外部 PD HTTP client，兼容回退属于 manager 而非传输层。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query HttpClient --kind struct --json` 定位到本文件第 11 行。`explore`、文件 `node` 与 `query request_json --kind method` 未返回该方法的调用图，因此调用关系以精确源码搜索补证，未将空图结果推断为“没有调用者”。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定的命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核未把测试代码放入生产源文件、未改动 Rust/Go/Cargo 或只读总计划。
