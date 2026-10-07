# `pkg/objstore/s3like/retry.rs`

## 文件定位

`retry.rs` 是 `astersql-objstore-s3like` crate 的通用重试策略适配层。crate 根在 `pkg/objstore/s3like/lib.rs` 中以 `mod retry` 装配并 `pub use retry::*` 导出本文件 API；`pkg/objstore/s3like/Cargo.toml` 声明其依赖 `anyhow`、`fail`、`storeapi` 和 `tracing`。它不实现某一云厂商的完整重试算法，而是用 `StandardRetryer` 抽象接收 S3/OSS 后端实现，再叠加 AsterSQL 共享的连接错误、IMDS 快速失败、日志和指标规则。

主要上游构造点是 `pkg/objstore/s3store/retry.rs::newRetryer` / `newBucketRegionDetectionRetryer` 与 `pkg/objstore/ossstore/retry.rs::new_retryer`。辅助函数 `IsHTTP2ConnAborted` 和 `IsDeadlineExceedError` 还被 `pkg/objstore/s3like/store.rs::ReadFile` / `doReadFile` 直接用于读路径判定。

## 核心职责

- 定义 `StandardRetryer` trait，统一不同 SDK 的可重试性、最大尝试次数、退避、令牌和实例元数据错误判定。
- 用 `Retryer` 包装底层策略，按固定优先级处理 IMDS 超时/复位、普通连接复位、连接拒绝和 HTTP/2 中断，其他错误才下沉到 `StandardRetryer::IsErrorRetryable`。
- 对可重试错误调用 `crate::RecordRetryableError`，并为每次分类输出 `tracing::warn!`；`WithLogSuppressor` 可只抑制匹配错误的该条告警，不改变重试决策。
- 将底层退避值提升到至少 1 秒，避免对对象存储的紧密重试。

## 主要符号

- `ReleaseToken = Box<dyn FnOnce(Option<&Error>) -> Result<()> + Send>`：一次性令牌释放回调。调用方可传入最终错误反馈配额系统；回调本身也可失败。
- `StandardRetryer: Send + Sync`：底层策略边界。`IsErrorRetryable`、`MaxAttempts`、`RetryDelay`、`GetRetryToken`、`GetInitialToken` 对应 SDK 重试器能力，`IsInstanceMetadataError` 是 AsterSQL 额外的凭证/IMDS 分类钩子。
- `Retryer { standardRetryer, suppressLog }`：持有 `Box<dyn StandardRetryer>` 和可选的线程安全日志过滤闭包。字段不对 crate 外暴露。
- `NewRetryer`：公开构造函数，初始无日志抑制器。
- `Retryer::WithLogSuppressor`：消费并返回 `self` 的 builder 方法，供 bucket-region 探测等已知、预期错误消音。
- `Retryer::IsErrorRetryable`：本文件的核心决策入口；返回布尔值，但同时有 failpoint、指标和日志副作用。
- `Retryer::{MaxAttempts, RetryDelay, GetRetryToken, GetInitialToken}`：除 `RetryDelay` 强制 1 秒下限外，均是对底层策略的直接转发。
- `IsDeadlineExceedError`、`isConnectionResetError`、`isConnectionRefusedError`、`IsHTTP2ConnAborted`：基于 `Error::to_string()` 的错误文本分类器；其中复位检测仅 crate 内可见，拒绝检测仅本模块可见。

## 执行流程

1. S3 或 OSS 后端将自己的 `StandardRetryer` 实现装箱后传给 `NewRetryer`；如需对预期错误消音，继续调用 `WithLogSuppressor`。
2. SDK/适配器调用 `IsErrorRetryable(err)`。failpoint `replace-error-to-connection-reset-by-peer` 启用时，当次分类改用一个“connection reset by peer”临时错误；原错误不被修改。
3. 按顺序决策：IMDS 错误且为 deadline/reset 时立即不重试；否则普通 reset 可重试；refused 不重试；三种 HTTP/2/意外 EOF 文本可重试；其余交给底层。这个顺序是不变量，特别是 IMDS 快速失败必须先于普通 reset 规则。
4. 结果为可重试时记录错误计数；若抑制器不匹配，输出包含错误文本和 `retry` 布尔值的 warning。
5. 若上层决定重试，它可查询 `MaxAttempts`、申请 `GetRetryToken`并调用 `RetryDelay`；后者先获取底层结果，传播错误，成功时返回 `max(delay, 1s)`。本文件不执行 sleep 或重试循环。

