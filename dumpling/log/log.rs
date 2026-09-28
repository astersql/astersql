// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Dumpling application logger wrapper, matching Go `dumpling/log`.
//!
//! Wraps a zap-style logger: package-level nop via [`Zap`], config-driven
//! [`InitAppLogger`] (delegating file permission checks to `pingcap/log`
//! semantics), [`NewAppLogger`], and [`ShortError`] field construction.
// Dumpling 应用日志封装，对齐 Go `dumpling/log` 包。
// 提供包级 nop 全局 logger、配置驱动的 InitAppLogger（文件权限检查语义同 pingcap/log）、
// NewAppLogger 包装与 ShortError 字段构造。

use std::fmt::{self, Display, Write as FmtWrite};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default single-file max size in MB (pingcap/log `defaultLogMaxSize`).
// 单日志文件默认上限 300MB，与 pingcap/log defaultLogMaxSize 一致。
const DEFAULT_LOG_MAX_SIZE: i32 = 300;

/// Config serializes log related config in toml/json.
// 日志配置结构，字段名与 Go toml/json 序列化标签一致（PascalCase）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Log level.
    /// One of "debug", "info", "warn", "error", "dpanic", "panic", and "fatal".
    // 日志级别：debug/info/warn/error/dpanic/panic/fatal，空串等同 info。
    pub Level: String,
    /// Log filename, leave empty to disable file log.
    // 日志文件路径；空串表示仅输出到 stdout，不写文件。
    pub File: String,
    /// Max size for a single file, in MB.
    // 单文件最大 MB；0 时 init_file_log 回落到 DEFAULT_LOG_MAX_SIZE。
    pub FileMaxSize: i32,
    /// Max log keep days, default is never deleting.
    // 日志保留天数；0 表示不自动删除（lumberjack 语义）。
    pub FileMaxDays: i32,
    /// Maximum number of old log files to retain.
    // 最多保留的旧日志文件数。
    pub FileMaxBackups: i32,
    /// Format of the log, one of `text`, `json` or `console`.
    // 输出格式：text/json；console 在 Go 侧存在，Rust 侧 parse_format 仅接受 text/json。
    pub Format: String,
}

/// Zap-compatible log level.
// zap 兼容日志级别枚举，顺序用于 enabled 比较（>= 阈值则输出）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    // 最详细诊断级别
    Debug,
    #[default]
    // 默认级别，空配置字符串解析为此项
    Info,
    // 警告，不影响主流程
    Warn,
    // 错误事件
    Error,
    // 开发环境 panic 阈值；InitAppLogger stacktrace 锚点
    DPanic,
    // 记录后 panic（zap 语义）
    Panic,
    // 记录后以状态码 1 退出进程（zap 语义）
    Fatal,
}

impl Level {
    /// Parse zap-style level text; empty string is Info (zap zero value).
    // 解析 zap 级别文本；空串视为 Info（zap 零值语义）。
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            // zap 零值
            "" | "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "dpanic" => Ok(Self::DPanic),
            "panic" => Ok(Self::Panic),
            "fatal" => Ok(Self::Fatal),
            // Go Level.UnmarshalText 同类错误
            _ => Err(format!("unrecognized level: {text:?}")),
        }
    }

    // 输出用大写级别名，与 zap encoder 文本格式一致。
    fn capital(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::DPanic => "DPANIC",
            Self::Panic => "PANIC",
            Self::Fatal => "FATAL",
        }
    }
}

// Field 构造与 skip 语义 ----------------------------------------

/// Structured field; `skip` mirrors `zap.Skip()`.
// 结构化日志字段；skip=true 时等价 Go zap.Skip()，with/log 时过滤。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    // 字段键，如 "error"
    pub key: String,
    // 字符串化值
    pub value: String,
    // true 时不参与 with/log 输出
    skip: bool,
}

impl Field {
    // 构造被跳过的占位字段，ShortError(None) 使用。
    pub fn skip() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            skip: true,
        }
    }

    // 构造 key/value 字符串字段。
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            skip: false,
        }
    }

    pub fn is_skip(&self) -> bool {
        self.skip
    }
}

