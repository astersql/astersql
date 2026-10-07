# `pkg/objstore/ossstore/retry.rs`

## 文件定位

本文件属于 `astersql-objstore-ossstore` crate，crate 根 `pkg/objstore/ossstore/lib.rs` 通过 `mod retry; pub use retry::*;` 将其公开符号再导出。它位于 OSS SDK 错误与仓库通用对象存储重试抽象之间：一方面识别 `ali_oss_rs`、`reqwest`、`std::io` 和 OSS 服务错误，另一方面实现 `s3like::StandardRetryer`，提供统一的最大尝试次数、退避和实例元数据错误识别。

当前 Rust 运行路径中，`pkg/objstore/ossstore/interface.rs` 的 `AliyunOssApi::execute` 直接构造 `OssRetryer::default()`，在每次 OSS 操作失败后调用本文件的分类和退避逻辑。文件末尾的 `new_retryer()` 能把策略包装为 `s3like::Retryer`，但仓库 Rust 调用点搜索只找到它的定义，尚未发现生产调用者；不能把 Go 版本在 Store 构造阶段的 SDK 注入方式视为 Rust 已接线事实。

## 核心职责

- `OssRetryer` 保存 OSS 重试策略参数，并以 `Default` 固化与 Go 版本一致的 20 次最大尝试、1 秒基础延迟和 32 秒退避上限。
- `StandardRetryer::IsErrorRetryable` 遍历完整 `anyhow::Error` 因果链，优先使用结构化错误类型和状态分类，再以一组已知传输错误文本兜底。
- `StandardRetryer::RetryDelay` 计算带 full jitter 的指数退避，使并发失败的客户端不会在同一时刻集中重试。
- 令牌接口返回无操作回调，因为当前 OSS 策略没有自适应令牌桶；`IsInstanceMetadataError` 单独识别阿里云 ECS 元数据地址，供通用 `s3like::Retryer` 避免在凭证链路错误上长时间重试。
- 固有方法 `OssRetryer::{IsErrorRetryable, MaxAttempts, RetryDelay, IsInstanceMetadataError}` 只是 trait 方法的直接转发，方便不显式导入 trait 的调用者使用；策略本身仍由 `s3like::StandardRetryer` 定义。

## 主要符号

- `MAX_ATTEMPTS: i32 = 20`：一次操作允许的总尝试次数，包含首次请求，因此最多发生 19 次重试。
- `ECS_META_ADDRESS: &str = "100.100.100.200"`：阿里云 ECS 实例元数据服务地址的识别片段。
- `RETRIABLE_ERROR_STRINGS`：12 个私有传输错误文本片段，包括连接复位/拒绝、连接关闭、TLS、HTTP/2 stream、broken pipe、EOF 和 CRC 不一致等兼容性兜底。
- `OssRetryer { max_attempts, base_delay, max_backoff }`：公开、可克隆的策略值对象；字段公开，调用者可构造非默认策略。
- `impl Default for OssRetryer`：产生 `20 / 1s / 32s` 的标准策略。
- `impl s3like::StandardRetryer for OssRetryer`：本文件的核心实现，包含错误分类、尝试上限、退避、令牌回调和元数据错误识别。
- `random_unit_interval() -> f64`：用新建 `RandomState` 的随机哈希键生成 53 位精度的 `[0, 1)` 小数，只供抖动计算使用。
- `is_retryable_status(StatusCode) -> bool`：将所有 `>= 500` 状态以及 401、408、429 标为可重试。
- `new_retryer() -> s3like::Retryer`：以默认 `OssRetryer` 构造通用对象存储重试包装器；当前 Rust 仓库未发现调用点。

## 执行流程

`AliyunOssApi::execute` 是已验证的主要上游。它先创建默认重试器，然后对 `attempt = 1..=MaxAttempts()` 循环：每次调用操作前检查 `storeapi::Context` 是否已取消；操作成功立即返回；失败时只有在尚未达到上限且 `IsErrorRetryable` 返回真时才计算延迟，否则原样返回错误。等待过程被拆成最多 50 ms 的 sleep，以便期间反复检查取消状态。

