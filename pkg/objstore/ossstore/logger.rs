// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// OSS SDK 日志桥接：将 Go OSS SDK 风格的级别前缀消息映射到 Rust `log`。
//
// `LogPrinter` 接收「级别字符串 + 消息」两参；`get_oss_log_level` 把仓库的
// `LevelFilter` 压缩为 OSS SDK 可识别的粗粒度级别。

use std::any::Any;

/// 从 `Any` 中取出 `&str`（支持 `String` 与 `&str` 两种装箱形式）。
fn any_str(value: &dyn Any) -> Option<&str> {
    if let Some(value) = value.downcast_ref::<String>() {
        Some(value.as_str())
    } else {
        value.downcast_ref::<&str>().copied()
    }
}

/// OSS SDK 侧启用的日志级别；比 `log::LevelFilter` 更粗。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OssLogLevel {
    /// 关闭 OSS SDK 日志。
    Off,
    /// 仅错误。
    Error,
    /// 警告及以上（含 Info 映射到 Warn）。
    Warn,
    /// 调试级别。
    Debug,
}

/// 无状态日志打印器，对应 Go OSS SDK 的 Logger 回调。
#[derive(Clone, Copy, Debug, Default)]
pub struct LogPrinter;

/// 构造默认 `LogPrinter`。
pub fn new_log_printer() -> LogPrinter {
    LogPrinter
}

impl LogPrinter {
    /// The Go OSS SDK passes exactly two strings: a level prefix and a message.
    /// 将 Go OSS SDK 传入的两段字符串（级别前缀 + 消息）转发到 `log` 宏。
    pub fn Print(&self, args: &[&dyn Any]) {
        if args.len() != 2 {
            log::warn!("invalid log from OSS: {} argument(s)", args.len());
            return;
        }
        let (Some(level), Some(message)) = (any_str(args[0]), any_str(args[1])) else {
            log::warn!("invalid log from OSS: arguments must be strings");
            return;
        };
        // 级别前缀带尾随空格，与 Go SDK 格式保持一致。
        match level {
            "ERROR " => log::error!("{message}"),
            "WARNING " => log::warn!("{message}"),
            "INFO " => log::info!("{message}"),
            "DEBUG " => log::debug!("{message}"),
            _ => {}
        }
    }
}

/// 将仓库全局 `LevelFilter` 映射为 OSS SDK 的 `OssLogLevel`。
/// Info 与 Warn 都落到 Warn，避免 SDK 过吵；Trace/Off 关闭。
pub fn get_oss_log_level(level: log::LevelFilter) -> OssLogLevel {
    match level {
        log::LevelFilter::Error => OssLogLevel::Error,
        log::LevelFilter::Warn | log::LevelFilter::Info => OssLogLevel::Warn,
        log::LevelFilter::Debug => OssLogLevel::Debug,
        log::LevelFilter::Off | log::LevelFilter::Trace => OssLogLevel::Off,
    }
}
