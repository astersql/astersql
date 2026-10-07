# `pkg/objstore/ossstore/logger.rs`

## 文件定位

本文件属于 `astersql-objstore-ossstore` crate 的日志适配层。crate 入口 `pkg/objstore/ossstore/lib.rs` 通过 `mod logger` 声明模块，并通过 `pub use logger::*` 将公开项重新导出；因此 `OssLogLevel`、`LogPrinter`、`new_log_printer` 和 `get_oss_log_level` 都可从 crate 根访问。`pkg/objstore/ossstore/Cargo.toml` 表明该 crate 同时依赖 OSS 客户端 `ali-oss-rs` 和日志门面 `log`，但当前 Rust 生产构造路径 `pkg/objstore/ossstore/store.rs::NewOSSStorage`、`build_api` 并未引用本文件的公开项。

本文件对应 Go 实现 `pkg/objstore/ossstore/logger.go`，意图是把 OSS SDK 产生的“级别前缀 + 消息”转换为仓库日志，并把仓库日志过滤级别压缩为 OSS SDK 的粗粒度级别。就当前 Rust 代码事实而言，它是已导出的兼容适配能力和迁移测试对象，尚未接入 `AliyunOssApi` 的实际创建链。

## 核心职责

- `any_str` 统一从 `&dyn Any` 提取 `String` 或 `&str`，为 Go 风格可变参数形态提供运行时类型检查。
- `LogPrinter::Print` 校验 OSS 日志回调必须恰有两个字符串参数，并按固定前缀将消息分派到 `log::error!`、`warn!`、`info!` 或 `debug!`。
- `get_oss_log_level` 将 `log::LevelFilter` 映射为 `OssLogLevel`，其中 `Info` 主动提升为 `Warn`，以避免 OSS SDK 的调用起止日志过多。
- `OssLogLevel` 表达适配层允许交给 OSS SDK 的四档日志配置：关闭、错误、警告和调试。

这些职责只处理日志格式与级别，不创建 SDK 客户端、不读取全局 logger、不保存上下文，也不决定当前进程实际启用的 `LevelFilter`。

## 主要符号

- `fn any_str(value: &dyn Any) -> Option<&str>`：私有转换函数。先尝试 `String`，成功时借用其 `str` 内容；否则尝试装箱的 `&str`。其他类型返回 `None`，没有分配或复制字符串。
- `pub enum OssLogLevel { Off, Error, Warn, Debug }`：SDK 侧粗粒度日志级别。它派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`，可按值传递并用于测试比较；没有 `Info` 或 `Trace` 变体。
- `pub struct LogPrinter`：零大小、无状态打印器，派生 `Clone`、`Copy`、`Debug`、`Default`。它没有持有 logger、bucket、prefix 或 caller 信息。
- `pub fn new_log_printer() -> LogPrinter`：构造零状态打印器，仅返回 `LogPrinter`，不注册回调或修改全局日志设施。
- `pub fn LogPrinter::Print(&self, args: &[&dyn Any])`：公开的 Go 风格命名方法；crate 根的 `#![allow(non_snake_case)]` 允许保留 `Print`。它不是本仓库中某个 Rust trait 的实现，当前也未实现 `ali-oss-rs` trait。
- `pub fn get_oss_log_level(level: log::LevelFilter) -> OssLogLevel`：纯映射函数。映射表为 `Error -> Error`、`Warn | Info -> Warn`、`Debug -> Debug`、`Off | Trace -> Off`。

## 执行流程

调用 `LogPrinter::Print` 时，流程如下：

1. 检查 `args.len()`。不是 2 时，以 `warn!` 记录参数数量并立即返回。
2. 分别用 `any_str(args[0])` 与 `any_str(args[1])` 读取级别和消息。任一参数不是 `String` 或 `&str` 时，记录类型不合法的警告并返回。
3. 对级别字符串做精确匹配；尾随空格是协议的一部分：`"ERROR "`、`"WARNING "`、`"INFO "`、`"DEBUG "` 分别调用对应的 `log` 宏。
4. 未知前缀不产生日志，也不返回错误。

