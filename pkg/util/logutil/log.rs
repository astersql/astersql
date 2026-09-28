// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TiDB 风格日志核心：配置、全局 Logger、采样与追踪字段。
//
// 对应 Go `pkg/util/logutil`。提供文件/内存下沉、慢查询与 General Log
// 全局实例、连接/会话追踪字段、代理环境变量记录，以及按 tick 限流的
// 采样 logger 工厂。

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// 默认单日志文件最大体积（MB 量级，对齐 Go 默认）。
pub const DefaultLogMaxSize: u32 = 300;
/// 默认日志格式（text）。
pub const DefaultLogFormat: &str = "text";
/// 默认慢查询阈值（毫秒）。
pub const DefaultSlowThreshold: i32 = 300;
/// 默认慢事务阈值（毫秒）；0 表示关闭或沿用其它配置。
pub const DefaultSlowTxnThreshold: i32 = 0;
/// 慢日志中 SQL 文本最大长度。
pub const DefaultQueryLogMaxLen: i32 = 4096;
/// 慢日志是否默认记录执行计划。
pub const DefaultRecordPlanInSlowLog: i32 = 1;
/// 是否默认启用慢查询日志。
pub const DefaultTiDBEnableSlowLog: bool = true;
/// 日志字段名：category。
pub const LogFieldCategory: &str = "category";
/// 日志字段名：连接 ID。
pub const LogFieldConn: &str = "conn";
/// 日志字段名：会话别名。
pub const LogFieldSessionAlias: &str = "session_alias";
/// 慢日志时间格式（RFC3339 风格）。
pub const SlowLogTimeFormat: &str = "%Y-%m-%dT%H:%M:%S%.fZ";
/// 旧版慢日志时间格式。
pub const OldSlowLogTimeFormat: &str = "%Y-%m-%d-%H:%M:%S%.9f %z";
/// 启用 gRPC debug 的环境变量名。
pub const GRPCDebugEnvName: &str = "GRPC_DEBUG";
/// 追踪事件字段键名。
pub const TraceEventKey: &str = "event";

/// 文件日志滚动相关配置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileLogConfig {
    /// 日志文件路径；空表示使用内存下沉。
    pub filename: String,
    /// 单文件最大体积。
    pub max_size: i32,
    /// 保留天数。
    pub max_days: i32,
    /// 保留备份数。
    pub max_backups: i32,
    /// 压缩算法名（仅支持空或 gzip）。
    pub compression: String,
}

/// 空的文件日志配置常量（全零/空串）。
pub const EmptyFileLogConfig: FileLogConfig = FileLogConfig {
    filename: String::new(),
    max_size: 0,
    max_days: 0,
    max_backups: 0,
    compression: String::new(),
};

/// 仅设置 `max_size` 的便捷构造。
pub fn new_file_log_config(max_size: u32) -> FileLogConfig {
    FileLogConfig {
        max_size: max_size as i32,
        ..FileLogConfig::default()
    }
}

/// Go 风格别名：`NewFileLogConfig`。
pub use new_file_log_config as NewFileLogConfig;

/// 完整日志配置：级别、格式、文件与慢查询/General 专用路径。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogConfig {
    /// 日志级别字符串（info/debug/warn/error/fatal）。
    pub level: String,
    /// 输出格式（如 text）。
    pub format: String,
    /// 是否禁用时间戳。
    pub disable_timestamp: bool,
    /// 是否禁用 error verbose。
    pub disable_error_verbose: bool,
    /// 主日志文件配置。
    pub file: FileLogConfig,
    /// 慢查询专用文件路径。
    pub slow_query_file: String,
    /// General Log 专用文件路径。
    pub general_log_file: String,
}

impl LogConfig {
    /// 构造日志配置（其余字段取 Default）。
    pub fn new(
        level: impl Into<String>,
        format: impl Into<String>,
        slow_query_file: impl Into<String>,
        general_log_file: impl Into<String>,
        file: FileLogConfig,
        disable_timestamp: bool,
    ) -> Self {
        Self {
            level: level.into(),
            format: format.into(),
            disable_timestamp,
            file,
            slow_query_file: slow_query_file.into(),
            general_log_file: general_log_file.into(),
            ..Self::default()
        }
    }

    /// 链式应用若干就地修改闭包。
    pub fn with_options(mut self, options: impl IntoIterator<Item = fn(&mut LogConfig)>) -> Self {
        for option in options {
            option(&mut self);
        }
        self
    }
}

