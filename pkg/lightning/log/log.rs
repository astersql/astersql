// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Lightning 应用日志：Logger、配置初始化与任务起止计时。
//
// 对应 Go 侧 zap 风格封装：全局 Logger、按包路径过滤诊断噪音、
// `Begin`/`End` 记录任务耗时，以及上下文取消错误的识别。

use std::error::Error as StdError;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::filter::{Core, Entry, Field, FilterCore, Level, encode_json};

/// 默认日志级别字符串。
const defaultLogLevel: &str = "info";
/// 默认日志文件保留天数。
const defaultLogMaxDays: i32 = 7;
/// 默认单文件最大体积（MB 量级，与 Go 配置语义对齐）。
const defaultLogMaxSize: i32 = 512;

/// 日志配置：级别、文件路径、滚动参数与诊断开关。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// 日志级别字符串（如 info、warn）。
    pub Level: String,
    /// 日志文件路径；空或 `"-"` 表示标准输出。
    pub File: String,
    /// 单文件最大大小。
    pub FileMaxSize: i32,
    /// 文件最长保留天数。
    pub FileMaxDays: i32,
    /// 最多保留备份数。
    pub FileMaxBackups: i32,
    /// 为真时关闭包路径过滤并开启 gRPC 调试环境变量。
    pub EnableDiagnoseLogs: bool,
}

impl Config {
    /// 填充空缺默认值，并将 `"warning"` 规范为 `"warn"`。
    pub fn Adjust(&mut self) {
        if self.Level.is_empty() {
            self.Level = defaultLogLevel.to_owned();
        }
        if self.Level == "warning" {
            self.Level = "warn".to_owned();
        }
        if self.FileMaxSize == 0 {
            self.FileMaxSize = defaultLogMaxSize;
        }
        if self.FileMaxDays == 0 {
            self.FileMaxDays = defaultLogMaxDays;
        }
    }
}

/// 面向调用方的 Logger：包装 `Core`，提供级别化写日志与任务 Begin。
#[derive(Clone)]
pub struct Logger {
    core: Arc<dyn Core>,
    level: Option<Arc<RwLock<Level>>>,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger").finish_non_exhaustive()
    }
}

impl Logger {
    /// 用给定 Core 包装为 Logger。
    pub fn Wrap(core: Arc<dyn Core>) -> Self {
        Self { core, level: None }
    }

    /// 返回内部 Core 的克隆。
    pub fn core(&self) -> Arc<dyn Core> {
        self.core.clone()
    }

    /// 写一条日志：未启用该级别则直接返回；调用方取自 `#[track_caller]`。
    #[track_caller]
    fn log(&self, level: Level, message: &str, fields: impl IntoIterator<Item = Field>) {
        if !self.core.enabled(level) {
            return;
        }
        let caller = std::panic::Location::caller();
        let entry = Entry::new(level, message).with_caller(caller.file());
        let _ = self.core.write(entry, fields.into_iter().collect());
    }

    /// 写 Debug 级别日志。
    #[track_caller]
    pub fn Debug(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Debug, message, fields);
    }

    /// 写 Info 级别日志。
    #[track_caller]
    pub fn Info(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Info, message, fields);
    }

    /// 写 Warn 级别日志。
    #[track_caller]
    pub fn Warn(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Warn, message, fields);
    }

    /// 写 Error 级别日志。
    #[track_caller]
    pub fn Error(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Error, message, fields);
    }

    /// 返回附加了固定字段的新 Logger。
    pub fn With(&self, fields: impl IntoIterator<Item = Field>) -> Self {
        Self {
            core: self.core.with(fields.into_iter().collect()),
            level: self.level.clone(),
        }
    }

    /// 返回带层级名称的新 Logger。
    pub fn Named(&self, name: &str) -> Self {
        Self {
            core: self.core.named(name),
            level: self.level.clone(),
        }
    }

    /// 开始命名任务：立即打 `"… start"`，返回用于 `End` 的计时句柄。
    pub fn Begin(&self, level: Level, name: &str) -> Task {
        self.log(level, &(name.to_owned() + " start"), []);
        Task {
            Logger: self.clone(),
            level,
            name: name.to_owned(),
            since: Instant::now(),
        }
    }
}

/// Go 风格自由函数：等价于 `Logger::Wrap`。
pub fn Wrap(core: Arc<dyn Core>) -> Logger {
    Logger::Wrap(core)
}

/// 实际写出到 stdout/文件/丢弃的 Core 实现。
#[derive(Debug)]
struct OutputCore {
    /// 输出目的地。
    destination: Destination,
    /// 最低启用级别。
    level: Arc<RwLock<Level>>,
    /// 固定附加字段。
    fields: Vec<Field>,
    /// Logger 名称。
    name: String,
}

