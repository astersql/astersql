# `pkg/objstore/s3store/logger.rs`

## 文件定位

用 `newLogger` 提供与 Go 版本命名相近的构造入口；构造不分配资源，也不携带配置。

## 核心职责

- 用本地 `Classification` 表达 SDK 日志的 `Warn`、`Debug`、`Info` 三类级别。
- 通过无状态的 `PingcapLogger` 把消息按级别转发给 `tracing::warn!`、`tracing::debug!` 或 `tracing::info!`。
- 为所有转发事件固定设置 `target: "aws_smithy"`，让订阅器可以按目标过滤或路由 AWS/Smithy 日志。
- 用 `newLogger` 提供与 Go 版本命名相近的构造入口；构造不分配资源，也不携带配置。

该文件不负责初始化 tracing subscriber、设置过滤级别、格式化可变参数、保存日志，也不负责把 logger 注入 AWS SDK。

## 主要符号

- `pub enum Classification`：日志级别枚举，派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`PartialEq`。变体为 `Warn`、`Debug`、`Info`，其中 `Info` 是默认值。它是本地兼容类型，不是 `aws_smithy_types` 中的 trait 或类型。
- `pub struct PingcapLogger`：零字段单元结构体，派生 `Clone`、`Debug`、`Default`。实例之间没有身份或状态差异。
- `pub fn newLogger() -> PingcapLogger`：直接返回 `PingcapLogger`。该函数不会读取全局 logger，也不会验证 subscriber 是否安装。
- `pub fn PingcapLogger::Logf(&self, classification: Classification, message: &str)`：同步分派入口。名称沿用 Go 风格，但 Rust 签名接收已经格式化好的 `&str`，没有 `format` 与可变参数。

crate 根的 `#![allow(non_snake_case, non_upper_case_globals)]` 使 `newLogger` 和 `Logf` 这类 Go 风格名称不会产生命名告警。

## 执行流程

1. 调用者用 `newLogger` 获得一个无状态 `PingcapLogger`；也可以等价地使用 `PingcapLogger` 或 `PingcapLogger::default()`。
2. 调用者把明确的 `Classification` 和已经生成的消息字符串传给 `Logf`。
3. `Logf` 对枚举做穷尽匹配：`Warn` 进入 `tracing::warn!`，`Debug` 进入 `tracing::debug!`，`Info` 进入 `tracing::info!`。
4. 三个分支都以 `aws_smithy` 为 tracing target，并以 `{message}` 记录消息。
5. 是否实际输出、输出到哪里以及是否因级别或 target 被过滤，由进程中已安装的 tracing subscriber 决定；本文件不返回记录结果。

当前唯一已索引 Rust 调用是测试中的显式 `newLogger().Logf(Classification::Debug, ...)`，并不经过 `NewS3Storage -> load_sdk_config -> build_api` 的生产构造流程。

## 数据与状态

`Classification` 的默认状态是 `Info`，因此通过 `Classification::default()` 构造时不会落入警告或调试级别。枚举是封闭集合，`Logf` 不需要兜底分支。

`PingcapLogger` 不包含 logger handle、字段上下文、缓冲区或锁；`Clone` 只是复制零大小值。`message: &str` 仅在同步宏调用期间借用，文件不会持有或修改消息。固定 target 是唯一由本模块附加的元数据；bucket、prefix、provider 等 S3 上下文字段不会在这里加入。

## 依赖与调用关系

- 模块上游：`pkg/objstore/s3store/lib.rs` 装载并公开本模块。RustCodeGraph 的文件关系显示 `logger.rs` 被 `pkg/objstore/s3store/s3_test.rs` 使用。
- 当前直接调用边：`test_s3_read_file_suppresses_skipped_checksum_validation_log -> newLogger -> PingcapLogger::Logf`。图查询没有发现生产调用者。
- 下游依赖：`Logf` 仅调用 `tracing` 的 `warn!`、`debug!`、`info!` 宏；`pkg/objstore/s3store/Cargo.toml` 声明 `tracing = "0.1"`，测试依赖另有 `tracing-subscriber = "0.3"`。
- 预期接线位置：若要恢复 Go 行为，应从 `pkg/objstore/s3store/store.rs::load_sdk_config` 或 `build_api` 所构造的 AWS Rust SDK 配置能力入手，而不能仅调用 `newLogger`；具体 SDK 接口在当前代码中尚未实现，需先验证所用 AWS Rust SDK 的日志/telemetry 接口。
- 应用主链关系：S3 的实际创建路径是 `NewS3Storage -> load_sdk_config/build_api -> AwsS3Api -> S3Client/s3like::Storage`。本文件当前不在这条运行时调用链上。

