// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// timeutil 错误类型定义。
//
// 覆盖时区名无效、系统时区未设置、zoneinfo 路径不支持、偏移越界、
// 本地时间歧义以及 IO 失败等；并提供与 MySQL「未知时区」错误类对齐的标记。

use std::fmt;
use std::path::Path;

/// timeutil 包产生的错误枚举。
/// Errors produced by the timeutil package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeUtilError {
    /// 未知或不正确的时区名（对齐 MySQL unknown time zone）。
    UnknownTimeZone { name: String },
    /// 时区名格式/内容非法。
    InvalidTimeZoneName { name: String },
    /// 系统时区变量 `systemTZ` 未正确设置。
    InvalidSystemTimeZone,
    /// 不支持的 zoneinfo 文件路径。
    UnsupportedZoneInfoPath { path: String },
    /// 时区偏移秒数超出合法范围。
    InvalidOffset { seconds: i32 },
    /// 本地时间无效或在夏令时切换时存在歧义。
    InvalidLocalTime,
    /// 读写时区相关路径时的 IO 错误。
    Io {
        operation: &'static str,
        path: String,
        message: String,
    },
}

impl TimeUtilError {
    /// 由 IO 操作名、路径与底层错误构造 `Io` 变体。
    pub(crate) fn io(operation: &'static str, path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for TimeUtilError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTimeZone { name } => {
                let name = name.chars().take(64).collect::<String>();
                write!(formatter, "Unknown or incorrect time zone: '{name}'")
            }
            Self::InvalidTimeZoneName { name } => {
                write!(formatter, "invalid name for timezone {name}")
            }
            Self::InvalidSystemTimeZone => {
                formatter.write_str("variable `systemTZ` is not properly set")
            }
            Self::UnsupportedZoneInfoPath { path } => {
                write!(formatter, "path {path} is not supported")
            }
            Self::InvalidOffset { seconds } => {
                write!(formatter, "timezone offset {seconds} is out of range")
            }
            Self::InvalidLocalTime => formatter.write_str("local time is invalid or ambiguous"),
            Self::Io {
                operation,
                path,
                message,
            } => {
                write!(formatter, "{operation} {path}: {message}")
            }
        }
    }
}

impl std::error::Error for TimeUtilError {}

/// 与 Go/MySQL「未知时区」错误类兼容的标记类型。
/// Go-compatible marker for the MySQL unknown-time-zone error class.
pub struct UnknownTimeZoneClass;

impl UnknownTimeZoneClass {
    /// 按时区名生成 `UnknownTimeZone` 错误（对齐 Go GenWithStackByArgs）。
    pub fn GenWithStackByArgs(&self, name: impl Into<String>) -> TimeUtilError {
        TimeUtilError::UnknownTimeZone { name: name.into() }
    }

    /// 判断错误是否属于未知时区类。
    pub fn Equal(&self, error: &TimeUtilError) -> bool {
        matches!(error, TimeUtilError::UnknownTimeZone { .. })
    }
}

/// 全局「未知时区」错误类实例。
/// ErrUnknownTimeZone indicates that a timezone is unknown.
pub static ErrUnknownTimeZone: UnknownTimeZoneClass = UnknownTimeZoneClass;