// InitLogger 返回的属性块，字段 PascalCase 与 Go pingcap/log 配置 JSON 对齐。

/// Records logger wiring info returned by InitLogger (Core/Syncer/Level).
// InitAppLogger 返回的属性快照，供调用方读取实际生效的配置。
#[derive(Clone, Debug)]
pub struct ZapProperties {
    // 解析后的生效级别
    pub Level: Level,
    // 空表示 stdout 模式
    pub Filename: String,
    // text 或 json
    pub Format: String,
    // 可能已由 0 替换为 300
    pub FileMaxSize: i32,
    // 保留天数配置快照
    pub FileMaxDays: i32,
    // 备份数配置快照
    pub FileMaxBackups: i32,
}

// 日志输出目的地：nop/stdout/文件/内存（测试捕获）。
#[derive(Debug)]
enum Sink {
    // 丢弃所有输出，包级 Zap 默认
    Nop,
    // File 配置为空时 InitAppLogger 选用
    Stdout,
    // 按配置追加并轮转指定路径，对齐 pingcap/log 使用的 lumberjack。
    File(FileSink),
    // 测试捕获，entries() 读取
    Memory(Vec<String>),
}

/// File sink with the size/retention behavior configured by dumpling.
#[derive(Debug)]
struct FileSink {
    path: PathBuf,
    max_bytes: i64,
    max_days: i32,
    max_backups: i32,
    initialized: bool,
}

