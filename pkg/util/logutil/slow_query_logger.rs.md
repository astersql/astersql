# `pkg/util/logutil/slow_query_logger.rs`

## 文件定位

本文件属于 `astersql-util-logutil` crate；crate 入口 `pkg/util/logutil/lib.rs` 以 `pub mod slow_query_logger` 暴露该模块，`pkg/util/logutil/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认其编译边界。它位于通用 `Logger` 与 SQL 执行器生成的完整慢日志文本之间：本文件负责从普通日志配置/实例派生慢查询 logger，并定义最终文件编码格式；是否应记录某条查询、慢日志正文包含哪些字段，则由 `pkg/executor/adapter_slow_log.rs` 等上游决定。

当前全局接线在 `pkg/util/logutil/log.rs::initialize_loggers`：慢查询文件非空且不同于主日志文件时调用 `new_slow_query_logger`；路径为空或与主日志文件相同时直接克隆 background logger。写出侧由 `pkg/executor/adapter_slow_log.rs::WriteForcedSlowLog` 取得 `log::slow_query_logger()` 后调用 `Logger::warn`。

## 核心职责

- `new_slow_query_log_config` 克隆调用方配置，清空全局日志级别，并在配置了 `slow_query_file` 时只替换目标文件名，同时保留主文件的滚动、保留和压缩参数。
- `new_slow_query_logger` 用派生配置调用 `log::init_logger`，再把返回的 `Logger` 标记为慢日志编码模式。
- `new_slow_query_logger_from_logger` 从现有 `Logger` 克隆出共享同一 sink/level 的慢日志视图，只改变克隆实例的编码标志。
- `SlowLogEncoder::encode` 把时间与已经格式化好的 SQL/慢日志正文编码为 `# Time: ...` 和消息两部分，并明确忽略结构化字段。
- 三个 `pub use` 别名保留 Go 风格命名，便于移植代码使用 `newSlowQueryLogConfig`、`newSlowQueryLogger` 和 `newSlowQueryLoggerFromZapLogger`。

本文件不负责慢查询阈值判断、慢日志项目组装、日志级别比较、文件打开/轮转或全局 logger 的保存；这些职责分别位于执行器和 `pkg/util/logutil/log.rs`。

## 主要符号

- `pub fn new_slow_query_logger(cfg: &LogConfig) -> Result<Logger, String>`：主要构造入口。先调用 `new_slow_query_log_config`，再调用 `init_logger`；成功后以 `Logger::with_slow_log_encoding` 返回启用专用编码的克隆，失败则原样传播字符串错误。
- `pub fn new_slow_query_logger_from_logger(logger: &Logger) -> Logger`：不重新解析配置、不产生错误；调用 `with_slow_log_encoding`，保留原 logger 共享的 sink 与 level。
- `pub fn new_slow_query_log_config(cfg: &LogConfig) -> LogConfig`：纯配置派生函数。`LogConfig: Clone`，因此不会修改输入；无论输入为何都令 `level` 为空。若 `slow_query_file` 非空，则从 `cfg.file` 完整复制文件策略，再把 `filename` 改为慢日志路径；为空时保留克隆所得的主文件配置。
- `pub struct SlowLogEncoder`：无字段、`Clone + Copy + Debug + Default` 的编码器标记类型，不持有缓冲区或资源。
- `pub fn SlowLogEncoder::encode(&self, time: SystemTime, message: &str, _fields: &[LogField]) -> String`：将 `SystemTime` 转为 `chrono::DateTime<Utc>`，按 `log::SlowLogTimeFormat`（`%Y-%m-%dT%H:%M:%S%.fZ`）格式化，返回 `# Time: <UTC 时间>\n<message>\n`。参数名 `_fields` 和实现共同表明字段不会进入输出。
- `newSlowQueryLogConfig`、`newSlowQueryLogger`、`newSlowQueryLoggerFromZapLogger`：上述 snake_case API 的公开别名，没有额外逻辑。最后一个别名沿用 Go 的 Zap 名称，但 Rust 参数是本 crate 的 `Logger`，不是 Zap 类型。

## 执行流程

独立慢日志文件的初始化流程如下：

1. `log.rs::initialize_loggers` 发现 `slow_query_file` 非空且与 `cfg.file.filename` 不同，调用 `new_slow_query_logger`。
2. `new_slow_query_log_config` 克隆 `LogConfig`，清空 `level`；复制主文件配置并将文件名替换为 `slow_query_file`。
3. `log.rs::init_logger` 校验 compression、解析空 level 为默认级别，并依据文件名创建内存或文件 sink。
4. `with_slow_log_encoding` 克隆 logger，将该克隆的 `slow_log_encoding` 设为 `true`；sink 和 level 仍通过 `Arc` 共享。
5. 上游调用 `Logger::log` 时，级别和采样检查先发生，随后构造 `LogEntry`。文件 sink 且编码标志为真时调用 `SlowLogEncoder::encode`，再追加写入字符串。

