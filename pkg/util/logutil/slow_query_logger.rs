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

// 慢查询日志（slow query log）构造与编码。
//
// 慢查询日志记录执行时间超过阈值的 SQL，便于运维排查性能问题。
// 本模块从全局 `LogConfig` 派生专用配置，并用 `SlowLogEncoder` 输出固定两行格式。

use crate::log::{LogConfig, LogField, Logger, SlowLogTimeFormat, init_logger};
use chrono::{DateTime, Utc};
use std::time::SystemTime;

/// 按慢查询专用配置初始化 Logger。
pub fn new_slow_query_logger(cfg: &LogConfig) -> Result<Logger, String> {
    init_logger(&new_slow_query_log_config(cfg)).map(|logger| logger.with_slow_log_encoding())
}

/// 从已有 Logger 派生慢查询 Logger，并替换为慢查询专用编码器。
pub fn new_slow_query_logger_from_logger(logger: &Logger) -> Logger {
    logger.with_slow_log_encoding()
}

/// 从全局日志配置派生慢查询配置：清空 level，并按需改写输出文件名。
pub fn new_slow_query_log_config(cfg: &LogConfig) -> LogConfig {
    let mut slow = cfg.clone();
    // 慢查询通道不继承全局 level，由调用方按需控制写入。
    slow.level.clear();
    if !cfg.slow_query_file.is_empty() {
        // 复用滚动/压缩等文件策略，仅替换慢查询专用文件名。
        slow.file = cfg.file.clone();
        slow.file.filename = cfg.slow_query_file.clone();
    }
    slow
}

/// 慢查询文本编码器：忽略结构化字段，输出 `# Time:` + SQL 两行。
#[derive(Clone, Copy, Debug, Default)]
pub struct SlowLogEncoder;

impl SlowLogEncoder {
    /// 将时间与 SQL 编码为慢查询日志文本；`_fields` 保留以对齐 Go 签名。
    pub fn encode(&self, time: SystemTime, message: &str, _fields: &[LogField]) -> String {
        let time: DateTime<Utc> = time.into();
        format!("# Time: {}\n{}\n", time.format(SlowLogTimeFormat), message)
    }
}

/// Go 风格别名：`newSlowQueryLogConfig`。
pub use new_slow_query_log_config as newSlowQueryLogConfig;
/// Go 风格别名：`newSlowQueryLogger`。
pub use new_slow_query_logger as newSlowQueryLogger;
/// Go 风格别名：`newSlowQueryLoggerFromZapLogger`。
pub use new_slow_query_logger_from_logger as newSlowQueryLoggerFromZapLogger;