impl FileSink {
    fn new(path: String, max_size: i32, max_days: i32, max_backups: i32) -> Self {
        Self {
            path: PathBuf::from(path),
            max_bytes: i64::from(max_size) * 1024 * 1024,
            max_days,
            max_backups,
            initialized: false,
        }
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let write_len = i64::try_from(line.len() + 1).unwrap_or(i64::MAX);
        // lumberjack rejects a single write larger than MaxSize.
        if write_len > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "write length {write_len} exceeds maximum file size {}",
                    self.max_bytes
                ),
            ));
        }

        if !self.initialized {
            // lumberjack runs retention once when lazily opening the sink.
            self.remove_expired_backups()?;
        }
        let current_size = match fs::metadata(&self.path) {
            Ok(metadata) => i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err),
        };
        if current_size + write_len > self.max_bytes
            || (!self.initialized && current_size + write_len == self.max_bytes)
        {
            self.rotate()?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        self.initialized = true;
        Ok(())
    }

    fn rotate(&self) -> std::io::Result<()> {
        if let Ok(metadata) = fs::metadata(&self.path) {
            let permissions = metadata.permissions();
            let backup = self.next_backup_path();
            fs::rename(&self.path, backup)?;
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?;
            drop(file);
            fs::set_permissions(&self.path, permissions)?;
        }
        self.remove_expired_backups()
    }

    fn next_backup_path(&self) -> PathBuf {
        let filename = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log");
        let extension = self
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        let stem = filename.strip_suffix(&extension).unwrap_or(filename);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.path
            .with_file_name(format!("{stem}-{timestamp}{extension}"))
    }

    fn remove_expired_backups(&self) -> std::io::Result<()> {
        if self.max_backups <= 0 && self.max_days <= 0 {
            return Ok(());
        }
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        let filename = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log");
        let extension = self
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        let stem = filename.strip_suffix(&extension).unwrap_or(filename);
        let prefix = format!("{stem}-");

        let mut backups = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                if !name.starts_with(&prefix) || !name.ends_with(&extension) {
                    return None;
                }
                let metadata = entry.metadata().ok()?;
                if !metadata.is_file() {
                    return None;
                }
                Some((
                    entry.path(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ))
            })
            .collect::<Vec<_>>();
        backups.sort_by(|left, right| right.1.cmp(&left.1));

        let max_age =
            (self.max_days > 0).then(|| Duration::from_secs(self.max_days as u64 * 24 * 60 * 60));
        let now = SystemTime::now();
        for (index, (path, modified)) in backups.into_iter().enumerate() {
            let exceeds_count = self.max_backups > 0 && index >= self.max_backups as usize;
            let exceeds_age = max_age.is_some_and(|age| {
                now.duration_since(modified)
                    .is_ok_and(|elapsed| elapsed > age)
            });
            if exceeds_count || exceeds_age {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

// 内部 logger 状态：级别、格式、栈追踪阈值、固定字段、可变 sink。
struct Inner {
    // 最低输出级别阈值
    level: Level,
    // text/json，决定文本行或 JSON 对象编码
    format: String,
    // >= 此级别时在行尾附加 caller 位置
    stacktrace_at: Level,
    // with() 附加的固定字段
    fields: Vec<Field>,
    // 可变输出目的地；派生 logger 与原 logger 共享同一 zap WriteSyncer 语义
    sink: Arc<Mutex<Sink>>,
}

/// Inner zap-style logger (Go `*zap.Logger`).
// 内层 zap 风格 logger，对应 Go *zap.Logger；Clone 共享 Arc<Inner>。
#[derive(Clone)]
pub struct ZapLogger {
    inner: Arc<Inner>,
}

impl fmt::Debug for ZapLogger {
    // Debug 仅暴露 level/format，避免打印 sink 内文件路径或内存缓冲。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZapLogger")
            .field("level", &self.inner.level)
            .field("format", &self.inner.format)
            .finish_non_exhaustive()
    }
}

impl ZapLogger {
    // 构造内层 logger；sink 初始为空或由调用方指定。
    fn new(level: Level, format: String, sink: Sink, stacktrace_at: Level) -> Self {
        Self {
            inner: Arc::new(Inner {
                level,
                format,
                stacktrace_at,
                fields: Vec::new(),
                sink: Arc::new(Mutex::new(sink)),
            }),
        }
    }

    /// Nop logger (`zap.NewNop()`).
    // 空操作 logger，对应 zap.NewNop()；默认 stacktrace_at=Error。
    pub fn nop() -> Self {
        Self::new(Level::Info, "text".to_owned(), Sink::Nop, Level::Error)
    }

    /// Attach stacktrace capture starting at `level` (Go `zap.AddStacktrace`).
    // 设置栈追踪捕获阈值，对应 Go zap.AddStacktrace(level)。
    pub fn with_stacktrace_at(self, level: Level) -> Self {
        Self {
            inner: Arc::new(Inner {
                level: self.inner.level,
                format: self.inner.format.clone(),
                stacktrace_at: level,
                fields: self.inner.fields.clone(),
                // zap.WithOptions preserves the core and its WriteSyncer.
                sink: Arc::clone(&self.inner.sink),
            }),
        }
    }

    /// Clone with extra fixed fields.
    // 克隆并附加固定字段；skip 字段被过滤，与 Go With 行为一致。
    pub fn with(&self, fields: impl IntoIterator<Item = Field>) -> Self {
        let mut all = self.inner.fields.clone();
        all.extend(fields.into_iter().filter(|f| !f.skip));
        Self {
            inner: Arc::new(Inner {
                level: self.inner.level,
                format: self.inner.format.clone(),
                stacktrace_at: self.inner.stacktrace_at,
                fields: all,
                // zap.With keeps the same core/WriteSyncer while adding fields.
                sink: Arc::clone(&self.inner.sink),
            }),
        }
    }

    /// Stacktrace threshold applied by InitAppLogger.
    // 返回当前 stacktrace 阈值；InitAppLogger 固定设为 DPanic。
    pub fn stacktrace_at(&self) -> Level {
        self.inner.stacktrace_at
    }

    /// Enabled check for a level.
    // 判断给定级别是否应输出（level >= 配置的最低级别）。
    pub fn enabled(&self, level: Level) -> bool {
        level >= self.inner.level
    }

    /// Captured in-memory lines (test helper); empty for file/stdout/nop.
    // 测试辅助：读取 Memory sink 捕获的日志行；其他 sink 返回空 Vec。
    pub fn entries(&self) -> Vec<String> {
        match &*self.inner.sink.lock().expect("logger sink poisoned") {
            Sink::Memory(entries) => entries.clone(),
            _ => Vec::new(),
        }
    }

    #[track_caller]
    // 核心写日志路径：级别过滤 → 拼行 → 可选 stack → 写入 sink。
    fn log(&self, level: Level, message: &str, fields: impl IntoIterator<Item = Field>) {
        if !self.enabled(level) {
            return;
        }
        let fields: Vec<_> = self
            .inner
            .fields
            .iter()
            .cloned()
            .chain(fields.into_iter().filter(|f| !f.skip))
            .collect();
        let mut line = if self.inner.format == "json" {
            let mut line = format!(
                "{{\"level\":\"{}\",\"message\":\"{}\"",
                level.capital(),
                json_escape(message)
            );
            for field in &fields {
                let _ = write!(
                    &mut line,
                    ",\"{}\":\"{}\"",
                    json_escape(&field.key),
                    json_escape(&field.value)
                );
            }
            line.push('}');
            line
        } else {
            let mut line = String::new();
            let _ = write!(&mut line, "[{}] {}", level.capital(), message);
            for field in &fields {
                let _ = write!(&mut line, " {}={}", field.key, field.value);
            }
            line
        };
        if level >= self.inner.stacktrace_at {
            let location = std::panic::Location::caller().to_string();
            if self.inner.format == "json" {
                line.pop();
                let _ = write!(&mut line, ",\"stack\":\"{}\"}}", json_escape(&location));
            } else {
                let _ = write!(&mut line, " stack={location}");
            }
        }
        match &mut *self.inner.sink.lock().expect("logger sink poisoned") {
            Sink::Nop => {}
            Sink::Stdout => {
                // 忽略 stdout 写入错误，与 Go zap Sync 部分失败语义类似。
                let _ = writeln!(std::io::stdout(), "{line}");
            }
            Sink::File(file_sink) => {
                // zap 的日志方法不返回 sink 错误；写入/轮转失败时保持同一调用契约。
                let _ = file_sink.write_line(&line);
            }
            Sink::Memory(entries) => entries.push(line),
        }
    }

    #[track_caller]
    // Debug 级别日志，PascalCase 方法名对齐 Go zap.Logger.Debug。
    pub fn Debug(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Debug, message, fields);
    }

    #[track_caller]
    // Info 级别，Dumpling 常规运行信息。
    pub fn Info(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Info, message, fields);
    }

    #[track_caller]
    // Warn 级别，可恢复异常。
    pub fn Warn(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Warn, message, fields);
    }

    #[track_caller]
    // Error 级别，需关注的失败事件。
    pub fn Error(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Error, message, fields);
    }

    #[track_caller]
    // 生产 logger 的 DPanic 只记录；仅 zap development 模式会触发 panic。
    pub fn DPanic(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::DPanic, message, fields);
    }

    #[track_caller]
    // zap Panic 在写入日志后触发 unwinding。
    pub fn Panic(&self, message: &str, fields: impl IntoIterator<Item = Field>) -> ! {
        self.log(Level::Panic, message, fields);
        panic!("{message}");
    }

    #[track_caller]
    // zap Fatal 在同步写入日志后以状态码 1 终止进程。
    pub fn Fatal(&self, message: &str, fields: impl IntoIterator<Item = Field>) -> ! {
        self.log(Level::Fatal, message, fields);
        std::process::exit(1);
    }

    /// Build an in-memory capturing logger for tests.
    // 构造 Memory sink 的测试 logger，便于断言日志内容。
    pub fn capture(level: Level) -> Self {
        Self::new(
            level,
            "text".to_owned(),
            Sink::Memory(Vec::new()),
            Level::Error,
        )
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            character if character.is_control() => {
                let _ = write!(&mut escaped, "\\u{:04x}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// Logger wraps the zap logger.
// 对外 Logger 包装，内嵌 ZapLogger；方法名 PascalCase 对齐 Go 导出 API。
#[derive(Clone, Debug)]
pub struct Logger {
    /// Inner zap logger (Go embedded `*zap.Logger`).
    // 公开内层字段名 Logger，与 Go 嵌入字段访问习惯一致
    pub Logger: ZapLogger,
}

impl Logger {
    /// Nop wrapper.
    // 对外 Nop Logger，委托内层 ZapLogger::nop。
    pub fn nop() -> Self {
        Self {
            Logger: ZapLogger::nop(),
        }
    }

    #[track_caller]
    // 透传至内层 ZapLogger，保持 Go 嵌入 *zap.Logger 的方法集。
    pub fn Debug(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.Logger.Debug(message, fields);
    }

    #[track_caller]
    pub fn Info(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.Logger.Info(message, fields);
    }

    #[track_caller]
    pub fn Warn(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.Logger.Warn(message, fields);
    }

    #[track_caller]
    pub fn Error(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.Logger.Error(message, fields);
    }

    #[track_caller]
    pub fn DPanic(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.Logger.DPanic(message, fields);
    }

    #[track_caller]
    pub fn Panic(&self, message: &str, fields: impl IntoIterator<Item = Field>) -> ! {
        self.Logger.Panic(message, fields);
    }

    #[track_caller]
    pub fn Fatal(&self, message: &str, fields: impl IntoIterator<Item = Field>) -> ! {
        self.Logger.Fatal(message, fields);
    }

    // 返回带附加字段的新 Logger，不修改 self。
    pub fn with(&self, fields: impl IntoIterator<Item = Field>) -> Self {
        Self {
            Logger: self.Logger.with(fields),
        }
    }

    pub fn stacktrace_at(&self) -> Level {
        self.Logger.stacktrace_at()
    }

    // 测试读取内层捕获条目。
    pub fn entries(&self) -> Vec<String> {
        self.Logger.entries()
    }
}

// 包级全局 logger 槽位；Go appLogger 变量，Rust 用 OnceLock 惰性初始化 nop。
fn app_logger() -> &'static Logger {
    static APP: OnceLock<Logger> = OnceLock::new();
    // 首次访问安装 nop，无 SetGlobal API
    APP.get_or_init(Logger::nop)
}

/// Zap returns the global logger.
// 返回包级全局 logger；Go 侧为 Zap()，默认 nop 且 InitAppLogger 不替换它。
pub fn Zap() -> Logger {
    app_logger().clone()
}

/// Validate format like pingcap/log `NewTextEncoder`.
// 校验日志格式，语义同 pingcap/log NewTextEncoder；不支持格式返回 Go 风格错误文本。
fn parse_format(format: &str) -> Result<String, String> {
    match format {
        // 空 format 同 text
        "text" | "" => Ok("text".to_owned()),
        "json" => Ok("json".to_owned()),
        // Go NewTextEncoder 错误文案
        other => Err(format!("unsupport log format: {other}")),
    }
}

/// Map OS permission errors to Go-style lowercase `"permission denied"` text.
// 将 OS 权限错误映射为小写 "permission denied"，Go syscall.Errno 测试断言依赖此子串。
fn io_err_display(err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        // Go `syscall.Errno.Error()` is lowercase; dumpling tests assert this substring.
        // 刻意小写以通过 TestInitLogNoPermission 子串匹配。
        "permission denied".to_owned()
    } else {
        // 非权限错误保留 Rust/OS 原始描述。
        err.to_string()
    }
}

/// Eager file permission checks matching pingcap/log `initFileLog`.
// 文件日志预检：建目录、拒绝目录作文件名、探针写权限、创建后删除空文件（lumberjack 接管）。
fn init_file_log(
    filename: &str,
    max_size: i32,
    max_days: i32,
    max_backups: i32,
) -> Result<(String, i32, i32, i32), String> {
    let path = Path::new(filename);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // 先 ensure 父目录存在，失败时包装 cannot create log directory。
    fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create log directory: {}", io_err_display(&err)))?;

    match fs::metadata(filename) {
        Ok(meta) if meta.is_dir() => {
            // Go initFileLog 同样拒绝目录路径。
            return Err("can't use directory as log file name".to_owned());
        }
        Ok(_) => {
            // 文件已存在：探针 append 写权限。
            let file = OpenOptions::new()
                .write(true)
                .append(true)
                .open(filename)
                .map_err(|err| format!("can't write to log file: {}", io_err_display(&err)))?;
            drop(file);
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // 不存在则 create 验证可写，随后删除供 lumberjack 首次写入创建。
            File::create(filename)
                .map_err(|err| format!("can't create log file: {}", io_err_display(&err)))?;
            // Remove the empty file since lumberjack will create it.
            // 删除探针空文件，实际轮转由 lumberjack 在首次写入时创建。
            let _ = fs::remove_file(filename);
        }
        Err(err) => {
            // 其他 stat 错误统一 error checking log file 前缀。
            return Err(format!("error checking log file: {}", io_err_display(&err)));
        }
    }

    // FileMaxSize==0 回落默认 300MB，与 pingcap/log 一致。
    let max_size = if max_size == 0 {
        DEFAULT_LOG_MAX_SIZE
    } else {
        max_size
    };
    Ok((filename.to_owned(), max_size, max_days, max_backups))
}

/// InitAppLogger inits the wrapped logger from config.
// 从 Config 初始化 logger 并返回 (Logger, ZapProperties)；不修改包级 Zap() nop。
pub fn InitAppLogger(cfg: &Config) -> Result<(Logger, ZapProperties), String> {
    // pingcap/log initializes and validates the output before parsing level/format.
    let file_config = if cfg.File.is_empty() {
        None
    } else {
        Some(init_file_log(
            &cfg.File,
            cfg.FileMaxSize,
            cfg.FileMaxDays,
            cfg.FileMaxBackups,
        )?)
    };
    let level = Level::parse(&cfg.Level)?;
    let format = parse_format(&cfg.Format)?;

    let (sink, filename, file_max_size, file_max_days, file_max_backups) =
        if let Some((filename, max_size, max_days, max_backups)) = file_config {
            (
                Sink::File(FileSink::new(
                    filename.clone(),
                    max_size,
                    max_days,
                    max_backups,
                )),
                filename,
                max_size,
                max_days,
                max_backups,
            )
        } else {
            // 无 File：stdout sink，Filename 留空写入 props。
            (
                Sink::Stdout,
                String::new(),
                cfg.FileMaxSize,
                cfg.FileMaxDays,
                cfg.FileMaxBackups,
            )
        };

    let props = ZapProperties {
        Level: level,
        Filename: filename,
        Format: format.clone(),
        FileMaxSize: file_max_size,
        FileMaxDays: file_max_days,
        FileMaxBackups: file_max_backups,
    };
    // props 供调用方读取实际 sink/轮转参数，不持有 logger 句柄

    // Go: logger.WithOptions(zap.AddStacktrace(zap.DPanicLevel))
    // InitAppLogger 固定 AddStacktrace(DPanicLevel)，与 Go 一致。
    let zap_logger = ZapLogger::new(level, format, sink, Level::DPanic);
    // 返回新 logger 实例，不写回全局 APP
    Ok((Logger { Logger: zap_logger }, props))
}

/// NewAppLogger returns the wrapped logger from config.
// 将已有 ZapLogger 包装为 Logger，对应 Go NewAppLogger。
pub fn NewAppLogger(logger: ZapLogger) -> Logger {
    Logger { Logger: logger }
}

/// ShortError constructs a field which only records the error message without the
/// verbose text (i.e. excludes the stack trace).
// 构造仅含 error 消息的字段，不含堆栈；err=None 时返回 skip 字段。
pub fn ShortError(err: Option<&dyn Display>) -> Field {
    match err {
        // 无错误时不占日志字段
        None => Field::skip(),
        // 仅消息，不含 stack
        Some(err) => Field::string("error", err.to_string()),
    }
}
