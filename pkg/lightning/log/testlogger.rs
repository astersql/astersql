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

// 测试用内存 Logger：把日志行写入可观测的 Buffer，便于断言 JSON 输出。
//
// 对应 Go 的 testlogger，实现 `Core` 接口后通过 `Logger::Wrap` 暴露与生产相同的 API。

use std::sync::{Arc, Mutex};

use crate::filter::{Core, Entry, Field, Level, encode_json};
use crate::log::Logger;

/// 线程安全的日志行缓冲；`Arc<Mutex<_>>` 让子 Logger 与测试侧共享同一份输出。
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    lines: Arc<Mutex<Vec<String>>>,
}

impl Buffer {
    /// 将已记录的日志行用换行拼接成单字符串，便于整段断言。
    pub fn stripped(&self) -> String {
        self.lines().join("\n")
    }

    /// 返回当前缓冲中的全部日志行副本。
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("test log buffer poisoned").clone()
    }
}

/// 预留的测试 Logger 选项占位类型；当前无可用变体，签名与 Go 可变选项对齐。
#[derive(Clone, Copy, Debug)]
pub enum TestLoggerOption {}

/// 内存实现的日志 Core：始终启用，写入时编码为 JSON 并追加到 Buffer。
#[derive(Debug)]
struct MemoryCore {
    buffer: Buffer,
    fields: Vec<Field>,
    name: String,
}

impl Core for MemoryCore {
    fn enabled(&self, _level: Level) -> bool {
        true
    }

    fn with(&self, fields: Vec<Field>) -> Arc<dyn Core> {
        // 派生 Core 合并父级字段与新增字段，仍共享同一 Buffer。
        let mut combined = self.fields.clone();
        combined.extend(fields);
        Arc::new(Self {
            buffer: self.buffer.clone(),
            fields: combined,
            name: self.name.clone(),
        })
    }

    fn named(&self, name: &str) -> Arc<dyn Core> {
        // 层级命名用 `.` 连接，对应 zap/Go Logger.Named 的层次结构。
        let mut full_name = self.name.clone();
        if !full_name.is_empty() {
            full_name.push('.');
        }
        full_name.push_str(name);
        Arc::new(Self {
            buffer: self.buffer.clone(),
            fields: self.fields.clone(),
            name: full_name,
        })
    }

    fn write(&self, mut entry: Entry, fields: Vec<Field>) -> Result<(), String> {
        entry.logger_name = self.name.clone();
        // 先写入口级字段，再追加本次调用字段，编码为与 Go 兼容的 JSON 行。
        let line = encode_json(&entry, self.fields.iter().cloned().chain(fields));
        self.buffer
            .lines
            .lock()
            .map_err(|error| error.to_string())?
            .push(line);
        Ok(())
    }
}

/// 构造测试 Logger 及其共享 Buffer；`_opts` 保留 Go 可变选项形参，当前忽略。
pub fn MakeTestLogger(_opts: impl IntoIterator<Item = TestLoggerOption>) -> (Logger, Buffer) {
    let buffer = Buffer::default();
    let core = MemoryCore {
        buffer: buffer.clone(),
        fields: Vec::new(),
        name: String::new(),
    };
    (Logger::Wrap(Arc::new(core)), buffer)
}