## 数据与状态

`Retryer` 的持久状态只有底层 trait object 和可选日志过滤器；它没有尝试计数、时钟、令牌桶或任务句柄。尝试次数只作为 `RetryDelay(attempt, err)` 参数向下传递；具体退避上限、抖动和配额由 S3/OSS 实现管理。

`effective` 是单次 `IsErrorRetryable` 调用内的借用：通常指向输入 `err`，failpoint 触发时指向局部 `injected_error`。指标标签和日志均使用这个有效错误的字符串，因而 failpoint 场景观测到的是注入错误。

## 依赖与调用关系

- crate 内部：`crate::RecordRetryableError` 定义于 `pkg/objstore/s3like/metrics.rs`；`lib.rs` 将本文件全部公开符号重导出。
- S3 下游：`pkg/objstore/s3store/retry.rs::S3StandardRetryer` 实现 trait，`newRetryer` 构造通用包装器；`newBucketRegionDetectionRetryer` 通过 `WithLogSuppressor` 仅抑制结构化 HTTP 301 region redirect 的告警。
- OSS 下游：`pkg/objstore/ossstore/retry.rs::OssRetryer` 实现 trait，`new_retryer` 将默认策略交给 `NewRetryer`。
- 读路径：`pkg/objstore/s3like/store.rs::ReadFile` 用 `IsHTTP2ConnAborted` 进行额外的 5 次、10 ms 读重试；`doReadFile` 用 `IsDeadlineExceedError` 阻止 deadline 错误进入 body-read 重试。这是独立于 SDK `Retryer` 的上层循环。
- 外部类型：`anyhow::Error/Result` 是错误边界，`storeapi::Context` 只传给底层令牌策略，`fail::eval` 支持测试注入，`tracing` 负责诊断日志。

RustCodeGraph 能确认包装方法到 trait 方法的调用边（`Retryer::IsErrorRetryable -> StandardRetryer::IsErrorRetryable`、`MaxAttempts -> MaxAttempts`、`RetryDelay -> RetryDelay`），并确认 `RecordRetryableError` 的调用者包括本文件核心方法。

## 错误处理与边界

`RetryDelay` 用 `?` 原样传播底层计算错误，只在成功时施加 1 秒下限。`GetRetryToken` 也原样返回底层错误；`GetInitialToken` 的 trait 签名不允许创建阶段失败，但返回的释放回调仍可返回 `Result::Err`。

错误分类是大小写敏感的子串匹配，不解析结构化错误链。已确认文本为 `context deadline exceeded`、`read: connection reset`、`connection refused` 以及两种 HTTP/2 消息和 `unexpected EOF`。这使其能适配不同 SDK 包装层，也意味着上游文案变更、本地化、大小写差异或过度宽泛的 `unexpected EOF` 都是兼容风险。

规则冲突时按源码顺序处理：IMDS deadline/reset 即使底层认为可重试也返回 `false`；普通 reset 和 HTTP/2 中断无需查询底层即返回 `true`；refused 无需查询底层即返回 `false`。日志抑制回调只控制 warning，不抑制可重试指标。

## 并发与资源生命周期

`StandardRetryer: Send + Sync`、`suppressLog: ... + Send + Sync` 使 `Retryer` 可在满足 trait object 边界时跨线程共享；本文件自身没有内部可变计数器或锁。可重试指标的并发安全性由 metrics 实现提供，底层策略的共享状态也必须自行满足 `Send + Sync`。

