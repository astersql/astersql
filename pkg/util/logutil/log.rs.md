# [`pkg/util/logutil/log.rs`](./log.rs)

## 文件定位

本文件是 `astersql-util-logutil` crate 的核心日志实现。crate 入口 `pkg/util/logutil/lib.rs` 将它公开为 `log` 模块，并把专用的 General Log 与慢查询 logger 分别放在 `general_logger.rs`、`slow_query_logger.rs`。`pkg/util/logutil/Cargo.toml` 的 `[package.metadata.porting]` 明确把该 crate 对应到 Go 包 `pkg/util/logutil`。

它位于应用级日志调用与具体输出目标之间：`cmd/tidb-server/main.rs:1810` 和 `br/cmd/br/cmd.rs:302` 通过 `InitLogger` 建立进程全局 logger；session、server、DDL、statistics 等模块随后通过 `BgLogger`/`background_logger`、`general_logger` 或采样工厂写日志。RustCodeGraph 将该文件标为被 257 个文件使用，因此这里的级别、字段、全局替换和锁语义属于跨子系统公共契约。

## 核心职责

- 定义与 Go 同名的日志默认值、字段名和配置模型：`FileLogConfig`、`LogConfig`、`Default*`、`LogField*`。
- 用 `Logger` 封装内存或追加文件两类 `Sink`，执行级别过滤、固定字段合并和按“级别 + 消息”采样。
- 通过 `GlobalLoggers` 与 `globals()` 管理后台、慢查询、General、error-verbose 四个进程级 logger，并提供初始化、替换、查询和动态级别修改入口。
- 提供连接/会话追踪字段、轻量 `LogContext`、`TraceSpan` 事件/标签、代理环境变量记录以及采样 logger 工厂。
- 保留 `InitLogger`、`BgLogger`、`WithConnID` 等 Go 风格名称，降低逐文件移植时的调用改写成本。

本文件当前不是完整的 `pingcap/log`/zap 复刻：文件滚动参数只被保存，实际输出使用 `OpenOptions` 追加写；`format`、`disable_timestamp`、`disable_error_verbose` 没有参与编码选择；`gzip` 仅通过配置校验而未在本文件中实现压缩。

## 主要符号

- `FileLogConfig` 保存文件名、大小、天数、备份数和压缩名；`new_file_log_config`/`NewFileLogConfig` 只设置 `max_size`。`EmptyFileLogConfig` 是全空配置。
- `LogConfig::new`/`NewLogConfig` 组装主日志与慢查询、General 专用路径；`with_options` 依次运行函数指针形式的配置修改器。
- `LogLevel` 按 `Debug < Info < Warn < Error < Fatal` 排序；`FromStr` 接受空串、大小写不敏感的常用名称以及 `warning`，非法值返回字符串错误。
- `LogField` 支持单字符串、字符串数组、`u64`、`i64`、布尔值；`LogEntry` 保存时间、级别、消息和字段。
- `Logger` 的共享成员是 `Arc<Mutex<Sink>>` 与 `Arc<RwLock<LogLevel>>`；派生 logger 自有固定字段，可选共享 `Sampler`，并可启用慢日志编码。
- `Logger::memory`、`Logger::file` 创建输出目标；`with_fields`、`sample` 创建派生实例；`log` 是过滤、采样、组装与写入的统一入口；`entries` 只对内存 sink 返回快照。
- `GlobalLoggers`、`globals`、`initialize_loggers`/`InitLogger`、`replace_logger`/`ReplaceLogger`、`set_level`/`SetLevel` 组成全局生命周期 API。
- `background_logger`/`BgLogger`、`slow_query_logger`、`general_logger`、`err_verbose_logger`/`ErrVerboseLogger` 返回当前全局实例的克隆。
- `TraceInfo`、`fields_from_trace_info`、`logger_with_trace_info` 只在值有效时生成 `conn` 和 `session_alias` 字段；`LogContext` 及 `WithConnID`、`WithSessionAlias`、`WithCategory`、`WithKeyValue`、`WithTraceFields` 提供链式字段附加。
- `TraceSpan`、`event`/`Event`、`eventf`/`Eventf`、`set_tag`/`SetTag` 在可选 span 上维护事件与标签。
- `proxy_fields_from`、`proxy_fields`、`log_env_variables`/`LogEnvVariables` 读取代理变量；`sample_logger_factory` 与 `sample_err_verbose_logger_factory` 返回同一采样 logger 的克隆工厂。

## 执行流程

初始化从 `initialize_loggers(cfg)` 开始。它先调用 `init_logger`：拒绝非空且非 `gzip` 的压缩名，解析 `cfg.level`，再根据主文件名为空与否选择内存或文件 sink。慢查询路径为空或等于主文件时克隆 background，否则调用 `slow_query_logger::new_slow_query_logger`；General 路径采用同样的复用判定并在独立路径时调用 `general_logger::new_general_logger`。随后构造 `GlobalLoggers`，以 `GRPC_DEBUG` 是否存在且非空设置 `grpc_debug`，最后在 `globals()` 的写锁下原子替换整组实例并返回快照。