`IsErrorRetryable` 对 `anyhow` 因果链逐层执行以下判定，任一命中即返回 `true`：

1. `OssServiceError.code` 是 `RequestTimeTooSkewed` 或 `BadRequest`。
2. `ali_oss_rs::error::Error::StatusError` 的 HTTP 状态满足 `is_retryable_status`，或 `ApiError.response.code` 是上述两个服务码。
3. `reqwest::Error` 是 timeout、connect 错误，或携带可重试 HTTP 状态。
4. `std::io::ErrorKind` 是 `TimedOut`、`ConnectionReset`、`ConnectionRefused`、`BrokenPipe` 或 `UnexpectedEof`。
5. 若结构化判定全部未命中，再次扫描因果链各层的显示文本，匹配 `RETRIABLE_ERROR_STRINGS` 中任一片段。

`RetryDelay(attempt, error)` 不使用错误内容。它先把负数或无法转换的 attempt 当作 0，并把指数限制到 31；随后计算 `min(base_delay * 2^exponent, max_backoff)`，乘以 `[0, 1)` 随机数，返回 `[0, ceiling)` 的延迟。`Duration::saturating_mul` 和最大上限共同防止算术溢出或无限增长。在当前 `AliyunOssApi::execute` 中第一次失败传入 `attempt = 1`，默认延迟范围因此是 `[0, 2s)`。

若改用 `new_retryer()` 返回的 `s3like::Retryer`，外层还会应用通用规则：实例元数据 deadline/reset 不重试、普通 reset 和 HTTP/2 中断重试、connection refused 不重试，并把最终延迟抬高到至少 1 秒。当前直接调用 `OssRetryer` 的 `AliyunOssApi::execute` 不经过这些外层规则。

## 数据与状态

`OssRetryer` 仅持有三个不可自动变化的配置字段，没有内部计数器、缓存或共享可变状态。尝试次数由调用循环维护，错误历史也不保存在重试器中；因此克隆策略不会共享运行状态。

默认不变量是 `max_attempts = 20`、`base_delay = 1s`、`max_backoff = 32s`。字段公开意味着自定义实例可以打破通常预期，例如把最大次数设为非正数、基础延迟设为零或令基础延迟大于上限；本文件不做构造时校验，调用者必须维护合理配置。

随机抖动不持久化随机数生成器。每次调用 `random_unit_interval` 都从 `RandomState::new()` 获取带随机键的 hasher，读取 64 位结果的高 53 位并归一化；输出可能为 0，但严格小于 1，因此正常情况下延迟小于 ceiling，而不是等于 ceiling。

## 依赖与调用关系

上游已验证关系如下：

- `pkg/objstore/ossstore/lib.rs` 声明并再导出本模块。
- `pkg/objstore/ossstore/interface.rs::AliyunOssApi::execute` 调用 `OssRetryer::default`、`MaxAttempts`、`IsErrorRetryable` 和 `RetryDelay`，为底层 OSS API 请求提供可取消的同步重试循环。
- `pkg/objstore/ossstore/retry_test.rs` 验证包装在 `anyhow` 因果链中的传输错误文本仍可重试。
- `pkg/objstore/ossstore/migration_aster_unit_test.rs` 验证默认次数、抖动范围和非恒定性、ECS 地址识别、结构化 HTTP/服务错误与 IO 错误分类。

下游依赖如下：

- `anyhow::{Error, Result}` 承载异构错误链和 trait 返回错误。
- `ali_oss_rs::error::Error` 提供 OSS SDK 的结构化状态错误与 API 错误。
- `reqwest::{Error, StatusCode}` 提供 HTTP 客户端错误属性与状态码。
- `std::io::Error` 提供网络/流错误种类；`std::time::Duration` 表示配置和退避结果。
- `s3like::StandardRetryer` 与 `s3like::NewRetryer` 定义跨对象存储的策略边界和包装逻辑；`storeapi::Context` 出现在令牌接口中，但本实现不读取它。

