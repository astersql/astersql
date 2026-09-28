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

// 基于 `tracing` 的日志捕获钩子，对齐 Go 侧 zap Core 的测试用法。
//
// 通过 `WithLogHook` 安装本地 dispatcher，按消息子串过滤并缓存字段，供断言。

use std::fmt::{self, Write as _};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Dispatch, Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

/// 单条日志字段的取值形态（字符串 / 数值 / 布尔 / Debug）。
#[derive(Clone, Debug, PartialEq)]
enum LogValue {
    String(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
    Debug(String),
}

impl fmt::Display for LogValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) | Self::Debug(value) => formatter.write_str(value),
            Self::I64(value) => value.fmt(formatter),
            Self::U64(value) => value.fmt(formatter),
            Self::F64(value) => value.fmt(formatter),
            Self::Bool(value) => value.fmt(formatter),
        }
    }
}

/// 日志结构化字段：键与类型化取值。
#[derive(Clone, Debug, PartialEq)]
pub struct LogField {
    key: String,
    value: LogValue,
}

impl LogField {
    /// 以字符串值构造字段。
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: LogValue::String(value.into()),
        }
    }

    /// 以 i64 值构造字段。
    pub fn i64(key: impl Into<String>, value: i64) -> Self {
        Self {
            key: key.into(),
            value: LogValue::I64(value),
        }
    }
}

/// 一条已捕获的日志：级别、target、消息与字段列表。
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    level: tracing::Level,
    target: String,
    message: String,
    fields: Vec<LogField>,
}

impl LogEntry {
    /// 断言消息文本完全相等。
    pub fn CheckMsg(&self, msg: &str) {
        assert_eq!(msg, self.message);
    }

    /// 断言本条日志包含全部指定字段（按键值完全匹配）。
    pub fn CheckField(&self, requireFields: &[LogField]) {
        for required in requireFields {
            assert!(
                self.fields.iter().any(|field| field == required),
                "matched log field {}:{} not found in log: {:?}",
                required.key,
                required.value,
                self.fields
            );
        }
    }

    /// 断言指定字段存在且为非空字符串。
    pub fn CheckFieldNotEmpty(&self, fieldName: &str) {
        let field = self
            .fields
            .iter()
            .find(|field| field.key == fieldName)
            .unwrap_or_else(|| panic!("log field {fieldName} not found in log"));
        match &field.value {
            LogValue::String(value) => {
                assert!(!value.is_empty(), "log field {fieldName} is empty")
            }
            _ => panic!("log field {fieldName} is not a string field"),
        }
    }
}

// LogHook captures tracing events, mainly for testing.
/// 捕获 tracing 事件的钩子；可按消息子串过滤后缓存条目。
#[derive(Clone, Debug)]
pub struct LogHook {
    logs: Arc<Mutex<Vec<LogEntry>>>,
    messageFilter: Arc<str>,
}

impl LogHook {
    /// 创建钩子；`messageFilter` 为空表示不过滤。
    fn new(messageFilter: impl Into<String>) -> Self {
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
            messageFilter: Arc::from(messageFilter.into()),
        }
    }

    // Write captures the log and saves it.
    /// 写入一条日志（附带字段）到内部缓存。
    pub fn Write(&self, mut entry: LogEntry, fields: Vec<LogField>) {
        entry.fields = fields;
        self.logs
            .lock()
            .expect("log hook mutex poisoned")
            .push(entry);
    }

    // Check implements the same substring message filter as the Go zap core.
    /// 判断条目消息是否通过子串过滤器（对齐 Go zap Core）。
    pub fn Check(&self, entry: &LogEntry) -> bool {
        self.messageFilter.is_empty() || entry.message.contains(self.messageFilter.as_ref())
    }

    /// 将条目编码为 `级别\t消息\t键=值...` 文本，便于断言输出。
    fn encode(&self, entry: &LogEntry) -> Result<String, fmt::Error> {
        let mut encoded = String::new();
        write!(encoded, "{}\t{}", entry.level, entry.message)?;
        for field in &entry.fields {
            write!(encoded, "\t{}={}", field.key, field.value)?;
        }
        Ok(encoded)
    }

    // CheckLogCount asserts the number of captured logs after encoding every entry.
    /// 断言已捕获日志条数；失败时附带编码后的全文便于排查。
    pub fn CheckLogCount(&self, expected: usize) {
        let logs = self.Logs();
        let encoded = logs
            .iter()
            .map(|entry| self.encode(entry).expect("encode captured log"))
            .collect::<Vec<_>>();
        assert_eq!(expected, encoded.len(), "captured logs: {encoded:?}");
    }

    /// 返回当前已捕获日志的克隆快照。
    pub fn Logs(&self) -> Vec<LogEntry> {
        self.logs.lock().expect("log hook mutex poisoned").clone()
    }
}

/// 遍历 tracing 字段时的临时收集器：拆出 `message` 与其余字段。
#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: Vec<LogField>,
}

impl EventVisitor {
    /// 将单个字段记入 message 或 fields。
    fn record(&mut self, field: &Field, value: LogValue) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(LogField {
                key: field.name().to_owned(),
                value,
            });
        }
    }
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, LogValue::String(value.to_owned()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record(field, LogValue::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record(field, LogValue::U64(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record(field, LogValue::F64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record(field, LogValue::Bool(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record(field, LogValue::Debug(format!("{value:?}")));
    }
}

impl<S> Layer<S> for LogHook
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // 先收集字段，再按消息过滤，通过后才写入缓存。
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let entry = LogEntry {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: visitor.message,
            fields: Vec::new(),
        };
        if self.Check(&entry) {
            self.Write(entry, visitor.fields);
        }
    }
}

// WithLogHook returns a context-local tracing dispatcher and the hook used by it.
/// 构造带本钩子的 tracing `Dispatch`，供 `with_default` 局部安装。
pub fn WithLogHook(msgFilter: &str) -> (Dispatch, LogHook) {
    let hook = LogHook::new(msgFilter);
    let dispatcher = Dispatch::new(Registry::default().with(hook.clone()));
    (dispatcher, hook)
}
