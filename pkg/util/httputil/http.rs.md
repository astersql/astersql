# `pkg/util/httputil/http.rs`

## 文件定位

本文件是 `astersql-util-httputil` crate 的 HTTP 客户端辅助实现，源文件为 [`http.rs`](http.rs)。crate 入口 [`lib.rs`](lib.rs) 声明 `pub mod http` 并通过 `pub use http::*` 再导出这里的公开 API；根工作区 `Cargo.toml` 将该 crate 注册为成员和 `facade_util_httputil` 依赖，随后 `pkg/lib.rs` 又在 `util::httputil` 下再导出。因此它位于通用工具层，提供阻塞式 HTTP GET 能力，而不属于 SQL 规划、执行或存储主链。

当前 Rust 仓库中，通过符号限定搜索能确认的直接调用来自同目录独立测试 [`http_test.rs`](http_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；未发现生产 Rust 文件调用这些 API。这说明 crate 已接入工作区和总门面，但不能据此宣称业务主链已经采用它。

## 核心职责

- `NewClient` 统一构造 `reqwest::blocking::Client`，把请求总超时和连接池空闲超时都设为 30 秒；调用方也可传入一个已经配置证书、身份或协议策略的 builder。
- `GetJSON` 发起 GET，在状态严格等于 200 时从响应流反序列化第一个 JSON 值。
- `GetText` 发起 GET，在状态严格等于 200 时读取完整响应体，并以有损 UTF-8 规则返回字符串。
- 私有函数 `doGet` 汇总发送请求和校验状态的共同逻辑；`HttpUtilError` 区分传输/客户端构建、JSON 解码和非 200 响应三类错误。

该文件只提供客户端侧同步辅助函数，不实现重试、鉴权头、请求体、异步执行、状态码范围策略或业务级响应校验。

## 主要符号

- `DEFAULT_TIMEOUT: Duration`：模块私有常量，固定为 30 秒，同时传给 `ClientBuilder::timeout` 与 `ClientBuilder::pool_idle_timeout`。
- `HttpUtilError`：公开错误枚举。
  - `Request(reqwest::Error)` 保存客户端构建、发送和读取响应时的 `reqwest` 错误。
  - `Json(serde_json::Error)` 保存 JSON 反序列化错误。
  - `Status { url, body }` 保存非 200 请求的原 URL 和已解码文本；它不保存实际状态码。
- `impl Display for HttpUtilError`：前两类错误沿用内部错误文本；状态错误格式固定为 `get {url} http status code != 200, message {body}`。
- `impl Error for HttpUtilError`：`Request` 和 `Json` 暴露底层 `source`，`Status` 没有底层错误源。
- `impl From<reqwest::Error>` / `impl From<serde_json::Error>`：使 `?` 能把下游错误转换为对应枚举分支。
- `TlsConfig = ClientBuilder`：公开类型别名。名称用于对齐 Go 概念，但实际代表完整的 `reqwest` client builder，而不是仅 TLS 参数对象。
- `NewClient(Option<TlsConfig>) -> Result<Client, HttpUtilError>`：公开构造函数；消费传入 builder，或从 `Client::builder()` 开始配置。
- `GetJSON<T>(&Client, &str) -> Result<T, HttpUtilError>`：公开泛型函数，要求 `T: DeserializeOwned`，直接返回拥有所有权的解码值。
- `GetText(&Client, &str) -> Result<String, HttpUtilError>`：公开文本读取函数。
- `doGet(&Client, &str) -> Result<Response, HttpUtilError>`：私有共享入口，只允许 `StatusCode::OK`。

文件使用 `#![allow(non_snake_case)]` 保留 `NewClient`、`GetJSON`、`GetText` 等 Go 风格 API 名称；没有条件编译项。

## 执行流程

1. 构造客户端时，`NewClient` 取得调用方传入的 `ClientBuilder`，没有传入则调用 `Client::builder()`。
2. 它覆盖 builder 上的请求总超时和连接池空闲超时为 `DEFAULT_TIMEOUT`，再调用 `build`。构建失败经 `From<reqwest::Error>` 变为 `HttpUtilError::Request`。
3. `GetJSON` 或 `GetText` 把借用的 client 与 URL 交给 `doGet`。
4. `doGet` 通过 `client.get(url).send()` 同步发送 GET。URL 解析、连接、TLS、超时等失败作为 `Request` 返回。
5. 如果响应状态不是精确的 200，`doGet` 消费完整响应体，以有损 UTF-8 转成文本并返回 `Status { url, body }`；所有其他 2xx 状态也走错误分支。
6. 状态为 200 时，`doGet` 把仍持有响应体的 `Response` 返回调用者。
7. `GetJSON` 用 `serde_json::Deserializer::from_reader` 流式解码第一个 JSON 值。它不额外检查首个值之后是否还有非空数据。
8. `GetText` 调用 `Response::bytes` 读取完整 body，再以 `String::from_utf8_lossy` 生成拥有所有权的字符串。
9. 成功、错误或提前返回时，局部 `Response` 被析构，底层响应资源随之释放或交回连接池。

## 数据与状态

本文件没有可变全局状态。唯一模块级数据 `DEFAULT_TIMEOUT` 是编译期常量。`Client` 由调用者拥有并以共享引用传入，连接池和 TLS 状态封装在 `reqwest::blocking::Client` 内部，辅助函数本身不缓存响应或跨调用保留业务数据。

`GetJSON<T>` 产生一个新的 `T`，不修改调用方已有对象；这与 Go 版把结果写入 `any` 指针不同。`GetText` 总是分配完整 body 对应的字节缓冲及结果 `String`。非 200 分支也会完整读取 body 并把 URL/body 复制进错误，因此错误响应大小直接影响内存占用。

## 依赖与调用关系

直接依赖由 [`Cargo.toml`](Cargo.toml) 给出：

- `reqwest 0.12` 关闭默认 feature，仅启用 `blocking`、`json`、`rustls-tls`；本文件实际使用其阻塞客户端、builder、response、状态码与错误类型。
- `serde` 提供 `DeserializeOwned` 约束和 `T::deserialize`。
- `serde_json` 提供流式 deserializer 及 JSON 错误。
- `tiny_http` 仅是 dev-dependency，由独立测试启动本地服务，不进入生产实现。

模块内调用边为 `GetJSON -> doGet`、`GetText -> doGet`；`NewClient` 下游是 `Client::builder`/`ClientBuilder::{timeout,pool_idle_timeout,build}`，`doGet` 下游是 reqwest 的 request builder 和 `Response::bytes`。RustCodeGraph 能索引本文件 10 个符号并报告文件级引用，但对带文件限定的 `callers/callees` 查询未给出函数级边；上述边由已索引源码和独立测试补证。

工作区接线是 `pkg/util/httputil/lib.rs -> http::*`，再由 `pkg/lib.rs::util::httputil` 通过 `facade_util_httputil::*` 暴露。当前可确认消费者是测试：`http_test.rs` 直接覆盖 `GetJSON`/`GetText`，`migration_aster_unit_test.rs` 覆盖三项公开 API；生产 Rust 消费者未验证到。

## 错误处理与边界

- 只有状态码 200 被接受；201、204、重定向最终未落到 200 的情况都会被视为状态错误。reqwest 默认重定向策略仍由 builder/client 决定。
- URL 构造、DNS、连接、TLS、超时、发送和 body 读取错误统一归入 `Request`，调用方可通过 `Error::source` 查看原始 `reqwest::Error`。
- 200 响应的空 body、类型不匹配或非法 JSON 归入 `Json`。解析首个 JSON 值成功后，尾随的第二个 JSON 值不会在这里被显式拒绝。
- 非 200 分支若读取 body 本身失败，返回 `Request`，而不是 `Status`；此时 URL 和 HTTP 状态信息不会保留在最终错误中。
- `Status` 的展示文本虽说明“status code != 200”，却不包含实际状态码，只包含 URL 和 body。
- 状态错误和 `GetText` 都使用有损 UTF-8：非法字节会替换为 Unicode replacement character。该行为不等同于 Go `string([]byte)` 对原始字节的逐字节保留。
- 响应体没有本模块自己的大小上限；大 body 会被 `GetText` 或非 200 错误分支整体载入内存。JSON 成功路径则从响应流解码。
- 本 API 不接受取消 token 或请求上下文；取消只能依赖 client 超时或外部执行环境，不能逐请求传播 Go `context.Context`。

[`http_test.rs`](http_test.rs) 验证未监听端口产生错误、200 JSON 成功、204 被拒绝、文本路径 `/test` 和完整文本读取。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步验证 204 的空 body 错误文案、503 body 写入错误文案，以及自定义 builder 可被消费。现有测试没有覆盖非法 UTF-8、大响应、JSON 尾随值、超时触发和 client 构建失败。

## 并发与资源生命周期

所有网络调用都是 `reqwest::blocking` 同步调用，会占用当前线程直至完成或超时；本文件不创建线程、任务、锁、通道或运行时。`Client` 以共享引用传入，可按 reqwest 的线程安全语义由上层复用其连接池，但本模块不管理并发度。

`NewClient` 消费 `ClientBuilder`，构建后的 builder 不能复用。成功响应由 `GetJSON`/`GetText` 在函数作用域内持有：JSON 解码或完整读取完成后自动析构；非 200 响应在 `doGet` 中读取并析构。测试中的线程只属于 `tiny_http` 服务夹具，不是生产实现的一部分。

30 秒 `timeout` 覆盖从连接到响应体读取的请求生命周期；`pool_idle_timeout` 控制池内空闲连接保留时间。两项都由 `NewClient` 最后设置，因此调用方 builder 先前配置的这两个值会被覆盖。

## 与 Go 版本的对应关系

Go 对照文件是 [`http.go`](http.go)，测试对照是 [`http_test.go`](http_test.go)。共同语义包括：默认请求超时 30 秒、GET 请求、仅接受 200、非 200 时读取 body 并采用相同主干错误文案、JSON 只解码首个值，以及返回前释放响应体。

需要保留的差异如下：

- Go `GetJSON` 接受 `context.Context` 并写入调用方提供的目标；Rust 版没有逐请求 context，而是以泛型返回新值。
- Go `NewClient` 返回 `*http.Client` 且无错误；Rust 构建可能失败，所以返回 `Result<Client, HttpUtilError>`。
- Go 参数是可选 `*tls.Config`，并克隆默认 transport 后安装 TLS 配置；Rust `TlsConfig` 实际是整个 `ClientBuilder`，可携带的不只是 TLS 配置，并在传入后被消费。
- Rust 无论是否传 builder 都将池空闲超时设为 30 秒；Go 代码仅在 `tlsConf != nil`、创建自定义 transport 时显式设置 30 秒。不能把两者描述为所有路径完全一致。
- Go 使用 `http.NewRequestWithContext`，因此无效 URL 可在执行前形成请求构造错误；Rust 通过 reqwest builder/send 报错，错误分类统一为 `Request`。
- Go `GetText` 的 `string(data)` 保留任意字节序列；Rust 使用有损 UTF-8 转换，非法序列会被替换。
- 两边对非 200 body 都整体读取；两边的流式 JSON decode 都只要求第一个值可解码。

## 扩展指南

- 若要增加 POST、header 或可接受状态范围，应优先抽象 `doGet` 附近的请求/状态策略，避免 `GetJSON` 与 `GetText` 复制发送和错误文案逻辑；同时明确是否必须继续保持 Go API 语义。
- 若要支持逐请求取消或 deadline，需调整公开函数签名和下游请求构造；这是与 Go `context.Context` 对齐的兼容性变化，应先寻找并迁移所有消费者。
- 若要准确诊断状态错误，可在 `HttpUtilError::Status` 增加实际状态码，但这会改变公开枚举的模式匹配接口和展示行为。
- 若要限制内存，需分别处理 `GetText`、非 200 body 和 JSON 流：前两者当前会完整缓冲，不能只修改 JSON 路径。
- 若要严格保留原始文本字节，应新增返回 `Vec<u8>` 的 API，而不是悄然改变 `GetText` 的 UTF-8 契约。
- 修改生产逻辑时，测试必须继续放在独立文件中：基础行为同步更新 [`http_test.rs`](http_test.rs)，Go 迁移契约同步更新 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并与 [`http_test.go`](http_test.go) 的意图复核。不要把 Rust 测试内嵌进 `http.rs`。
- 性能风险主要来自阻塞线程和无限制整体缓冲；兼容风险主要来自公开错误枚举、Go 风格函数名、严格 200 策略、错误文本以及 builder 覆盖顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/util/httputil` 列出 Go/Rust 实现和测试；`node --file pkg/util/httputil/http.rs --offset 1 --limit 240` 返回目标文件完整 133 行与 10 个符号。
- RustCodeGraph 符号查询确认 `GetJSON`、`GetText`、`doGet` 的 Go/Rust定义；对 `NewClient`、`GetJSON`、`GetText`、`doGet` 执行带 `--file pkg/util/httputil/http.rs` 的 `callers`/`callees`，没有函数级结果，因此没有将文件级“used by”列表误写成真实调用关系。
- 已读生产与装配文件：[`http.rs`](http.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、根 `Cargo.toml` 的 workspace/依赖条目、`pkg/lib.rs` 的 `util::httputil` 门面。
- 已读 Go 对照与独立测试：[`http.go`](http.go)、[`http_test.go`](http_test.go)、[`http_test.rs`](http_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 文本引用搜索确认当前生产 Rust 中未发现对该 crate 名、`util_httputil` 路径或这些 API 的直接消费；这一结论只覆盖当前检出的仓库源码，不代表外部 crate 没有消费者。
- 按任务要求未运行 Cargo。交付前以固定标题结构命令确认本文恰有 11 个二级章节，并人工复核文件定位、运行流程、安全扩展点及未验证范围。