调用 `get_oss_log_level` 时只进行穷尽 `match`。`Info` 与 `Warn` 合并到 `OssLogLevel::Warn`；`Trace` 与 `Off` 合并到 `Off`。该函数不会查询进程当前过滤级别，调用者必须显式传入 `log::LevelFilter`。

## 数据与状态

本文件不维护可变状态。`LogPrinter` 是零大小类型，实例之间没有身份差异；`new_log_printer` 与 `Default::default()` 在状态上等价。`Print` 只在调用栈内借用参数，`any_str` 返回值的生命周期受输入 `Any` 引用约束，不会把消息保存到调用结束之后。

`OssLogLevel` 只是内部兼容枚举，并非 `ali-oss-rs` 的配置类型；当前源码中没有从它到 SDK 配置的转换实现。日志文本由 `log` 宏交给进程安装的全局 logger，过滤、格式化和输出目的地均不在本文件控制范围内。

## 依赖与调用关系

上游模块关系为 `pkg/objstore/ossstore/lib.rs -> logger.rs`：入口声明并公开再导出模块。RustCodeGraph 能索引 `LogPrinter`、`new_log_printer`、`get_oss_log_level` 和 `any_str`，但对这些符号的 `callers`/`callees` 查询没有返回调用边；仓库文本引用补查显示：

- `get_oss_log_level` 的唯一文件外 Rust 调用位于 `pkg/objstore/ossstore/migration_aster_unit_test.rs::retry_logger_and_store_helpers_match_go`。
- `new_log_printer`、`LogPrinter::Print` 和 `OssLogLevel`（除上述测试比较外）没有生产调用者。
- `pkg/objstore/ossstore/store.rs::NewOSSStorage -> build_api -> AliyunOssApi::new` 是当前 Rust OSS 客户端构造链，该链未配置本日志桥接。

下游依赖只有标准库 `std::any::Any` 和 `log` crate。`any_str` 被 `LogPrinter::Print` 调用；`Print` 再调用 `log` 的宏。`get_oss_log_level` 依赖 `log::LevelFilter`，但与 `Print` 没有直接调用关系。

## 错误处理与边界

`Print` 没有 `Result` 返回值。参数数量错误和参数类型错误被降级为警告并丢弃原消息，这符合日志回调不能反向中断 OSS 操作的边界。未知级别字符串被静默忽略；匹配区分大小写并要求尾随空格，因此 `"ERROR"`、`"error "` 等都不会输出。

`Any` 转换只接受具体的 `String` 与 `&str` 装箱形式；`Box<str>`、字节串、实现 `Display` 的其他类型都不接受。合法消息可为空。函数不转义、不截断、不去除换行，也不增加结构化字段。

`get_oss_log_level` 是穷尽匹配，不会失败。需要注意 `Trace -> Off` 并非通常的“更详细”映射，而是当前与 Go 默认分支对齐的明确策略；更改它会影响 SDK 日志量。当前测试覆盖 `Error`、`Warn`、`Info`、`Debug`、`Trace`，没有单独断言 `Off`，也没有直接覆盖 `Print` 的合法、非法和未知前缀分支。

## 并发与资源生命周期

`LogPrinter` 没有字段、锁、通道、线程或析构逻辑，可安全地按值复制；其方法只读借用 `self` 和参数。文件本身不启动异步任务或后台线程，也不拥有 OSS 客户端资源。

并发调用最终汇入 `log` 全局门面；输出端是否线程安全、是否异步以及日志刷新时机由安装的 logger 实现保证，本文件没有额外同步。借用的字符串只在一次 `Print` 调用期间使用，不存在跨线程保存或释放顺序要求。

## 与 Go 版本的对应关系

`pkg/objstore/ossstore/logger.go` 的 `logPrinter.Print` 同样要求两个字符串参数，并按四个带尾随空格的前缀分派日志；无效参数记录警告，未知前缀忽略。Rust 的 `any_str` 额外明确接受 `String` 与 `&str` 两种 `Any` 形态。

两版的重要差异如下：