从已有 logger 派生时直接从第 4 步开始。若全局初始化发现慢日志路径为空或等于主文件路径，则 `initialize_loggers` 直接克隆 background，不调用本文件工厂；因此该共享路径按当前 Rust 代码不会设置 `slow_log_encoding`。这与“专用 logger 工厂被调用”是两个不同分支。

## 数据与状态

本文件自身没有全局可变状态。`SlowLogEncoder` 是零大小值；每次 `encode` 新建并返回拥有所有权的 `String`。配置派生基于深克隆的 `LogConfig`/`FileLogConfig`，所以清空 level 或替换 filename 不会回写原配置，`pkg/util/logutil/log_test.rs::TestSlowQueryLoggerAndGeneralLoggerCreation` 对输入 level 不变有直接断言。

状态实际承载于 `log.rs::Logger`：`sink: Arc<Mutex<Sink>>` 与 `level: Arc<RwLock<LogLevel>>` 在克隆间共享，`slow_log_encoding: bool` 则按值属于各克隆。因而 `new_slow_query_logger_from_logger` 会向同一内存/文件目标写入并服从同一动态级别，但只让返回值采用慢日志文件编码。固定字段和 sampler 的克隆语义也由 `Logger` 决定，而不是由本文件另行维护。

## 依赖与调用关系

直接 crate 内依赖来自 `crate::log::{LogConfig, LogField, Logger, SlowLogTimeFormat, init_logger}`；外部依赖只有 `chrono::{DateTime, Utc}`，其 `clock` feature 在 `pkg/util/logutil/Cargo.toml` 中启用。标准库提供 `SystemTime`。

RustCodeGraph 对 `new_slow_query_logger` 给出的明确下游调用边是 `new_slow_query_logger -> new_slow_query_log_config`。源码还显示随后调用 `init_logger` 和 `Logger::with_slow_log_encoding`；图索引未为这些方法调用生成完整边，因此以 `slow_query_logger.rs` 与 `log.rs` 源码为直接依据。上游直接接线为 `log.rs::initialize_loggers -> new_slow_query_logger`；编码调用为 `log.rs::Logger::log -> SlowLogEncoder::encode`。业务侧可见调用链是 `adapter_slow_log.rs::WriteForcedSlowLog -> log.rs::slow_query_logger -> Logger::warn/log`。

独立测试调用 `new_slow_query_logger`、`new_slow_query_logger_from_logger` 和 `SlowLogEncoder::encode`。`lib.rs` 没有把函数提升到 crate 根，而是公开整个 `slow_query_logger` 模块；调用者需经该模块路径或使用模块内 Go 风格别名。

## 错误处理与边界

`new_slow_query_logger` 的错误边界完全来自 `init_logger`：不支持的压缩名产生 `unsupported log compression ...`，无法解析的 level 产生解析错误。因为派生配置把 level 清空，而 `LogLevel` 的空串解析语义是默认 Info，正常慢日志构造不会继承全局级别。函数使用 `?` 原样传播 `String`，不增加上下文。`new_slow_query_logger_from_logger` 和 `encode` 均为不返回错误的接口。

`encode` 接受任意消息，包括空串或本身含换行的文本；它始终在时间行和消息末尾各补一个换行，不转义消息，也不读取字段。UTC 转换和固定格式确保测试中的纳秒精度样例为 `1970-01-01T00:00:00.123456789Z`。文件打开和 `write_all` 失败在 `Logger::log` 中被忽略，因此本文件的成功构造不保证后续落盘成功；这是调用方扩展错误报告时必须注意的既有边界。

`slow_query_file` 为空时，单独调用 `new_slow_query_logger` 会沿用 `cfg.file.filename`；但全局 `initialize_loggers` 在同一条件下不会调用该工厂。对外说明和测试必须区分工厂语义与全局接线语义。

## 并发与资源生命周期

本文件不创建线程、任务、通道或后台刷新器。编码过程只读参数并生成局部 `String`，`SlowLogEncoder` 无内部状态，可复制并在多个线程使用。

logger 的同步和资源生命周期由 `log.rs` 管理：sink 由 `Arc<Mutex<_>>` 串行访问，level 由 `Arc<RwLock<_>>` 共享；每次文件日志写入都在持有 sink mutex 时打开文件、追加并在局部句柄离开作用域时关闭。`new_slow_query_logger_from_logger` 不复制底层文件或内存条目，而是延长共享 `Arc` 的生命周期。独立测试用全局 `OnceLock<Mutex<()>>` 串行化临时文件场景，这是测试隔离措施，不是生产编码器状态。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/util/logutil/slow_query_logger.go`。两端共同保持以下语义：配置先复制而不修改原值；全局 level 不控制专用慢日志 logger；指定慢日志文件时继承主文件的滚动/保留配置并替换 filename；编码只输出 `# Time:` 行和消息，忽略所有结构化字段。