/// 日志输出目的地。
#[derive(Debug)]
enum Destination {
    Stdout,
    File(String),
    Discard,
}

impl Core for OutputCore {
    fn enabled(&self, level: Level) -> bool {
        level >= *self.level.read().expect("output level lock poisoned")
    }

    fn with(&self, fields: Vec<Field>) -> Arc<dyn Core> {
        let mut combined = self.fields.clone();
        combined.extend(fields);
        Arc::new(Self {
            destination: match &self.destination {
                Destination::Stdout => Destination::Stdout,
                Destination::File(path) => Destination::File(path.clone()),
                Destination::Discard => Destination::Discard,
            },
            level: self.level.clone(),
            fields: combined,
            name: self.name.clone(),
        })
    }

    fn named(&self, name: &str) -> Arc<dyn Core> {
        let mut cloned = Self {
            destination: match &self.destination {
                Destination::Stdout => Destination::Stdout,
                Destination::File(path) => Destination::File(path.clone()),
                Destination::Discard => Destination::Discard,
            },
            level: self.level.clone(),
            fields: self.fields.clone(),
            name: self.name.clone(),
        };
        // 层级名用 `.` 拼接，对齐 zap Named 语义。
        if !cloned.name.is_empty() {
            cloned.name.push('.');
        }
        cloned.name.push_str(name);
        Arc::new(cloned)
    }

    fn write(&self, mut entry: Entry, fields: Vec<Field>) -> Result<(), String> {
        entry.logger_name = self.name.clone();
        let line = encode_json(&entry, self.fields.iter().cloned().chain(fields));
        match &self.destination {
            Destination::Stdout => println!("{line}"),
            Destination::File(path) => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|error| error.to_string())?;
                writeln!(file, "{line}").map_err(|error| error.to_string())?;
            }
            Destination::Discard => {}
        }
        Ok(())
    }
}

/// 构造丢弃所有输出的空操作 Logger。
fn nop_logger() -> Logger {
    Logger::Wrap(Arc::new(OutputCore {
        destination: Destination::Discard,
        level: Arc::new(RwLock::new(Level::Debug)),
        fields: Vec::new(),
        name: String::new(),
    }))
}

/// 进程级全局 Logger 的惰性初始化存储。
fn app_logger() -> &'static RwLock<Logger> {
    static LOGGER: OnceLock<RwLock<Logger>> = OnceLock::new();
    LOGGER.get_or_init(|| RwLock::new(nop_logger()))
}

/// 进程级当前日志级别的惰性初始化存储。
fn app_level() -> &'static RwLock<Level> {
    static LEVEL: OnceLock<RwLock<Level>> = OnceLock::new();
    LEVEL.get_or_init(|| RwLock::new(Level::Info))
}

/// 按配置初始化全局 Logger；诊断关闭时套上包路径 FilterCore。
pub fn InitLogger(cfg: &Config, _unused: &str) -> Result<(), String> {
    if cfg.EnableDiagnoseLogs {
        // Logger initialization runs before worker threads read this process flag.
        unsafe { std::env::set_var("GRPC_DEBUG", "true") };
    }
    let level = Level::parse(&cfg.Level)?;
    // 空文件名或 "-" 走 stdout；否则校验路径不是目录并预创建文件。
    let destination = if cfg.File.is_empty() {
        Destination::Stdout
    } else if cfg.File == "-" {
        Destination::Stdout
    } else {
        if fs::metadata(&cfg.File).is_ok_and(|metadata| metadata.is_dir()) {
            return Err("can't use directory as log file name".to_owned());
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&cfg.File)
            .map_err(|error| error.to_string())?;
        Destination::File(cfg.File.clone())
    };
    let level_controller = Arc::new(RwLock::new(level));
    let base: Arc<dyn Core> = Arc::new(OutputCore {
        destination,
        level: level_controller.clone(),
        fields: Vec::new(),
        name: String::new(),
    });
    // 非诊断模式：仅转发白名单包路径相关日志，降低噪音。
    let core: Arc<dyn Core> = if cfg.EnableDiagnoseLogs {
        base
    } else {
        Arc::new(FilterCore::new(
            base,
            [
                "github.com/pingcap/tidb/br/",
                "/lightning/",
                "/ingestor/ingestctrl",
                "main.main",
                "github.com/tikv/pd/client",
            ],
        ))
    };
    *app_logger().write().expect("app logger lock poisoned") = Logger {
        core,
        level: Some(level_controller),
    };
    *app_level().write().expect("app level lock poisoned") = level;
    Ok(())
}

