# `pkg/objstore/s3store/retry.rs`

## 文件定位

本文件属于 `astersql-objstore-s3store` crate；crate 入口 [`lib.rs`](lib.rs) 以 `mod retry` 装入它并用 `pub use retry::*` 重新导出其公开项。它位于 S3/兼容 S3 客户端的策略层，负责把 TiDB/AsterSQL 的重试约定同时适配到两套接口：供兼容层使用的 `s3like::StandardRetryer`，以及供 AWS Rust SDK 客户端配置使用的 `storeapi::Retryer`。

在完整调用链中，[`store.rs`](store.rs) 创建 AWS 配置和 S3 client 时读取 `S3StandardRetryer::retry_config` 与 `retry_classifier`；[`interface.rs`](interface.rs) 的 `AwsS3Api::bucket_region` 在 `HeadBucket` 失败后使用 `newBucketRegionDetectionRetryer` 判断并记录错误，同时从 `x-amz-bucket-region` 响应头取回实际区域。因此，本文件不执行对象 I/O，也不拥有 HTTP client，而是为凭证加载、区域探测和普通 S3 操作提供重试参数、分类规则与兼容日志行为。

## 核心职责

- `newRetryer` 用无状态的 `S3StandardRetryer` 构造 `s3like::Retryer`，提供最多 20 次尝试、指数退避、可重试错误判定和不耗尽的令牌接口。
- `newBucketRegionDetectionRetryer` 在通用重试器上增加日志抑制器，只对确认为桶区域重定向的错误免除普通重试警告；它不放宽错误分类，也不吞掉错误。
- `isBucketRegionRedirectError` 同时检查 `HeadBucket` 的 HTTP 301 与 S3 错误码，避免仅凭错误消息把鉴权失败等错误当成预期重定向。
- `storeapi::Retryer for S3StandardRetryer` 生成 AWS SDK 原生 `RetryConfig`，并安装一个优先级晚于 transient-error classifier 的 IMDS 分类器，禁止重试访问 `169.254.169.254` 时产生的错误。
- `s3like::StandardRetryer for S3StandardRetryer` 保留 Go 风格接口语义，使现有对象存储兼容层能够统一处理日志、指标、连接错误特例和最小一秒退避。

## 主要符号

- `MAX_ATTEMPTS: i32 = 20`：公开常量，表示包含首次请求在内的最大尝试次数，即最多 19 次重试。`s3like` 路径的 `MaxAttempts` 和 AWS SDK 路径的 `retry_config` 都使用它。
- `EC2_META_ADDRESS: &str = "169.254.169.254"`：公开的 IMDS 链路本地地址标记，供兼容层与 SDK 分类器识别凭证元数据访问错误。
- `MAX_BACKOFF: Duration = 32s`：内部退避上限；兼容层在第 6 次及以后固定返回该值，SDK 配置也把它作为最大退避。
- `newRetryer() -> s3like::Retryer`：公开构造入口，将 `S3StandardRetryer` 装箱为 `s3like::StandardRetryer` trait object。
- `newBucketRegionDetectionRetryer() -> s3like::Retryer`：公开的区域探测专用构造入口；调用 `newRetryer().WithLogSuppressor(...)`。
- `isBucketRegionRedirectError(&SdkError<HeadBucketError, HttpResponse>) -> bool`：公开的结构化错误分类函数，要求响应状态和 modeled error code 同时匹配；兼容 Rust SDK 对空响应体 301 不提供错误码的情况。
- `S3StandardRetryer`：公开、零字段、`Copy` 的策略类型；实现 `s3like::StandardRetryer` 和 `storeapi::Retryer` 两个 trait。
- `S3StandardRetryer::IsInstanceMetadataError`：检查错误文本是否含 IMDS 地址；同名 trait 方法显式转发到这个固有方法。
- `random_unit_interval() -> f64`：内部函数，从新建的 `RandomState` hasher 取 53 位随机数并归一化到 `[0, 1)`，用于 full jitter。
- `InstanceMetadataRetryClassifier`：内部 AWS SDK classifier；返回 `RetryForbidden`、`NoActionIndicated`，并声明运行优先级和诊断名称。