实现机制并非一一同构。Go `newSlowQueryLogger` 返回 `(*zap.Logger, *log.ZapProperties, error)`，用 `log.NewTextCore`、`zap.WrapCore` 替换 core，并更新 `prop.Core`；Rust 返回 `Result<Logger, String>`，没有 properties 返回值，而是在自有 `Logger` 克隆上设置布尔编码标志。Go `slowLogEncoder` 实现完整 `zapcore.Encoder` 接口并从 buffer pool 取得缓冲区；Rust `SlowLogEncoder` 只有单个 `encode` 方法，每次分配 `String`。Go 的 from-Zap 工厂还接收 properties 以复用 syncer/level；Rust 同名别名只接收 `&Logger`，通过 `Arc` 克隆共享 sink/level。

此外，Go 全局初始化的共享文件分支会调用 `newSlowQueryLoggerFromZapLogger` 替换编码 core；当前 Rust `initialize_loggers` 在慢日志路径为空或等于主文件时仅克隆 background。`log_test.rs::TestSlowQueryLoggerAndGeneralUseSameLogFileName` 只验证内容共同可见，没有断言共享分支仍使用两行慢日志编码，因此不能宣称该分支与 Go 编码完全等价。

## 扩展指南

- 修改慢日志文本格式、时间格式应用方式或字段处理时，应改 `SlowLogEncoder::encode`，并同步 `pkg/util/logutil/migration_aster_unit_test.rs::test_slow_encoder_ignores_fields_and_writes_two_lines` 与 `pkg/util/logutil/slow_query_logger_test.rs::slow_query_factories_replace_the_normal_file_encoder`。注意下游慢日志解析器可能依赖 `# Time:` 前缀和换行边界。
- 修改专用路径、滚动参数继承或 level 隔离时，应改 `new_slow_query_log_config`，同步 Rust `log_test.rs::TestSlowQueryLoggerAndGeneralLoggerCreation`，并对照 Go 同名测试。保持输入配置不变是已有不变量。
- 修改 logger 构造或共享策略时，应联合检查 `new_slow_query_logger`、`new_slow_query_logger_from_logger`、`log.rs::initialize_loggers` 和 `Logger::with_slow_log_encoding`。尤其要决定“共享主文件”分支应使用普通还是慢日志编码，并新增能精确断言文件格式的独立测试，不能只断言消息存在。
- 若引入缓冲池、持久文件句柄或异步写入，资源所有权、flush/关闭、锁顺序和写失败传播将成为新契约；当前零状态编码器和按次打开文件的假设不能直接沿用。
- 测试逻辑应继续放在相邻的独立 `*_test.rs` 文件中，不要内嵌到生产源文件。兼容风险集中在日志解析格式和 Go API 名称；性能风险集中在每条记录的字符串分配、时间格式化和文件 I/O。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/util/logutil` 找到目标 Rust/Go/测试文件。`node --file pkg/util/logutil/slow_query_logger.rs` 核对了 65 行完整实现，显示该文件被 `log.rs` 与 `slow_query_logger_test.rs` 使用。
- RustCodeGraph 符号/调用查询：`query slow_query_logger` 找到三个 snake_case 工厂/配置函数、`SlowLogEncoder::encode` 与 `log.rs::slow_query_logger`；`callees new_slow_query_logger` 确认 `new_slow_query_logger -> new_slow_query_log_config`。其余 callers/callees 结果为空或不完整，本文用相邻源码明确标注补证，没有把缺失图边当作无调用。
- 已读生产与配置证据：`pkg/util/logutil/slow_query_logger.rs`、`pkg/util/logutil/log.rs`、`pkg/util/logutil/lib.rs`、`pkg/util/logutil/Cargo.toml`、`pkg/executor/adapter_slow_log.rs`。
- 已读 Go 对照：`pkg/util/logutil/slow_query_logger.go`、`pkg/util/logutil/log.go` 的调用检索，以及 `pkg/util/logutil/log_test.go::TestSlowQueryLoggerAndGeneralLoggerCreation` 和共享文件测试片段。
- 已读 Rust 测试：`pkg/util/logutil/slow_query_logger_test.rs`、`pkg/util/logutil/migration_aster_unit_test.rs::test_slow_encoder_ignores_fields_and_writes_two_lines`、`pkg/util/logutil/log_test.rs` 中专用 logger 与共享文件测试。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅运行任务规定的 11 章节结构检查，并人工复核结论均能回指上述符号或文件。
