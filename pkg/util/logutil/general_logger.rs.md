# `pkg/util/logutil/general_logger.rs`

## 文件定位

[`general_logger.rs`](general_logger.rs) 属于 `astersql-util-logutil` crate；crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod general_logger` 公开此模块。它位于全局日志配置与 General Log 专用 logger 之间，负责从 [`log.rs`](log.rs) 的 `LogConfig` 派生专用配置，并委托同文件中的通用初始化器 `init_logger` 创建 `Logger`。它不决定是否记录某条 SQL，也不保存进程级 logger；进程级选择和安装发生在 `log.rs::initialize_loggers`。

根据 [`Cargo.toml`](Cargo.toml)，该 crate 的库入口是 `lib.rs`，Go 对照包为 `pkg/util/logutil`。本文件没有直接引入第三方 crate，使用的 `LogConfig`、`Logger` 和 `init_logger` 都来自 crate 内部 `log` 模块。

## 核心职责

本文件只有两项运行时职责。

1. `new_general_log_config(&LogConfig) -> LogConfig` 克隆调用者传入的全局配置，清空克隆值的 `level`；当 `general_log_file` 非空时，在保留主文件滚动配置等其余字段的同时，仅把克隆值的 `file.filename` 改成 General Log 路径。
2. `new_general_logger(&LogConfig) -> Result<Logger, String>` 把上述派生配置交给 `log::init_logger`，返回独立的内存或文件 logger，或原样传播字符串错误。

清空 `level` 是有意语义而非遗漏：`log.rs::LogLevel::from_str` 将空字符串解析成 `Info`。因此专用 logger 不继承全局日志阈值，而以 Info 为阈值；[`log_test.rs`](log_test.rs) 的 `TestSlowQueryLoggerAndGeneralLoggerCreation` 验证了全局配置为 `Error` 时 General logger 仍过滤 Debug、保留 Info，并且源配置未被修改。

## 主要符号

- `pub fn new_general_logger(cfg: &LogConfig) -> Result<Logger, String>`：公开工厂。它先调用 `new_general_log_config`，再调用 `init_logger`。返回的 `Logger` 是否使用内存 sink 或文件 sink，取决于派生后的 `file.filename` 是否为空。
- `pub fn new_general_log_config(cfg: &LogConfig) -> LogConfig`：公开的纯配置派生函数。其输入只通过共享引用读取，输出是拥有所有权的新 `LogConfig`。
- `pub use new_general_log_config as newGeneralLogConfig` 与 `pub use new_general_logger as newGeneralLogger`：为迁移代码保留 Go 风格名称；它们不是额外实现，也不改变调用语义。

本文件没有模块级常量、自定义类型、trait、`impl`、静态状态或条件编译项。

## 执行流程

应用初始化主链可概括为：

1. `log.rs::initialize_loggers` 先创建 background logger。
2. 若 `cfg.general_log_file` 为空，或与 `cfg.file.filename` 相同，初始化器直接克隆 background logger，不进入本文件，从而复用同一个底层 sink。
3. 只有当 General Log 指向不同的非空路径时，`initialize_loggers` 才调用 `new_general_logger`。
4. `new_general_logger` 调用 `new_general_log_config`；后者克隆整个配置、清空 `level`，再将 `file.filename` 改为 `general_log_file`。
5. `init_logger` 校验压缩算法、解析空级别为 `Info`，并根据文件名创建 `Logger::file`（若文件名为空则为 `Logger::memory`）。结果随后被放入 `GlobalLoggers.general`。

直接调用 `new_general_logger` 时并没有第 2 步的复用判断：调用者会得到按派生配置新建的 logger。Go 的 `ReplaceLogger` 总是直接调用 `newGeneralLogger`；当前 Rust 的 `replace_logger` 则复用 `initialize_loggers`，所以 Rust 的统一入口仍遵循相同文件名时复用 background 的规则。

## 数据与状态

输入 `LogConfig` 包含日志级别、格式、时间戳选项、主文件配置，以及慢查询和 General Log 路径。本文件通过 `cfg.clone()` 复制完整值，因此不会修改调用者持有的配置。随后只改输出副本的两个位置：

- `level` 总是清空；
- 仅当 `general_log_file` 非空时，`file.filename` 被改写。代码显式再次克隆 `cfg.file`，因而 `max_size`、`max_days`、`max_backups`、`compression` 等文件策略均来自主文件配置。

本文件自身不持有状态。返回的 `Logger` 内部状态定义在 `log.rs`：sink 使用 `Arc<Mutex<_>>` 共享，级别使用 `Arc<RwLock<_>>` 共享。全局状态则由 `log.rs::globals` 的 `OnceLock<RwLock<GlobalLoggers>>` 管理，不属于本文件。

## 依赖与调用关系

RustCodeGraph 将本文件标记为被 `pkg/util/logutil/log.rs` 和 `pkg/util/logutil/migration_aster_unit_test.rs` 使用，并确认调用边 `new_general_logger -> new_general_log_config`。源码进一步给出以下关系：

- 上游生产调用者：`log.rs::initialize_loggers` 在专用 General Log 文件与主文件不同时调用 `new_general_logger`。
- 上游测试调用者：`log_test.rs::TestSlowQueryLoggerAndGeneralLoggerCreation` 通过 Go 风格别名测试工厂及派生配置；`migration_aster_unit_test.rs::test_dedicated_configs_copy_file_settings_without_mutating_source` 直接测试蛇形命名的配置函数。
- 下游依赖：`new_general_logger` 调用本模块的 `new_general_log_config` 和 `log.rs::init_logger`；配置函数只执行标准库拥有值的克隆、清空与字段赋值，没有 I/O 调用。
- 间接下游：`init_logger` 调用 `LogLevel::from_str`，并选择 `Logger::memory` 或 `Logger::file`。真正打开并追加文件发生在后续 `Logger::log` 时，不发生在本文件的配置派生阶段。

## 错误处理与边界

`new_general_log_config` 不返回错误。空 `general_log_file` 不会覆盖已有 `file.filename`；非空值（即使与主文件名相同）会成为派生配置的文件名。正常应用入口会在调用它之前处理“空或相同文件名则复用 background”的情形。

`new_general_logger` 不包装错误，直接传播 `init_logger` 的 `Result<_, String>`。当前可见失败来源是：非空 `compression` 既不是 `gzip` 时返回“不支持的压缩算法”，以及无法识别的级别；但本函数把级别清空后，级别解析固定成功为 `Info`，故 General logger 在此路径上的现实配置错误主要是非法压缩设置。创建 `Logger::file` 本身不打开文件；后续写入时 `Logger::log` 对打开或写入失败采用尽力而为并忽略 I/O 错误，因此工厂成功并不证明目标路径可写。

空文件名会令直接工厂调用创建内存 logger；应用级 `initialize_loggers` 通常已在此前选择复用 background。测试应分别覆盖直接工厂语义与应用入口的复用语义，避免把二者混为一谈。

## 并发与资源生命周期

配置派生只操作函数局部的克隆值，没有锁、线程、异步任务、通道、事务或共享可变状态，可并发调用。`new_general_logger` 创建的 `Logger` 使用 `Arc` 加锁封装 sink 和级别，克隆 logger 时共享这些资源；锁中毒会在 `Logger` 后续操作中触发 `expect`，但本文件不获取这些锁。

文件资源不是由工厂长期持有：`Logger::file` 只保存路径，每次 `Logger::log` 才以 create/append 模式打开文件，写完后文件句柄随局部变量释放。同名文件的资源复用由 `initialize_loggers` 通过克隆 background logger 实现；[`log_test.rs`](log_test.rs) 的 `TestSlowQueryLoggerAndGeneralUseSameLogFileName` 验证慢查询与 General Log 都能写入共享主文件。

## 与 Go 版本的对应关系

直接对照 [`general_logger.go`](general_logger.go)：

- 两版都复制全局日志配置，不修改输入；都把专用配置的 level 设为零值/空值；`GeneralLogFile` 非空时都复制主文件配置并只替换文件名。
- Go `newGeneralLogger` 调用 `pingcap/log.InitLogger`，返回 `(*zap.Logger, *log.ZapProperties, error)`，并用 `errors.Trace` 包装错误；Rust 返回仓库内 `Logger` 和字符串错误，没有 `ZapProperties` 对应返回值，也没有错误链包装。
- Go 的空 level 由上游日志库解释为 Info；Rust 在 `LogLevel::from_str` 中显式规定 `"" | "info" => Info`，由 Rust 测试固定该迁移语义。
- Go `InitLogger` 在 General 文件为空或等于主文件时直接令 `GeneralLogger = gl`；Rust `initialize_loggers` 在相同条件下克隆 background。由于 Rust `Logger::clone` 共享 `Arc` 包装的 sink 和 level，这里保留了资源复用意图。
- Go 风格再导出名让邻近迁移代码继续使用 `newGeneralLogger` / `newGeneralLogConfig`，而蛇形名称提供惯用 Rust API。

Go 测试 `log_test.go::TestSlowQueryLoggerAndGeneralLoggerCreation` 是 Rust 同名测试的语义来源：两者都验证专用 logger 为 Info、输入级别不变以及文件滚动字段被保留。Go 的同文件名测试还通过反射比较底层 `WriteSyncer`；Rust 测试验证共享文件的可见输出，但没有暴露或直接比较内部 sink 指针。

## 扩展指南

- 若增加 General Log 独有配置，优先在 `new_general_log_config` 中集中派生，并保持先克隆、后修改的非破坏性约束；同时检查 `LogConfig`、构造函数及 Go 同路径实现是否需同步。
- 若改变“空 level 等于 Info”的约定，需要同时审查 `log.rs::LogLevel::from_str`、慢查询 logger（使用相同模式）、`TestSlowQueryLoggerAndGeneralLoggerCreation` 和迁移回归测试，不能只改本文件注释或字段赋值。
- 若改变独立文件与主文件的复用规则，接入点是 `log.rs::initialize_loggers`，不是此配置工厂；需保留相同文件名时避免重复 sink 的行为，并同步同文件名测试。
- 若增加可能失败的校验，决定错误属于纯配置派生还是 `init_logger` 的通用策略，并保持 `new_general_logger` 的传播契约。需要关注错误文本兼容性以及文件初始化/写入时机的变化。
- 测试逻辑应继续放在独立文件：专用配置边界可扩充 `migration_aster_unit_test.rs`，公开迁移行为可扩充 `log_test.rs`；不要把单元测试内嵌进 `general_logger.rs`。
- 性能风险主要来自不必要的配置/字符串克隆或错误地为同一文件创建独立 sink；正确性风险主要是意外继承全局级别、丢失文件滚动字段、修改输入配置或破坏同文件复用。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含本仓库 Rust/Go 文件；`node --file pkg/util/logutil/general_logger.rs` 展示完整 44 行源码并列出使用者 `log.rs`、`migration_aster_unit_test.rs`。
- RustCodeGraph 符号/调用查询：定位 `new_general_logger`；`callees new_general_logger` 确认调用 `new_general_log_config`；配置函数没有图中被调用者（字段克隆和赋值不会形成项目符号调用边）。
- Rust 源码：`general_logger.rs::{new_general_logger,new_general_log_config}`；`log.rs::{LogConfig,LogLevel::from_str,Logger,init_logger,initialize_loggers}`；crate 入口 `lib.rs`。
- crate 边界：`pkg/util/logutil/Cargo.toml` 的包名、库入口和 `package.metadata.porting.go-package`。
- Rust 独立测试：`log_test.rs::{TestSlowQueryLoggerAndGeneralLoggerCreation,TestSlowQueryLoggerAndGeneralUseSameLogFileName}`；`migration_aster_unit_test.rs::test_dedicated_configs_copy_file_settings_without_mutating_source`。
- Go 对照：`general_logger.go::{newGeneralLogger,newGeneralLogConfig}`；`log.go::{InitLogger,ReplaceLogger}`；`log_test.go::{TestSlowQueryLoggerAndGeneralLoggerCreation,TestSlowQueryLoggerAndGeneralUseSameLogFileName}`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令检查文档存在且恰有十一个固定二级章节，并人工复核链接、符号名称、范围和无依据结论。