/// Go 风格构造函数别名。
pub fn NewLogConfig(
    level: impl Into<String>,
    format: impl Into<String>,
    slow_query_file: impl Into<String>,
    general_log_file: impl Into<String>,
    file: FileLogConfig,
    disable_timestamp: bool,
) -> LogConfig {
    LogConfig::new(
        level,
        format,
        slow_query_file,
        general_log_file,
        file,
        disable_timestamp,
    )
}

/// 日志级别，按严重程度可比较排序。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
    Fatal,
}

impl FromStr for LogLevel {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "" | "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "fatal" => Ok(Self::Fatal),
            _ => Err(format!("unrecognized log level {value:?}")),
        }
    }
}

/// 结构化日志字段的几种取值形态。
#[derive(Clone, Debug, PartialEq)]
pub enum LogField {
    String(String, String),
    /// A structured string array (for example zap.Strings partition names).
    Strings(String, Vec<String>),
    U64(String, u64),
    I64(String, i64),
    Bool(String, bool),
}

impl LogField {
    /// 返回字段键名。
    pub fn key(&self) -> &str {
        match self {
            Self::String(k, _)
            | Self::Strings(k, _)
            | Self::U64(k, _)
            | Self::I64(k, _)
            | Self::Bool(k, _) => k,
        }
    }
}

/// 一条已落盘/入内存的日志记录。
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// 记录时间。
    pub time: SystemTime,
    /// 级别。
    pub level: LogLevel,
    /// 消息正文。
    pub message: String,
    /// 附加字段。
    pub fields: Vec<LogField>,
}

/// 日志下沉目标：内存向量或追加写入文件。
#[derive(Debug)]
enum Sink {
    Memory(Vec<LogEntry>),
    File(String),
}

/// 按 (级别, 消息) 在 tick 窗口内只放行前 `first` 条的采样器。
#[derive(Debug)]
struct Sampler {
    tick: Duration,
    first: usize,
    counters: HashMap<(LogLevel, String), (Instant, usize)>,
}

/// 可克隆的 logger：共享 sink，可附加字段与采样器。
#[derive(Clone, Debug)]
pub struct Logger {
    sink: Arc<Mutex<Sink>>,
    level: Arc<RwLock<LogLevel>>,
    fields: Vec<LogField>,
    sampler: Option<Arc<Mutex<Sampler>>>,
    slow_log_encoding: bool,
}

/// 进程内全局日志族：后台、慢查询、General、error verbose。
#[derive(Clone, Debug)]
pub struct GlobalLoggers {
    /// 默认后台 logger。
    pub background: Logger,
    /// 慢查询 logger。
    pub slow_query: Logger,
    /// General Log logger。
    pub general: Logger,
    /// error verbose logger。
    pub error_verbose: Logger,
    /// 是否检测到 gRPC debug 环境变量。
    pub grpc_debug: bool,
}

/// 惰性初始化的全局 logger 锁。
fn globals() -> &'static RwLock<GlobalLoggers> {
    static GLOBALS: OnceLock<RwLock<GlobalLoggers>> = OnceLock::new();
    GLOBALS.get_or_init(|| {
        let logger = Logger::memory(LogLevel::Info);
        RwLock::new(GlobalLoggers {
            background: logger.clone(),
            slow_query: logger.clone(),
            general: logger.clone(),
            error_verbose: logger,
            grpc_debug: false,
        })
    })
}

impl Logger {
    /// 创建内存下沉 logger（便于测试读取 `entries`）。
    pub fn memory(level: LogLevel) -> Self {
        Self {
            sink: Arc::new(Mutex::new(Sink::Memory(Vec::new()))),
            level: Arc::new(RwLock::new(level)),
            fields: Vec::new(),
            sampler: None,
            slow_log_encoding: false,
        }
    }

    /// 创建追加写入指定路径的文件 logger。
    pub fn file(level: LogLevel, path: impl Into<String>) -> Self {
        Self {
            sink: Arc::new(Mutex::new(Sink::File(path.into()))),
            level: Arc::new(RwLock::new(level)),
            fields: Vec::new(),
            sampler: None,
            slow_log_encoding: false,
        }
    }

    /// 克隆 logger 并将文件输出切换为慢查询专用两行格式。
    pub(crate) fn with_slow_log_encoding(&self) -> Self {
        let mut logger = self.clone();
        logger.slow_log_encoding = true;
        logger
    }

    /// 克隆并追加固定字段。
    pub fn with_fields(&self, fields: impl IntoIterator<Item = LogField>) -> Self {
        let mut logger = self.clone();
        logger.fields.extend(fields);
        logger
    }