普通写入统一经过 `Logger::log`：先读级别阈值，低级别立即返回；如安装采样器，则在采样锁下按 `(LogLevel, message)` 找到窗口计数，窗口超过 `tick` 时归零，达到 `first` 时丢弃；随后把派生 logger 的固定字段放在调用字段之前并生成 `LogEntry`。内存 sink 直接追加；文件 sink 每条记录重新以 create+append 打开文件，慢日志实例交给 `SlowLogEncoder`，其他实例由 `Fields` 格式化为一行。

上下文字段不会修改原 logger：`with_fields` 克隆句柄并扩展其局部字段；`LogContext::with_*` 再包成新上下文。`fields_from_trace_info` 会省略零连接 ID 和空别名，而 `WithTraceFields` 在 `Some` 情况下无条件附加两个字段，这一差异由测试固定。

代理变量按 HTTP、HTTPS、NO_PROXY 顺序处理，每项优先大写名、再回退小写名，并忽略空值；只有至少一个字段存在时 `log_env_variables` 才写入 `using proxy config`。采样工厂预先附加调用字段和空值 `sampled` 标记、安装采样器，然后闭包每次返回同一共享状态的 logger 克隆。

## 数据与状态

全局状态由函数局部 `OnceLock<RwLock<GlobalLoggers>>` 惰性建立。初始四个角色共享同一个 Info 级内存 logger；初始化后它们可能继续共享 sink/level，也可能因专用路径而分离。`set_level` 只修改当前 background logger 的共享 `level`，因此只有与它共享该 `Arc<RwLock<_>>` 的克隆同步变化，独立慢查询/General logger 不随之改变。

`Logger` 克隆共享 sink、级别以及已经存在的 sampler，但 `fields` 和 `slow_log_encoding` 是按值复制。调用 `sample` 会创建一个新的 sampler 并放入派生 logger；该派生 logger 的后续克隆共同使用计数表。计数键包含完整消息字符串，旧键不会主动淘汰；窗口重置只发生在该键再次写入时。

`TraceSpan` 的事件向量和标签映射各自通过独立 `Arc<Mutex<_>>` 共享。事件保留插入顺序且允许重复；标签按 key 覆盖。`LogEntry.time` 使用 `SystemTime`，采样窗口使用不受系统时钟回拨影响的 `Instant`。

## 依赖与调用关系

本文件只直接依赖 Rust 标准库；crate 的 `chrono` 依赖由相邻慢查询编码模块使用。`Logger::log` 在慢日志文件分支下调用 `crate::slow_query_logger::SlowLogEncoder.encode`；`initialize_loggers` 向下调用 `new_slow_query_logger` 和 `new_general_logger`。

上游入口中，`cmd/tidb-server/main.rs` 与 `br/cmd/br/cmd.rs` 调用 `InitLogger`。`pkg/ddl/logutil/logutil.rs` 和 `pkg/statistics/handle/initstats/load_stats_page.rs` 调用 `sample_logger_factory`；`pkg/session/runtime/dispatch.rs` 使用 background/general logger 和多种 `LogField` 记录会话与 SQL 信息；`pkg/server/conn.rs`、`pkg/domain/serverinfo/syncer.rs` 等通过 background logger 记录服务运行状态。这些调用说明本文件既服务启动配置，也处于请求执行、DDL、统计和后台任务的热路径。

`pkg/ddl/logutil/lib.rs` 与 `pkg/statistics/handle/logutil/lib.rs` 还会再导出本模块 API，使不同子 crate 共享同一实现。新增字段变体或修改公开签名时，应检查这些再导出层及其调用者，而不能只搜索当前 crate。

## 错误处理与边界

可恢复配置错误以 `Result<_, String>` 返回：未知日志级别、非 `gzip` 压缩名，以及专用 logger 工厂返回的错误都会阻止全局替换。`initialize_loggers` 在所有 logger 均构造成功后才写全局状态，因此前序失败不会留下部分更新。

锁中毒使用 `expect`，会 panic；这适用于 sink、级别、采样器、全局状态和 trace 容器。文件打开、普通写入和慢日志 `write_all` 的错误则被静默忽略，`Logger::log` 不向调用者报告落盘失败。文件 sink 的 `entries()` 返回空数组，不能用于判断实际文件中是否有记录。

`FileLogConfig.max_size/max_days/max_backups` 当前不驱动滚动；接受 `gzip` 也不代表产物被压缩。非慢日志文件格式总是调试形式的级别加消息/字段，配置中的 `format` 与时间戳选项未被使用。`first == 0` 会丢弃所有采样日志；`tick == 0` 会在每次调用时重置窗口，因而每次都可重新放行。`WithTraceFields(Some(default))` 会输出零值字段，与 `fields_from_trace_info` 的省略策略不同。

## 并发与资源生命周期

`OnceLock` 保证全局容器只初始化一次，`RwLock` 允许并发读取当前 logger 族并串行替换。获取函数先在全局读锁下克隆 `Logger` 后释放锁，因此进行中的写日志不持有全局锁；替换只影响后续获取者，调用方已保存的旧 `Logger` 仍持有旧 `Arc` 并可继续写入。