- Go `logPrinter` 持有带 bucket、prefix、context 字段的 `*zap.Logger`，`newLogPrinter` 还通过 `zap.AddCallerSkip(3)` 保留 OSS SDK 原始调用位置；Rust `LogPrinter` 无状态，只调用 `log` 宏，因此不携带这些结构化字段，也不调整 caller 深度。
- Go `getOSSLogLevel` 自行读取 `tidblogutil.BgLogger().Level()` 并返回 OSS SDK 的整数常量；Rust `get_oss_log_level` 接收显式 `log::LevelFilter`，返回本地 `OssLogLevel`。
- Go `store.go::NewOSSStorage` 通过 `WithLogLevel(getOSSLogLevel()).WithLogPrinter(newLogPrinter(logger))` 实际接入 SDK；当前 Rust `store.rs` 和 `interface.rs` 未引用对应 Rust 符号。因此 Rust 文件目前完成了映射逻辑，但没有完成与生产 SDK 构造的等价接线。
- Go 的级别策略与 Rust 相同：`Info` 映射为 `Warn` 以降低噪声，其他未列出的级别关闭。Rust 综合迁移测试验证了这项映射，但没有复现 Go 对 printer 分派行为的直接测试。

## 扩展指南

若要接通生产日志，首先应确认 `ali-oss-rs` 当前版本的 logger 配置接口；不要假设 Go SDK 的 `LogPrinter` 接口可直接复用。最可能的修改点是 `pkg/objstore/ossstore/interface.rs::AliyunOssApi::new` 或 `pkg/objstore/ossstore/store.rs::build_api`，并需要定义 `OssLogLevel` 到 SDK 实际级别类型的明确转换。接线时应保留 `Info -> Warn` 的降噪策略，并评估 bucket/prefix 等结构化上下文和 caller 信息如何补齐。

修改参数协议或前缀映射时，应为 `LogPrinter::Print` 增加独立测试，覆盖四个合法前缀、未知前缀、参数个数错误、非字符串参数、`String` 与 `&str` 两种输入。按照仓库规则，测试逻辑应放在独立测试文件并由 `lib.rs` 的 `#[cfg(test)]` 模块装配，不应内嵌进 `logger.rs`；现有级别映射回归位于 `pkg/objstore/ossstore/migration_aster_unit_test.rs`，可拆出或在同类独立文件中扩充。

兼容风险主要是 SDK 版本接口差异和日志量变化；性能风险主要来自高频 SDK 日志的格式化与输出。若引入上下文状态，需要重新评估克隆成本、线程安全和生命周期，避免让日志桥接拥有或延长 OSS 客户端资源生命周期。

## 验证依据

- 源码：`pkg/objstore/ossstore/logger.rs`，核对 `any_str`、`OssLogLevel`、`LogPrinter`、`new_log_printer`、`LogPrinter::Print`、`get_oss_log_level` 的完整实现。
- crate 边界：`pkg/objstore/ossstore/lib.rs` 的 `mod logger` / `pub use logger::*`；`pkg/objstore/ossstore/Cargo.toml` 的 `ali-oss-rs`、`log` 依赖及 `lib.rs` 入口。
- Rust 生产链：`pkg/objstore/ossstore/store.rs::NewOSSStorage` 与 `build_api`，确认当前构造 `AliyunOssApi` 时未引用日志桥接。
- Go 对照：`pkg/objstore/ossstore/logger.go::{logPrinter,newLogPrinter,Print,getOSSLogLevel}` 与 `pkg/objstore/ossstore/store.go::NewOSSStorage`，确认参数协议、级别策略和 SDK 接线。
- 测试：`pkg/objstore/ossstore/migration_aster_unit_test.rs::retry_logger_and_store_helpers_match_go`，确认五种 `LevelFilter` 输入的预期映射；仓库搜索未发现 `LogPrinter::Print` 的 Rust 直接测试。
- RustCodeGraph：`status` 显示索引包含 `pkg/objstore/ossstore/logger.rs`；精确 `query` 定位到 `LogPrinter`（第 47 行）、`new_log_printer`（第 50 行）、`get_oss_log_level`（第 79 行）、`any_str`（第 24 行）。`explore`、按文件 `node` 及 `callers`/`callees` 未返回内容，因此调用者结论由全仓 `rg` 交叉核验，未据此虚构图边。
- 结构检查：交付前运行任务指定命令，要求文档存在且固定二级标题恰好为 11 个；本任务是纯文档分析，按计划不运行 Cargo。