    /// 克隆并挂接采样器：每个 tick 窗口内同消息仅放行前 `first` 条。
    pub fn sample(&self, tick: Duration, first: usize) -> Self {
        let mut logger = self.clone();
        logger.sampler = Some(Arc::new(Mutex::new(Sampler {
            tick,
            first,
            counters: HashMap::new(),
        })));
        logger
    }

    /// 若为内存下沉则返回已记录条目，否则返回空向量。
    pub fn entries(&self) -> Vec<LogEntry> {
        match &*self.sink.lock().expect("logger sink poisoned") {
            Sink::Memory(entries) => entries.clone(),
            Sink::File(_) => Vec::new(),
        }
    }

    /// 按级别写一条日志；低于阈值或被采样丢弃则直接返回。
    pub fn log(
        &self,
        level: LogLevel,
        message: impl Into<String>,
        fields: impl IntoIterator<Item = LogField>,
    ) {
        if level < *self.level.read().expect("logger level lock poisoned") {
            return;
        }
        let message = message.into();
        if let Some(sampler) = &self.sampler {
            let mut sampler = sampler.lock().expect("logger sampler poisoned");
            let tick = sampler.tick;
            let first = sampler.first;
            let now = Instant::now();
            let counter = sampler
                .counters
                .entry((level, message.clone()))
                .or_insert((now, 0));
            // 超过 tick 则重置窗口计数
            if now.duration_since(counter.0) >= tick {
                *counter = (now, 0);
            }
            if counter.1 >= first {
                return;
            }
            counter.1 += 1;
        }
        let mut all_fields = self.fields.clone();
        all_fields.extend(fields);
        let entry = LogEntry {
            time: SystemTime::now(),
            level,
            message,
            fields: all_fields,
        };
        match &mut *self.sink.lock().expect("logger sink poisoned") {
            Sink::Memory(entries) => entries.push(entry),
            Sink::File(path) => {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    if self.slow_log_encoding {
                        let encoded = crate::slow_query_logger::SlowLogEncoder.encode(
                            entry.time,
                            &entry.message,
                            &entry.fields,
                        );
                        let _ = file.write_all(encoded.as_bytes());
                    } else {
                        let _ = writeln!(
                            file,
                            "[{:?}] {}{}",
                            entry.level,
                            entry.message,
                            Fields(&entry.fields)
                        );
                    }
                }
            }
        }
    }

    /// 写 Debug 级日志。
    pub fn debug(&self, message: impl Into<String>) {
        self.log(LogLevel::Debug, message, []);
    }
    /// 写 Info 级日志。
    pub fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message, []);
    }
    /// 写 Warn 级日志。
    pub fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message, []);
    }
    /// 写 Error 级日志。
    pub fn error(&self, message: impl Into<String>) {
        self.log(LogLevel::Error, message, []);
    }
}

/// 将字段列表格式化为 ` [k=v]` 序列，供文件行拼接。
struct Fields<'a>(&'a [LogField]);
impl fmt::Display for Fields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for field in self.0 {
            match field {
                LogField::String(k, v) => write!(f, " [{k}={v}]"),
                LogField::Strings(k, v) => write!(f, " [{k}={v:?}]"),
                LogField::U64(k, v) => write!(f, " [{k}={v}]"),
                LogField::I64(k, v) => write!(f, " [{k}={v}]"),
                LogField::Bool(k, v) => write!(f, " [{k}={v}]"),
            }?;
        }
        Ok(())
    }
}

/// 按配置创建单个 Logger；校验压缩算法并解析级别。
pub fn init_logger(cfg: &LogConfig) -> Result<Logger, String> {
    if !cfg.file.compression.is_empty() && cfg.file.compression != "gzip" {
        return Err(format!(
            "unsupported log compression {:?}",
            cfg.file.compression
        ));
    }
    let level = cfg.level.parse()?;
    Ok(if cfg.file.filename.is_empty() {
        Logger::memory(level)
    } else {
        Logger::file(level, &cfg.file.filename)
    })
}

/// 初始化全局后台/慢查询/General logger 并写入进程全局状态。
pub fn initialize_loggers(cfg: &LogConfig) -> Result<GlobalLoggers, String> {
    let background = init_logger(cfg)?;
    // 慢查询文件为空或与主文件相同时复用 background
    let slow_query = if cfg.slow_query_file.is_empty() || cfg.slow_query_file == cfg.file.filename {
        background.clone()
    } else {
        crate::slow_query_logger::new_slow_query_logger(cfg)?
    };
    let general = if cfg.general_log_file.is_empty() || cfg.general_log_file == cfg.file.filename {
        background.clone()
    } else {
        crate::general_logger::new_general_logger(cfg)?
    };
    let error_verbose = background.clone();
    let configured = GlobalLoggers {
        background,
        slow_query,
        general,
        error_verbose,
        grpc_debug: std::env::var_os(GRPCDebugEnvName).is_some_and(|value| !value.is_empty()),
    };
    *globals().write().expect("global logger lock poisoned") = configured.clone();
    Ok(configured)
}