## 错误处理与边界

`newLogger` 与 `Logf` 都不返回 `Result`，也没有显式错误分支。tracing 宏是否被 subscriber 接收不影响调用结果；未安装 subscriber 或过滤掉事件时，调用仍正常返回。

边界包括：

- 只支持三种本地分类；新增级别会因穷尽 `match` 要求同步更新 `Logf`。
- `Logf` 不执行 Go 版本的 `fmt.Sprintf` 语义，格式化责任在调用者。
- 本文件没有 Go 版本“未知 classification 默认按 Info 记录”的开放输入边界，因为 Rust 枚举禁止构造未知变体；`Default` 仅覆盖显式默认构造。
- 本模块不实现 Go 的 checksum 跳过日志开关。Rust 测试名声称“suppresses”，但测试只在完成一次读取后手工记录 Debug 消息，没有安装观察器或断言该消息不可见，不能据此证明生产抑制行为。
- 日志内容未经本文件脱敏；接线或新增字段时必须避免写入 AccessKey、SecretAccessKey、SessionToken、签名头及其他凭证信息。

## 并发与资源生命周期

`PingcapLogger` 无内部可变状态、锁、通道、任务或运行时所有权；`Logf` 只同步提交一个 tracing 事件。因此本文件自身没有关闭、flush、取消或回收流程，也不存在由实例引入的共享状态竞争。

并发安全性实际取决于全局/线程局部 tracing dispatch 与已安装 subscriber；本类型没有显式实现或限制 `Send`/`Sync`，其零字段组成允许编译器自动推导。消息借用不会越过函数返回。若未来给结构体增加 subscriber handle、动态字段或缓冲，必须重新审查 `Clone` 语义、跨线程共享以及 flush 生命周期。

## 与 Go 版本的对应关系

- Go `pingcapLogger` 持有 `*zap.Logger`，`newLogger(logger)` 用 `zap.AddCallerSkip(1)` 保留调用点；Rust `PingcapLogger` 无状态，`newLogger()` 无参数，也没有 caller-skip 配置。
- Go `Logf(classification, format, v...)` 先用 `fmt.Sprintf` 格式化，再调用 zap；Rust `Logf(classification, message)` 只接收最终字符串。
- Go `store.go::NewS3Storage` 把 logger 同时传入 `config.WithLogger` 和 `s3.Options.Logger`，启用请求、重试、响应、弃用提示，并设置 `DisableLogOutputChecksumValidationSkipped = true`。Rust `store.rs` 没有对应 logger/日志模式/checksum 抑制接线。
- Go 回归测试 `TestS3ReadFileSuppressesSkippedChecksumValidationLog` 安装 warn 级观察器、执行真实本地 HTTP 读取，并断言观察日志不含指定文本；Rust 同名意图测试使用 mock 读取后手工调用 Debug 日志，没有等价断言。

因此，Rust 文件只移植了级别分派的核心外形，尚未达到 Go 生产接线和回归验证的完整语义。

## 扩展指南

- 新增分类时，修改 `Classification` 与 `PingcapLogger::Logf` 的穷尽匹配，并在独立测试文件（优先 `pkg/objstore/s3store/s3_test.rs`，或新增同目录独立 `logger_test.rs` 后从 `lib.rs` 以 `#[cfg(test)]` 装载）覆盖 target、级别和默认行为；不要把测试嵌入 `logger.rs`。
- 接入 AWS Rust SDK 时，修改最小范围应是 `store.rs` 的配置构造点和本日志桥，并对照 Go 的两个注入位置及日志模式。先确认 AWS Rust SDK 当前版本支持的 tracing/telemetry 扩展点，避免创建一个 SDK 从不调用的平行接口。
- 若要对齐 checksum 静默行为，应建立能捕获 tracing 事件的独立回归测试，验证完整 `NewS3Storage/ReadFile` 路径，而不是仅验证 `Logf` 可调用。
- 如需携带 bucket、prefix、provider 等上下文，优先用 tracing 字段表达；必须评估基数、性能与凭证泄露风险。
- 保留 `aws_smithy` target 的兼容性，或同步更新所有 subscriber filter、运维查询和测试。改变日志级别可能增加高频请求路径的日志量与格式化成本。