`ReleaseToken` 是 `FnOnce`，所以每个取得的令牌最多释放一次；本文件只传递回调，不自动执行。调用者需在操作结束时消费它，并传入最终错误或 `None`。`RetryDelay` 只返回 `Duration`，不阻塞线程；何时等待、如何响应取消由 SDK/上层调度者负责。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/objstore/s3like/retry.go`。Rust 保留了同名的 `StandardRetryer`、`Retryer`、`NewRetryer`、五个 SDK 重试方法、IMDS 钩子和四个文本分类器，且 `IsErrorRetryable` 的分支顺序、可重试指标、告警语义与 1 秒退避下限一致。S3/OSS 后端分别对应 Go 的 AWS/Alibaba SDK 实现。

语言差异包括：Go 用 interface 和函数值，Rust 用 `Box<dyn StandardRetryer>` 与带线程边界的闭包；Go `NewRetryer` 返回指针且 `WithLogSuppressor` 就地修改，Rust 构造器返回值且 builder 消费 `self`；Go release token 接收 `error`，Rust 用 `Option<&Error>` 表示成功/失败；Go 使用 PingCAP logger/metrics，Rust 使用 `tracing` 与 crate 内 metrics 函数。Go failpoint 仅在输入错误非 nil 时替换，Rust API 接收非可空 `&Error`，因此触发时始终有可替换的错误。

## 扩展指南

- 新增通用错误特判时，修改 `Retryer::IsErrorRetryable` 的顺序化决策链，先明确它与 IMDS/reset/refused/HTTP2 规则的优先级，再在独立测试 `pkg/objstore/s3like/migration_aster_unit_test.rs` 添加命中、不命中和“是否回退底层”的断言。
- 新增云厂商时，在厂商 crate 实现全部 `StandardRetryer` 方法，再通过 `NewRetryer`接入；厂商特有状态码或结构化错误应留在该实现，不应无条件污染通用文本规则。
- 改动 trait 签名时必须同步 `pkg/objstore/s3store/retry.rs::S3StandardRetryer`、`pkg/objstore/ossstore/retry.rs::OssRetryer` 和 `pkg/objstore/s3like/migration_aster_unit_test.rs::Standard`，并检查所有令牌调用方的生命周期。
- 改动退避下限或令牌语义时，同步 `pkg/objstore/s3store/retry_test.rs` 的总退避/边界/万次令牌测试、`pkg/objstore/ossstore/migration_aster_unit_test.rs` 以及 Go 对照测试 `pkg/objstore/s3store/retry_test.go`。重点风险是放大请求、总延迟漂移、凭证获取卡顿和令牌泄漏。
- 扩大日志抑制范围时，保持“只抑制 warning，不改变分类/指标”的不变量，并扩展 `pkg/objstore/s3store/retry_test.rs::only_probe_suppresses_expected_warning`。
- Rust 单元测试应继续放在独立 `*_test.rs` / `migration_aster_unit_test.rs` 文件，不应内嵌到 `retry.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/objstore/s3like/retry.rs` 全部 162 行和 24 个符号；`node --file` 核对了 trait、类型别名、构造器、impl 和四个辅助函数。
- RustCodeGraph 查询：`query NewRetryer/IsErrorRetryable/RetryDelay/IsHTTP2ConnAborted/IsDeadlineExceedError`核对 Rust/Go 对应符号；`explore "retry.rs::Retryer ..."` 给出包装方法向 trait 方法的调用边，并定位 `RecordRetryableError` 指标边。
- crate/模块依据：`pkg/objstore/s3like/Cargo.toml`、`pkg/objstore/s3like/lib.rs`、`pkg/objstore/s3like/metrics.rs::RecordRetryableError`。
- 上下游依据：`pkg/objstore/s3store/retry.rs`、`pkg/objstore/ossstore/retry.rs`、`pkg/objstore/s3like/store.rs::ReadFile/doReadFile`。
- Go 对照：`pkg/objstore/s3like/retry.go`；Go 行为测试：`pkg/objstore/s3store/retry_test.go`。
- Rust 独立测试：`pkg/objstore/s3like/migration_aster_unit_test.rs::retryer_matches_connection_rules_and_one_second_floor`、`metadata_deadline_fast_fails_without_standard_fallback`；`pkg/objstore/s3store/retry_test.rs::only_probe_suppresses_expected_warning`、`test_s3_tidb_retryer_never_exhaust_tokens`、`test_s3_tidb_retryer`、`retry_delay_matches_aws_exponential_jitter_boundary`、`test_retryer_is_instance_metadata_error`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求本文档存在且恰好包含上述 11 个固定二级标题。