`pkg/objstore/ossstore/Cargo.toml` 确认该 crate 直接依赖 `ali-oss-rs`（blocking、rust-tls）、`anyhow`、`reqwest`（blocking、rustls-tls），并以工作区 path 依赖连接 `s3like` 和 `storeapi`。本文件没有条件编译项。

## 错误处理与边界

分类逻辑读取完整错误链而非只读最外层错误，因此 `anyhow::Context` 添加操作说明不会遮蔽底层可重试原因。结构化类型判断先于文本兜底，避免把普通文本中的数字（例如字符串 `503`）误判为 HTTP 状态；现有测试明确验证 `anyhow!("operation 503 without structured status")` 不可重试。

HTTP 可重试集合包括所有 5xx、401 Unauthorized、408 Request Timeout 和 429 Too Many Requests。SDK API 码只特判 `RequestTimeTooSkewed` 与 `BadRequest`；`AccessDenied` 等永久错误不重试。需要注意，所有 connect 类 `reqwest::Error` 和 `std::io::ErrorKind::ConnectionRefused` 在本策略中可重试，但通用 `s3like::Retryer` 外层会把可识别的 connection-refused 文本改判为不可重试；最终行为取决于调用者使用直接策略还是包装策略。

文本匹配区分大小写，且基于各层错误的 `Display` 文本，新增 SDK 或传输栈后错误措辞变化可能造成漏判。反过来，宽泛片段如 `stream error:` 也可能把业务错误误判为瞬时错误，因此扩展文本表时必须用真实错误链回归。

`IsInstanceMetadataError` 只检查最外层 `error.to_string()` 是否包含地址，不遍历链；外层上下文若完全替换而不保留底层显示内容可能漏判。该方法参数不能为 Go 式 nil，Rust 类型系统消除了 Go 实现的 nil 分支。

## 并发与资源生命周期

`OssRetryer` 的字段均为值类型，实现 `Clone`，且因 `StandardRetryer: Send + Sync` 可安全放入跨线程包装器；本文件自身不创建线程、不持有锁、不打开连接，也没有显式关闭过程。

`GetInitialToken` 和 `GetRetryToken` 每次都返回一个新的无状态 `FnOnce` 释放回调。回调忽略传入的可选错误并总是返回 `Ok(())`，因此没有令牌配额的获取、归还或泄漏风险，但也不提供客户端侧重试限流。

