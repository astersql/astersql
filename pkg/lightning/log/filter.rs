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

// Lightning 日志过滤与结构化字段。
//
// 提供日志级别、键值字段、日志条目，以及按调用方包路径过滤的 `FilterCore`。
// 过滤语义对齐 Go 的 `strings.Contains`：调用栈函数名包含白名单子串才转发写入。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Number, Value};

/// 日志级别，从 Debug 到 Fatal，默认 Info。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
    DPanic,
    Fatal,
}

impl Level {
    /// 从字符串解析级别；空串与 `"info"` 均视为 Info。
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "" | "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "dpanic" => Ok(Self::DPanic),
            "fatal" => Ok(Self::Fatal),
            _ => Err(format!("unrecognized log level {value:?}")),
        }
    }

    /// 返回大写级别名，用于 JSON 编码中的 `$lvl`。
    pub fn capital(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::DPanic => "DPANIC",
            Self::Fatal => "FATAL",
        }
    }
}

/// 结构化日志字段：键 + JSON 值；`skip` 为真时编码时忽略。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// 字段名。
    pub key: String,
    /// 字段值（JSON）。
    pub value: Value,
    /// 为真表示占位/跳过字段，不写入输出。
    skip: bool,
}

impl Field {
    /// 构造字符串字段。
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: Value::String(value.into()),
            skip: false,
        }
    }

    /// 构造整数字段。
    pub fn int(key: impl Into<String>, value: i64) -> Self {
        Self {
            key: key.into(),
            value: Value::Number(value.into()),
            skip: false,
        }
    }

    /// 构造整数数组字段。
    pub fn ints(key: impl Into<String>, values: impl IntoIterator<Item = impl Into<i64>>) -> Self {
        Self {
            key: key.into(),
            value: Value::Array(
                values
                    .into_iter()
                    .map(|value| Value::Number(Number::from(value.into())))
                    .collect(),
            ),
            skip: false,
        }
    }

    /// 构造时长字段，内部格式化为可读字符串。
    pub fn duration(key: impl Into<String>, value: Duration) -> Self {
        Self::string(key, format_duration(value))
    }

    /// 构造跳过字段（编码时不输出）。
    pub fn skip() -> Self {
        Self {
            key: String::new(),
            value: Value::Null,
            skip: true,
        }
    }
}

/// 将 `Duration` 格式化为秒/毫秒/微秒/纳秒字符串。
fn format_duration(value: Duration) -> String {
    if value.as_secs() > 0 {
        format!("{}.{:09}s", value.as_secs(), value.subsec_nanos())
    } else if value.as_millis() > 0 {
        format!("{}ms", value.as_millis())
    } else if value.as_micros() > 0 {
        format!("{}µs", value.as_micros())
    } else {
        format!("{}ns", value.as_nanos())
    }
}

/// 单条日志条目：级别、消息、调用方与 logger 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 日志级别。
    pub level: Level,
    /// 日志消息正文。
    pub message: String,
    /// 调用方标识（文件路径或函数名），供 FilterCore 匹配。
    pub caller_function: String,
    /// Logger 名称（可经 `named` 拼接层级）。
    pub logger_name: String,
}

impl Entry {
    /// 创建条目，调用方与 logger 名初始为空。
    pub fn new(level: Level, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            caller_function: String::new(),
            logger_name: String::new(),
        }
    }

    /// 设置调用方标识后返回自身。
    pub fn with_caller(mut self, caller: impl Into<String>) -> Self {
        self.caller_function = caller.into();
        self
    }
}

/// Core 写入失败时的错误类型（字符串描述）。
pub type CoreError = String;

/// 日志核心抽象：级别开关、附加字段、命名与写条目。
pub trait Core: Send + Sync {
    /// 指定级别是否启用。
    fn enabled(&self, level: Level) -> bool;
    /// 返回附加了固定字段的新 Core。
    fn with(&self, fields: Vec<Field>) -> Arc<dyn Core>;
    /// 返回带层级名称的新 Core。
    fn named(&self, name: &str) -> Arc<dyn Core>;
    /// 写入一条日志。
    fn write(&self, entry: Entry, fields: Vec<Field>) -> Result<(), CoreError>;
}

/// 将条目与字段编码为 JSON 行（含 `$lvl`、`$msg` 等约定键）。
pub fn encode_json(entry: &Entry, fields: impl IntoIterator<Item = Field>) -> String {
    let mut object = Map::new();
    object.insert(
        "$lvl".to_owned(),
        Value::String(entry.level.capital().to_owned()),
    );
    object.insert("$msg".to_owned(), Value::String(entry.message.clone()));
    if !entry.logger_name.is_empty() {
        object.insert(
            "logger".to_owned(),
            Value::String(entry.logger_name.clone()),
        );
    }
    // 跳过 skip 字段，其余键值写入 JSON 对象。
    for field in fields {
        if !field.skip {
            object.insert(field.key, field.value);
        }
    }
    Value::Object(object).to_string()
}

/// A Core wrapper which forwards entries only when Caller.Function contains an
/// allowed package fragment, matching Go's strings.Contains semantics.
/// 按调用方包路径白名单过滤的 Core 包装器。
pub struct FilterCore {
    /// 被包装的下游 Core。
    pub Core: Arc<dyn Core>,
    /// 允许通过的包路径子串列表。
    filters: Vec<String>,
}

impl fmt::Debug for FilterCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilterCore")
            .field("filters", &self.filters)
            .finish()
    }
}

impl FilterCore {
    /// 用下游 Core 与允许的包路径片段构造过滤器。
    pub fn new(
        core: Arc<dyn Core>,
        allow_packages: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            Core: core,
            filters: allow_packages.into_iter().map(Into::into).collect(),
        }
    }

    /// 在保留过滤规则的同时附加固定字段。
    pub fn with(&self, fields: impl IntoIterator<Item = Field>) -> Self {
        Self {
            Core: self.Core.with(fields.into_iter().collect()),
            filters: self.filters.clone(),
        }
    }

    /// 便捷写入，内部转调 `Core::write`。
    pub fn write(
        &self,
        entry: Entry,
        fields: impl IntoIterator<Item = Field>,
    ) -> Result<(), CoreError> {
        Core::write(self, entry, fields.into_iter().collect())
    }
}

/// Go 风格构造函数别名：创建 `FilterCore`。
pub fn NewFilterCore(
    core: Arc<dyn Core>,
    allow_packages: impl IntoIterator<Item = impl Into<String>>,
) -> FilterCore {
    FilterCore::new(core, allow_packages)
}

impl Core for FilterCore {
    fn enabled(&self, level: Level) -> bool {
        self.Core.enabled(level)
    }

    fn with(&self, fields: Vec<Field>) -> Arc<dyn Core> {
        Arc::new(FilterCore {
            Core: self.Core.with(fields),
            filters: self.filters.clone(),
        })
    }

    fn named(&self, name: &str) -> Arc<dyn Core> {
        Arc::new(FilterCore {
            Core: self.Core.named(name),
            filters: self.filters.clone(),
        })
    }

    fn write(&self, entry: Entry, fields: Vec<Field>) -> Result<(), CoreError> {
        // 调用方函数名包含任一白名单子串才转发；否则静默丢弃。
        if self
            .filters
            .iter()
            .any(|filter| entry.caller_function.contains(filter))
        {
            self.Core.write(entry, fields)
        } else {
            Ok(())
        }
    }
}