/// Go 风格别名：`InitLogger`。
pub use initialize_loggers as InitLogger;
/// 用新配置替换全局 logger（语义同初始化）。
pub fn replace_logger(cfg: &LogConfig) -> Result<GlobalLoggers, String> {
    initialize_loggers(cfg)
}
/// Go 风格别名：`ReplaceLogger`。
pub use replace_logger as ReplaceLogger;
/// 解析级别字符串并更新全局 logger 阈值。
pub fn set_level(level: &str) -> Result<LogLevel, String> {
    let level = level.parse()?;
    let background = background_logger();
    *background
        .level
        .write()
        .expect("logger level lock poisoned") = level;
    Ok(level)
}
/// Go 风格别名：`SetLevel`。
pub use set_level as SetLevel;

/// 取得全局后台 logger 克隆。
pub fn background_logger() -> Logger {
    globals()
        .read()
        .expect("global logger lock poisoned")
        .background
        .clone()
}

/// 取得全局慢查询 logger 克隆。
pub fn slow_query_logger() -> Logger {
    globals()
        .read()
        .expect("global logger lock poisoned")
        .slow_query
        .clone()
}

/// 取得全局 General Log logger 克隆。
pub fn general_logger() -> Logger {
    globals()
        .read()
        .expect("global logger lock poisoned")
        .general
        .clone()
}

/// 取得全局 error verbose logger 克隆。
pub fn err_verbose_logger() -> Logger {
    globals()
        .read()
        .expect("global logger lock poisoned")
        .error_verbose
        .clone()
}

/// Go 风格别名：`BgLogger`。
pub use background_logger as BgLogger;
/// Go 风格别名：`ErrVerboseLogger`。
pub use err_verbose_logger as ErrVerboseLogger;

/// 会话追踪信息：连接 ID 与会话别名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceInfo {
    /// 连接 ID；0 表示不输出 conn 字段。
    pub connection_id: u64,
    /// 会话别名；空串表示不输出。
    pub session_alias: String,
}

/// 从 `TraceInfo` 生成 conn / session_alias 日志字段。
pub fn fields_from_trace_info(info: Option<&TraceInfo>) -> Vec<LogField> {
    let Some(info) = info else { return Vec::new() };
    let mut fields = Vec::with_capacity(2);
    if info.connection_id != 0 {
        fields.push(LogField::U64(LogFieldConn.into(), info.connection_id));
    }
    if !info.session_alias.is_empty() {
        fields.push(LogField::String(
            LogFieldSessionAlias.into(),
            info.session_alias.clone(),
        ));
    }
    fields
}

/// 在现有 logger 上叠加追踪字段。
pub fn logger_with_trace_info(logger: &Logger, info: Option<&TraceInfo>) -> Logger {
    logger.with_fields(fields_from_trace_info(info))
}

/// 携带 logger 的轻量上下文，便于链式附加字段。
#[derive(Clone, Debug)]
pub struct LogContext {
    logger: Logger,
}

impl LogContext {
    /// 用给定 logger 构造上下文。
    pub fn new(logger: Logger) -> Self {
        Self { logger }
    }
    /// 借用内部 logger。
    pub fn logger(&self) -> &Logger {
        &self.logger
    }
    /// 返回附加字段后的新上下文。
    pub fn with_fields(&self, fields: impl IntoIterator<Item = LogField>) -> Self {
        Self::new(self.logger.with_fields(fields))
    }
    /// 附加连接 ID 字段。
    pub fn with_conn_id(&self, id: u64) -> Self {
        self.with_fields([LogField::U64(LogFieldConn.into(), id)])
    }
    /// 附加会话别名字段。
    pub fn with_session_alias(&self, alias: impl Into<String>) -> Self {
        self.with_fields([LogField::String(LogFieldSessionAlias.into(), alias.into())])
    }
    /// 附加 category 字段。
    pub fn with_category(&self, category: impl Into<String>) -> Self {
        self.with_fields([LogField::String(LogFieldCategory.into(), category.into())])
    }
    /// 附加任意字符串键值字段。
    pub fn with_key_value(&self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.with_fields([LogField::String(key.into(), value.into())])
    }
}

/// Go 风格：`WithConnID`。
pub fn WithConnID(context: &LogContext, id: u64) -> LogContext {
    context.with_conn_id(id)
}