/// 替换全局应用 Logger。
pub fn SetAppLogger(logger: &Logger) {
    *app_logger().write().expect("app logger lock poisoned") = logger.clone();
}

/// 获取全局应用 Logger 的克隆。
pub fn L() -> Logger {
    app_logger()
        .read()
        .expect("app logger lock poisoned")
        .clone()
}

/// 获取当前全局日志级别。
pub fn Level() -> Level {
    *app_level().read().expect("app level lock poisoned")
}

/// 设置全局日志级别并返回旧值。
pub fn SetLevel(level: Level) -> Level {
    let mut current = app_level().write().expect("app level lock poisoned");
    let old = *current;
    *current = level;
    if let Some(controller) = &app_logger().read().expect("app logger lock poisoned").level {
        *controller.write().expect("output level lock poisoned") = level;
    }
    old
}

/// 将错误转为 `error` 字段；无错误时返回 skip 字段。
pub fn ShortError(err: Option<&(dyn StdError + 'static)>) -> Field {
    err.map_or_else(Field::skip, |error| {
        Field::string("error", error.to_string())
    })
}

/// 在全局 Logger 上附加字段。
pub fn With(fields: impl IntoIterator<Item = Field>) -> Logger {
    L().With(fields)
}

/// 表示各类“已取消”错误，供 `IsContextCanceledError` 识别。
#[derive(Debug)]
pub enum CancellationError {
    ContextCanceled,
    GrpcCanceled,
    SmithyCanceled,
    Operation(Box<CancellationError>),
}

impl fmt::Display for CancellationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContextCanceled => f.write_str("context canceled"),
            Self::GrpcCanceled => f.write_str("rpc error: code = Canceled"),
            Self::SmithyCanceled => f.write_str("request canceled"),
            Self::Operation(source) => write!(f, "operation error: {source}"),
        }
    }
}

impl StdError for CancellationError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Operation(source) => Some(source),
            _ => None,
        }
    }
}

/// 沿错误链查找是否存在 `CancellationError`（上下文/RPC 取消）。
pub fn IsContextCanceledError(err: Option<&(dyn StdError + 'static)>) -> bool {
    let mut current = err;
    while let Some(error) = current {
        if error.downcast_ref::<CancellationError>().is_some() {
            return true;
        }
        current = error.source();
    }
    false
}

/// 任务计时句柄：由 `Begin` 创建，`End`/`End2` 结束并打日志。
pub struct Task {
    /// 用于结束时写日志的 Logger。
    pub Logger: Logger,
    /// Begin 时选用的级别（成功结束时复用）。
    level: Level,
    /// 任务名。
    name: String,
    /// 起始时刻。
    since: Instant,
}

/// 以 Info 级别开始命名任务。
pub fn BeginTask(logger: &Logger, name: &str) -> Task {
    logger.Begin(Level::Info, name)
}

impl Task {
    /// 结束任务：成功/取消/失败选择不同文案与级别，并附上耗时与错误字段。
    pub fn End(
        &self,
        mut level: Level,
        err: Option<&(dyn StdError + 'static)>,
        extra_fields: impl IntoIterator<Item = Field>,
    ) -> Duration {
        let elapsed = self.since.elapsed();
        // 无错误用 Begin 级别；取消降为 Debug；其它失败用调用方传入级别。
        let (verb, mut fields) = if err.is_none() {
            level = self.level;
            (" completed", extra_fields.into_iter().collect::<Vec<_>>())
        } else if IsContextCanceledError(err) {
            level = Level::Debug;
            (" canceled", Vec::new())
        } else {
            (" failed", Vec::new())
        };
        fields.push(Field::duration("takeTime", elapsed));
        fields.push(ShortError(err));
        self.Logger.log(level, &(self.name.clone() + verb), fields);
        elapsed
    }

    /// 简化版结束：仅区分成功与失败，取消也当失败处理。
    pub fn End2(
        &self,
        level: Level,
        err: Option<&(dyn StdError + 'static)>,
        extra_fields: impl IntoIterator<Item = Field>,
    ) -> Duration {
        let elapsed = self.since.elapsed();
        let (level, verb, mut fields) = match err {
            None => (
                self.level,
                " completed",
                extra_fields.into_iter().collect::<Vec<_>>(),
            ),
            Some(_) => (level, " failed", Vec::new()),
        };
        fields.push(Field::duration("takeTime", elapsed));
        fields.push(ShortError(err));
        self.Logger.log(level, &(self.name.clone() + verb), fields);
        elapsed
    }
}