本文件没有条件编译项。公开 API 沿用 Go 风格命名，crate 根通过 `#![allow(non_snake_case, non_upper_case_globals)]` 接纳这些名字。

## 执行流程

1. 普通兼容层入口调用 `newRetryer`，得到包装 `S3StandardRetryer` 的 `s3like::Retryer`。其 `IsErrorRetryable` 先由 `s3like` 外层排除 IMDS deadline/reset 和 connection refused，再处理 connection reset、HTTP/2 中断，最后才委托本文件的关键字分类；可重试错误同时进入指标记录和警告日志。
2. 本文件的 `IsErrorRetryable` 将错误文本转为 ASCII 小写，匹配超时、临时不可用、连接重置、意外 EOF，以及 `SlowDown`、`RequestTimeout`、`InternalError`、`ServiceUnavailable` 等 S3 服务错误标识。
3. 调用方在一次失败后请求 `RetryDelay(attempt, error)`。`attempt <= 5` 时计算上界 `2^attempt` 秒并乘以 `[0,1)` 随机值；`s3like::Retryer` 外层再把结果钳制为至少 1 秒。`attempt > 5` 时直接返回 32 秒。
4. `GetRetryToken` 和 `GetInitialToken` 都返回无操作释放闭包，因此即使大量并发请求持续失败，也不会消耗共享令牌桶。
5. AWS SDK 配置路径由 `store.rs::retry_config_for_options` 选择：调用方未注入 `Options::S3Retryer` 时，使用本文件的标准配置（20 次、初始 1 秒、最大 32 秒）；`retry_classifier_for_options` 同理选择本文件的 IMDS classifier。普通 S3 client 将 classifier 推入配置，region probe client 则直接设置本策略的 retry config。
6. classifier 在 SDK 拦截器上下文存在错误且错误文本含 IMDS 地址时返回 `RetryForbidden`；其优先级排在 transient classifier 之后，以便覆盖后者可能给出的瞬时错误结论。其他输入返回 `NoActionIndicated`，交给其余 classifier 决定。
7. 桶区域探测失败后，`AwsS3Api::bucket_region` 先保留响应头中的区域，再调用 `newBucketRegionDetectionRetryer().IsErrorRetryable(...)` 走兼容分类/日志路径。日志抑制闭包只在 `isBucketRegionRedirectError` 判真时生效；随后有区域响应头则成功返回，否则传播 `HeadBucket` 诊断错误。

## 数据与状态

`S3StandardRetryer` 和 `InstanceMetadataRetryClassifier` 都是零字段策略对象，没有请求级可变状态。配置参数由三个常量确定：20 次最大尝试、32 秒最大退避、IMDS 地址字符串。`RetryDelay` 的输入 `attempt` 决定指数上界；负数经 `u32::try_from(...).unwrap_or(0)` 按指数 0 处理，而超过 5 的值直接使用上限。

唯一每次调用生成的数据是 `random_unit_interval` 的局部 hasher 和随机小数。令牌接口返回的 `Box<dyn FnOnce... + Send>` 不捕获共享配额；区域重定向 suppressor 则由 `s3like::Retryer` 持有。AWS SDK classifier 读取 `InterceptorContext::output_or_error`，但不修改上下文。

## 依赖与调用关系

- 上游装配：[`lib.rs`](lib.rs) 导出本模块；[`store.rs`](store.rs) 的 `build_api`、`retry_config_for_options`、`retry_classifier_for_options` 把策略接入 AWS SDK；[`interface.rs`](interface.rs) 的 `bucket_region` 使用区域探测重试器。
- 兼容层下游：`newRetryer -> s3like::NewRetryer`；`newBucketRegionDetectionRetryer -> newRetryer -> Retryer::WithLogSuppressor`。`s3like::Retryer` 再调用本类型的 `IsErrorRetryable`、`RetryDelay`、令牌和 IMDS 判定方法。
- SDK 下游：`retry_config` 构造 `aws_sdk_s3::config::retry::RetryConfig::standard()`；`retry_classifier` 构造 Smithy `SharedRetryClassifier`；重定向分类读取 `SdkError<HeadBucketError, HttpResponse>` 的 raw response 与 `ProvideErrorMetadata`。
- crate 边界：[`Cargo.toml`](Cargo.toml) 声明 `aws-sdk-s3`、`anyhow`、`s3like`、`storeapi` 等直接依赖，并以 `lib.rs` 为库入口；没有针对本文件的 feature gate。
- RustCodeGraph 的精确边确认 `newBucketRegionDetectionRetryer` 调用 `newRetryer`、`RetryDelay` 调用 `random_unit_interval`，并找到 `s3_test.rs::test_retry_error -> newRetryer`。trait object 和同名 trait 方法的动态分派没有形成完整 callers 边，因此上述生产接线另由 `store.rs` 与 `interface.rs` 的直接引用核验。