实际等待发生在 `AliyunOssApi::execute` 的调用线程中，是阻塞式 sleep；50 ms 分段只改善取消响应，不改变其占用线程的事实。随机数生成没有共享状态或锁，但也不承诺可复现顺序；测试只能断言范围和基本抖动性质，不能断言具体延迟值。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/objstore/ossstore/retry.go`。两版共同保持 `maxAttempts = 20`、1 秒基础延迟、32 秒最大退避、无操作令牌回调，以及通过 `100.100.100.200` 识别 ECS 元数据错误。Go `retryer` 嵌入阿里云官方 `ossretry.Retryer`，标准错误分类与抖动由 SDK 提供；Rust 因使用不同 SDK/适配层，在本文件中显式复刻了结构化错误分类、IO 错误分类、字符串兼容表和 full-jitter 计算。

Go `NewOSSStorage` 在 `pkg/objstore/ossstore/store.go` 中调用 `oss.NewConfig().WithRetryer(newRetryer())`，因此通用 `s3like.Retryer` 被注入官方 OSS SDK。Rust `pkg/objstore/ossstore/store.rs` 不调用 `new_retryer()`；实际重试由 `AliyunOssApi::execute` 自己驱动并直接使用 `OssRetryer`。这意味着两版目标语义相近，但接线层次和外层 `s3like::Retryer` 特化规则的覆盖范围并不完全相同。

Go 的 `IsInstanceMetadataError(nil)` 明确返回 false；Rust 不接受空错误。Go SDK 的确切内部错误集合属于外部依赖实现，本分析只依据 Rust 当前显式规则和仓库测试，不声称两套第三方 SDK 的所有边缘错误都完全等价。

## 扩展指南

- 新增可重试 HTTP 状态时修改 `is_retryable_status`，并在独立测试 `pkg/objstore/ossstore/migration_aster_unit_test.rs::standard_retry_classification_matches_aliyun_sdk` 同时加入正例与相邻永久错误反例。
- 新增 OSS 服务错误码时同时审查 `OssServiceError` 和 `ali_oss_rs::Error::ApiError` 两条结构化路径，避免同一服务响应因包装类型不同而得到不同结论。
- 新增文本兼容项时修改 `RETRIABLE_ERROR_STRINGS`，在 `pkg/objstore/ossstore/retry_test.rs` 添加带 `anyhow::Context` 的真实形态回归；不要把测试放回生产源文件。
- 调整退避公式或默认参数时同步核对 Go `retry.go` 的 SDK 配置、`AliyunOssApi::execute` 传入 attempt 的起始语义，以及通用 `s3like::Retryer::RetryDelay` 的至少 1 秒下限。性能风险主要是重试风暴、过长尾延迟和同步线程占用。
- 若要让 Rust Store 全面使用 `new_retryer()`，必须先决定由 SDK、`AliyunOssApi::execute` 还是二者之一负责重试，防止双层重试使最坏请求次数相乘；同时验证元数据错误和 connection-refused 在外层包装后的分类变化。
- 若引入真正的重试配额，应在 `GetInitialToken`、`GetRetryToken` 和 `ReleaseToken` 生命周期中成套实现，并增加并发测试验证额度获取、失败归还和取消路径，而不是只修改空回调。
- 修改公开字段或 `StandardRetryer` 实现会影响 crate 外调用者，需保留 `Send + Sync`、返回 `anyhow::Result` 和最大尝试次数“包含首次”的契约。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/objstore/ossstore` 列出目标模块；`query` 精确定位 `retry.rs::OssRetryer`、`retry.rs::new_retryer`、`retry.rs::random_unit_interval` 和 `retry.rs::is_retryable_status`；`node --file pkg/objstore/ossstore/retry.rs --offset 1 --limit 260` 展示完整 205 行源文件。批量 `callers/callees` 命令在 30 秒内未返回结果，因此调用关系另以直接源码搜索核实，未把超时当作图证据。
- 生产源码：`pkg/objstore/ossstore/retry.rs`（全部符号与实现）、`pkg/objstore/ossstore/lib.rs`（模块声明和再导出）、`pkg/objstore/ossstore/interface.rs::AliyunOssApi::execute`（实际 Rust 调用链）、`pkg/objstore/s3like/retry.rs`（trait、包装器和外层特化规则）、`pkg/objstore/ossstore/store.rs`（确认当前 Rust Store 未注入 `new_retryer`）。目标目录没有 `doc.go`，因此无额外包契约文件可读。
- crate 边界：`pkg/objstore/ossstore/Cargo.toml`。
- Go 对照：`pkg/objstore/ossstore/retry.go` 与 `pkg/objstore/ossstore/store.go::NewOSSStorage`。
- 独立 Rust 测试：`pkg/objstore/ossstore/retry_test.rs::wrapped_connection_error_text_remains_retryable_like_go_sdk`、`pkg/objstore/ossstore/migration_aster_unit_test.rs::retry_logger_and_store_helpers_match_go`、`standard_retry_classification_matches_aliyun_sdk`。本任务按计划为纯文档分析，未运行 Cargo；这些测试仅作为既有行为证据读取。
- 人工复核结论：本文件存在是为了把 OSS/HTTP/IO 错误统一成 `s3like::StandardRetryer` 策略；当前执行由 `AliyunOssApi::execute` 驱动；安全扩展必须同步独立测试，并特别防止直接策略与通用包装器或 SDK 形成双层重试。