/// Go 风格：`WithSessionAlias`。
pub fn WithSessionAlias(context: &LogContext, alias: impl Into<String>) -> LogContext {
    context.with_session_alias(alias)
}

/// Go 风格：`WithCategory`。
pub fn WithCategory(context: &LogContext, category: impl Into<String>) -> LogContext {
    context.with_category(category)
}

/// Go 风格：`WithKeyValue`。
pub fn WithKeyValue(
    context: &LogContext,
    key: impl Into<String>,
    value: impl Into<String>,
) -> LogContext {
    context.with_key_value(key, value)
}

/// Go 风格：`WithTraceFields`。
pub fn WithTraceFields(context: &LogContext, info: Option<&TraceInfo>) -> LogContext {
    let Some(info) = info else {
        return context.with_fields([]);
    };
    context.with_fields([
        LogField::U64(LogFieldConn.into(), info.connection_id),
        LogField::String(LogFieldSessionAlias.into(), info.session_alias.clone()),
    ])
}

/// 简易追踪 span：事件列表与标签映射。
#[derive(Clone, Debug, Default)]
pub struct TraceSpan {
    events: Arc<Mutex<Vec<(String, String)>>>,
    tags: Arc<Mutex<HashMap<String, String>>>,
}

impl TraceSpan {
    /// 返回已记录事件副本。
    pub fn events(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .expect("trace event lock poisoned")
            .clone()
    }
    /// 返回已记录标签副本。
    pub fn tags(&self) -> HashMap<String, String> {
        self.tags.lock().expect("trace tag lock poisoned").clone()
    }
}

/// 向 span 追加一条 event 字段。
pub fn event(span: Option<&TraceSpan>, value: impl Into<String>) {
    if let Some(span) = span {
        span.events
            .lock()
            .expect("trace event lock poisoned")
            .push((TraceEventKey.into(), value.into()));
    }
}

/// 以 `fmt::Arguments` 格式化后写入 event。
pub fn eventf(span: Option<&TraceSpan>, formatted: fmt::Arguments<'_>) {
    event(span, formatted.to_string());
}

/// 设置或覆盖 span 上的标签。
pub fn set_tag(span: Option<&TraceSpan>, key: impl Into<String>, value: impl ToString) {
    if let Some(span) = span {
        span.tags
            .lock()
            .expect("trace tag lock poisoned")
            .insert(key.into(), value.to_string());
    }
}

pub use event as Event;
pub use eventf as Eventf;
pub use set_tag as SetTag;

/// 从自定义取值函数收集代理相关环境变量字段。
pub fn proxy_fields_from(mut get: impl FnMut(&str) -> Option<String>) -> Vec<LogField> {
    let mut fields = Vec::with_capacity(3);
    for (lower, upper) in [
        ("http_proxy", "HTTP_PROXY"),
        ("https_proxy", "HTTPS_PROXY"),
        ("no_proxy", "NO_PROXY"),
    ] {
        // `httpproxy.FromEnvironment` checks the canonical uppercase name
        // before its lowercase fallback when both are present.
        if let Some(value) = get(upper)
            .filter(|v| !v.is_empty())
            .or_else(|| get(lower).filter(|v| !v.is_empty()))
        {
            fields.push(LogField::String(lower.into(), value));
        }
    }
    fields
}

/// 从进程环境变量读取代理配置字段。
pub fn proxy_fields() -> Vec<LogField> {
    proxy_fields_from(|key| std::env::var(key).ok())
}

/// 若存在代理配置则写入一条 info 日志。
pub fn log_env_variables() {
    let fields = proxy_fields();
    if !fields.is_empty() {
        background_logger().log(LogLevel::Info, "using proxy config", fields);
    }
}

/// Go 风格别名：`LogEnvVariables`。
pub use log_env_variables as LogEnvVariables;

/// 构造采样 logger 工厂：固定字段 + sampled 标记 + tick 限流。
pub fn sample_logger_factory(
    logger: Logger,
    tick: Duration,
    first: usize,
    fields: Vec<LogField>,
) -> impl Fn() -> Logger {
    let sampled = logger
        .with_fields(fields)
        .with_fields([LogField::String("sampled".into(), String::new())])
        .sample(tick, first);
    move || sampled.clone()
}

/// error verbose 场景复用同一采样工厂。
pub fn sample_err_verbose_logger_factory(
    logger: Logger,
    tick: Duration,
    first: usize,
    fields: Vec<LogField>,
) -> impl Fn() -> Logger {
    sample_logger_factory(logger, tick, first, fields)
}