## 错误处理与边界

`isBucketRegionRedirectError` 采用保守的结构化判断：没有 raw response、状态不是 301、错误码不是 `MovedPermanently`/`PermanentRedirect` 时均返回 `false`。Rust SDK 对空的 301 `HeadBucket` 响应可能留下一个无 code 的 service error，因此仅在“确实是 service error 且响应体存在并为空”时把 `None` 视作预期重定向；单纯匹配消息不成立。

兼容层错误分类依赖错误链最终格式化得到的文本。它对大小写做归一化，但仍可能受第三方错误消息变化影响。IMDS 防重试有两层：`s3like::Retryer` 只在 IMDS 错误同时属于 deadline/reset 时禁止重试；AWS SDK classifier 对任意包含 IMDS 地址的输出错误返回 `RetryForbidden`。两者服务于不同调用接口，扩展时不能误认为完全相同的判定范围。

`RetryDelay` 当前计算不会返回业务错误，签名保留 `Result` 是为了满足 trait。兼容层外部保证至少等待 1 秒；直接调用底层 trait 方法时，前五次 full jitter 可以小于 1 秒。移位只在 `attempt <= 5` 分支进行，因而不会随大 attempt 溢出。区域探测 suppressor 只控制警告输出，不把不可重试错误改为可重试，也不改变最终错误传播。

## 并发与资源生命周期

两个策略类型均无共享可变字段，`S3StandardRetryer` 实现 `Copy`，且两个 trait 都要求 `Send + Sync`，可安全地被多个请求共享。`random_unit_interval` 每次构造独立 hasher，不持有全局 RNG 锁；随机值只影响退避分散，不影响尝试上限或错误正确性。

关闭令牌桶是刻意的并发行为：Go 实现指出，同一 S3 store 上大量并发请求遇到网络故障时，默认共享 500-token bucket 可能耗尽且错误路径不归还 token；Rust 的两个 token 方法因此始终返回无操作闭包。代价是本层不提供客户端侧配额背压，流量控制依赖指数退避、SDK/HTTP 层和上游并发限制。

本文件不创建 Tokio task、不持有锁、连接、响应体或 runtime。SDK classifier 的生命周期由 `SharedRetryClassifier` 和 S3 client 配置拥有；`s3like::Retryer` 的 trait object 与 suppressor 闭包随其值一起释放。

## 与 Go 版本的对应关系

直接对照文件是 [`retry.go`](retry.go)，行为测试对照是 [`retry_test.go`](retry_test.go)。两端都规定 20 次最大尝试、32 秒退避上限、禁用会耗尽的共享令牌桶、识别 IMDS 地址，并仅对“301 + 指定 S3 code”的区域重定向抑制告警。

Go 的 `retryer` 嵌入 AWS Go SDK `retry.NewStandard`，由 SDK 提供标准错误分类和 full-jitter 退避；Rust 为兼容 `s3like` 接口显式实现关键字分类与抖动，同时通过 `storeapi::Retryer` 为 AWS Rust SDK 提供原生配置。Go 通过 `errors.As` 同时取 `smithy.APIError` 与 `ResponseError`；Rust 使用具体的 `SdkError<HeadBucketError, HttpResponse>`。Rust 额外兼容无 modeled code、空响应体的 301，因为 AWS Rust SDK 不像 Go SDK 那样为该响应合成 `MovedPermanently`。

