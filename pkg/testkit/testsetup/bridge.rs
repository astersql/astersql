// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 测试公共初始化桥接：按环境变量配置全局日志级别。
//
// 对齐 Go 侧 `applyOSLogLevel` / `SetupForCommonTest`：读取 `log_level`，
// 安装一次 stderr 文本 logger，并支持 zap 风格的级别名（含 dpanic/panic/fatal）。

use std::fmt;
use std::io::Write;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use log::{LevelFilter, Log, Metadata, Record};

/// 进程内唯一的文本日志实现实例。
static LOGGER: TextLogger = TextLogger;
/// 保证 `log::set_logger` 只成功安装一次。
static INSTALL_LOGGER: Once = Once::new();
/// 当前生效的日志级别（以 LevelFilter 判别值存储）。
static CONFIGURED_LEVEL: AtomicUsize = AtomicUsize::new(LevelFilter::Info as usize);

/// 将日志写到 stderr 的简易 facade，级别由 `CONFIGURED_LEVEL` 控制。
struct TextLogger;

impl Log for TextLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() as usize <= CONFIGURED_LEVEL.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "[{}] {}",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// 应用环境变量日志级别后的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyLogLevel {
    /// 未设置或为空：保持原配置。
    Unchanged,
    /// 已解析并写入全局级别。
    Configured(LevelFilter),
}

/// 无法识别的 `log_level` 字符串错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidLogLevel(String);

impl fmt::Display for InvalidLogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unrecognized log level {:?}", self.0)
    }
}

impl std::error::Error for InvalidLogLevel {}

/// Runs the common initialization required before tests, matching the Go entry point.
/// 测试公共入口：应用 OS 日志级别，失败则打印错误并以 -1 退出。
#[allow(non_snake_case)]
pub fn SetupForCommonTest() {
    if let Err(error) = apply_os_log_level() {
        eprintln!("applyOSLogLevel failed: {error}");
        std::process::exit(-1);
    }
}

/// Applies `log_level` using the levels accepted by PingCAP's zap-based logger.
/// 读取环境变量 `log_level` 并配置全局 logger；级别名兼容 PingCAP zap。
pub fn apply_os_log_level() -> Result<ApplyLogLevel, InvalidLogLevel> {
    let raw_level = match std::env::var("log_level") {
        Ok(level) if !level.is_empty() => level,
        _ => return Ok(ApplyLogLevel::Unchanged),
    };
    let level = parse_log_level(&raw_level)?;

    // `log` only permits installing a facade once. The installed logger remains a
    // stable global proxy while each call replaces its effective level.
    // log facade 只能安装一次；之后仅更新 CONFIGURED_LEVEL / max_level。
    INSTALL_LOGGER.call_once(|| {
        let _ = log::set_logger(&LOGGER);
    });
    CONFIGURED_LEVEL.store(level as usize, Ordering::Relaxed);
    log::set_max_level(level);

    Ok(ApplyLogLevel::Configured(level))
}

/// 返回当前已配置的全局日志级别。
pub fn configured_log_level() -> LevelFilter {
    match CONFIGURED_LEVEL.load(Ordering::Relaxed) {
        value if value == LevelFilter::Off as usize => LevelFilter::Off,
        value if value == LevelFilter::Error as usize => LevelFilter::Error,
        value if value == LevelFilter::Warn as usize => LevelFilter::Warn,
        value if value == LevelFilter::Info as usize => LevelFilter::Info,
        value if value == LevelFilter::Debug as usize => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    }
}

/// 将字符串解析为 LevelFilter；dpanic/panic/fatal 映射为 Error，与 zap 一致。
fn parse_log_level(level: &str) -> Result<LevelFilter, InvalidLogLevel> {
    match level.to_ascii_lowercase().as_str() {
        "debug" => Ok(LevelFilter::Debug),
        "info" => Ok(LevelFilter::Info),
        "warn" | "warning" => Ok(LevelFilter::Warn),
        "error" | "dpanic" | "panic" | "fatal" => Ok(LevelFilter::Error),
        _ => Err(InvalidLogLevel(level.to_owned())),
    }
}

#[cfg(test)]
#[path = "bridge_test.rs"]
mod bridge_test;