单个 sink 的所有写入由同一 `Mutex` 串行化，文件打开和写入也发生在锁内，避免同一 logger 族内部交错，但会把文件 I/O 延迟传播给并发调用者。派生 logger 若共享 sink，同样竞争这把锁。采样器另有 mutex，先于 sink 锁获取；本文件不存在反向获取顺序。

文件没有长期持有句柄：每条记录打开并在函数退出时关闭，简化替换/删除，却增加系统调用开销。内存 sink 无容量上限，长期运行或测试中持续记录会增长。采样计数表同样无容量界限。测试 `serial_guard` 串行化会修改全局 logger 或环境变量的用例，并用 `EnvRestore` 在 Drop 时恢复代理/GRPC 环境。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/logutil/log.go`，测试对照是 `pkg/util/logutil/log_test.go`。常量、`FileLogConfig`/`LogConfig`、初始化/替换/设级别、背景与 error-verbose logger、trace 字段、代理记录、事件/标签和采样工厂均保留了相同概念与多数 Go 风格别名。

关键差异如下：Go 版本把 `pingcap/log`、zap core、滚动 writer、gRPC logger、TiKV context key、`context.Context`、runtime trace 和 OpenTracing 连接起来；Rust 版本用自有 `Logger`、`LogContext`、`TraceSpan` 和内存/简单文件 sink 表达最小可用语义，`grpc_debug` 目前只是状态标志。Go `ReplaceLogger` 会序列化配置并记录替换消息，Rust 直接复用初始化逻辑。Go 的 error-verbose logger 会按格式与 `DisableErrorVerbose` 构建专用 core，Rust 当前只是 background 克隆；Rust 的 `sample_err_verbose_logger_factory` 也使用传入 logger，而不自行选择全局 error-verbose logger。

Go zap sampler 文档注释指出同类日志计数有实现上限；Rust `HashMap` 没有 4096 种消息上限或后续每 N 条放行逻辑，只放行每窗口前 `first` 条。Go 的代理解析委托 `httpproxy.FromEnvironment`；Rust 显式实现大写优先和小写回退。扩展时应以这些当前差异为事实，不能仅因 API 同名就假定行为完全等价。

## 扩展指南

- 增加输出格式、滚动或压缩时，从 `init_logger`、`Sink` 和 `Logger::log` 接入，并明确 `FileLogConfig` 每个字段的实际语义；同时在独立的 `pkg/util/logutil/log_test.rs` 增加文件生命周期和失败路径测试，不要把测试嵌入源文件。
- 增加结构化字段类型时，同步修改 `LogField::key`、`Fields::fmt`、慢日志 `SlowLogEncoder` 及相关消费者；特别检查数组转义和字段顺序。
- 修改全局初始化时保持“全部构造成功后一次替换”的不变量，并验证共享主文件、独立慢查询/General 文件、旧句柄与新句柄行为。
- 修改采样时需定义零值、窗口边界、计数表回收及高基数消息的内存上限；同步 `pkg/ddl/logutil/logutil.rs`、statistics 调用点与 `TestSampleLoggerFactory`。
- 扩展 trace/context 语义前，应决定是继续轻量容器还是接入真正的异步任务上下文；同步核对 Go 的 `WithTraceLogger`、runtime trace 和 OpenTracing 行为。
- 所有 Rust 行为测试继续放在相邻独立文件 `log_test.rs`；Go 对照语义变化时同时查看 `log.go` 与 `log_test.go`。性能敏感改动需关注每条日志打开文件、字段克隆、全局/采样/sink 锁竞争和无界容器。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/logutil` 确认本 crate 的 Rust/Go 源与独立测试；`node --file pkg/util/logutil/log.rs --offset 1 --limit 500` 及 `--offset 490 --limit 320` 读取完整 748 行并列出“used by 257 files”；`query initialize_loggers --kind function` 定位到第 451 行。针对 `initialize_loggers`、`background_logger`、`sample_logger_factory` 的 callers/callees 查询未返回静态边，因此用精确 `rg` 补充跨 crate 调用证据。
- 已读实现与边界文件：`pkg/util/logutil/log.rs`、`pkg/util/logutil/lib.rs`、`pkg/util/logutil/Cargo.toml`、`pkg/util/logutil/general_logger.rs` 和 `pkg/util/logutil/slow_query_logger.rs` 的直接调用关系。
- 已读对照与测试：`pkg/util/logutil/log.go`、`pkg/util/logutil/log_test.go`、`pkg/util/logutil/log_test.rs`。Rust 测试覆盖 trace 字段筛选、上下文字段、级别修改、专用/共享文件、压缩名校验、非空 `GRPC_DEBUG`、全局替换、代理组合、采样上限和字符串数组格式。
- 上游证据：`cmd/tidb-server/main.rs:1810`、`br/cmd/br/cmd.rs:302`、`pkg/ddl/logutil/logutil.rs:72`、`pkg/statistics/handle/initstats/load_stats_page.rs:66`、`pkg/session/runtime/dispatch.rs` 的直接引用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 加 11 个固定二级标题计数命令，并人工复核本说明没有把 Go 能力误写成 Rust 已支持能力。