Rust 的 `InstanceMetadataRetryClassifier` 是适配 AWS Rust SDK 原生重试流水线所需的局部接线；Go 侧对应约束由 `s3like::Retryer` 对嵌入 retryer 的 `IsInstanceMetadataError` 调用完成。两种实现路径不同，但共同目标是避免凭证元数据失败被当作普通 S3 瞬时错误反复等待。

## 扩展指南

- 增加可重试错误时，优先修改 `S3StandardRetryer::IsErrorRetryable` 的精确条件，并在独立的 [`retry_test.rs`](retry_test.rs) 添加正、反例；若意图对齐 Go，必须同步核对 `retry.go` 和 `retry_test.go`，不要只扩大字符串匹配。
- 调整次数或退避时，要同时修改 `MAX_ATTEMPTS`、`MAX_BACKOFF`/`RetryDelay` 和 `retry_config`，确保兼容层与 SDK 原生路径保持一致；同步验证 1 秒外层下限、32 秒抖动边界、总等待区间以及 `store.rs` 的默认/自定义选择测试。
- 修改区域重定向规则应从 `isBucketRegionRedirectError` 接入，并覆盖 code、HTTP status、缺失 response、空 body 无 code 和非预期错误；不得把 suppressor 扩展为宽泛的 301 或消息匹配，否则会隐藏真实故障告警。
- 修改 IMDS 策略要同时审查固有方法、`s3like::StandardRetryer::IsInstanceMetadataError` 转发和 `InstanceMetadataRetryClassifier::classify_retry/priority`，并明确两条路径是否应维持不同范围。
- 若新增状态、共享限流器或异步资源，需重新证明 `Send + Sync`、跨请求公平性、错误时 token 归还以及关闭 client 后的释放顺序。目前无状态实现不应被无意改成全局锁竞争点。
- Rust 单元测试继续放在独立的 `retry_test.rs`；涉及 store 选项接线的断言放在现有 `client_1_aster_unit_test.rs`，端到端 S3 行为则扩展 `s3_test.rs`，不要把测试内嵌回生产文件。

## 验证依据

- RustCodeGraph：`status` 显示目标在已索引的 7,032 个 Rust 文件内；`files --filter pkg/objstore/s3store` 显示 `retry.rs` 有 30 个符号；`node --file pkg/objstore/s3store/retry.rs` 读取了完整 203 行源码。
- RustCodeGraph 符号查询：核对了 `newRetryer`、`newBucketRegionDetectionRetryer`、`isBucketRegionRedirectError`、`S3StandardRetryer`、`IsErrorRetryable`、`RetryDelay`、`retry_config`、`classify_retry`；精确 callers/callees 查询确认内部构造、抖动函数和 `s3_test.rs::test_retry_error` 测试边。图对 trait 动态分派和若干同名符号有漏边/误配，未将其错误候选作为结论。
- 生产源码：完整读取目标文件；读取 [`lib.rs`](lib.rs) 的模块导出、[`store.rs`](store.rs) 第 197–320 行的 SDK 配置接线、[`interface.rs`](interface.rs) 第 982–1001 行的桶区域探测、`s3like/retry.rs` 第 29–162 行的外层错误规则，以及 `storeapi/storage.rs` 第 61–73 行的 SDK retry trait。
- crate/对照证据：读取 [`Cargo.toml`](Cargo.toml) 和完整 [`retry.go`](retry.go)，确认 crate 入口、依赖边界和 Go 移植语义。
- 测试证据：读取完整 [`retry_test.rs`](retry_test.rs) 与 [`retry_test.go`](retry_test.go)，并核对 `s3_test.rs::test_retry_error`、`client_1_aster_unit_test.rs` 的默认/自定义 retry config 测试。现有断言覆盖结构化 301 分类、日志抑制、10,000 次 token 申请、7–9 分钟总退避、抖动边界、IMDS 排除、连接重置和 SDK 默认参数。
- 本任务为只增说明文档的分析任务，未运行 Cargo；最终以固定 11 个二级章节的结构命令验证，并人工复查文件定位、运行流程、安全扩展点均能由上述路径和符号追溯。
