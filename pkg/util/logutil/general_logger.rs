// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// General Log（通用查询日志）专用 logger 工厂。
//
// 对应 Go `pkg/util/logutil` 中的 general logger：从全局 `LogConfig`
// 派生一份配置，清空级别（由 init 侧按 general log 约定处理），
// 并将输出文件切到 `general_log_file`。

use crate::log::{LogConfig, Logger, init_logger};

/// 按 general log 配置初始化并返回专用 `Logger`。
pub fn new_general_logger(cfg: &LogConfig) -> Result<Logger, String> {
    init_logger(&new_general_log_config(cfg))
}

/// 从全局日志配置派生 general log 配置：清空 level，必要时改写文件名。
pub fn new_general_log_config(cfg: &LogConfig) -> LogConfig {
    let mut general = cfg.clone();
    // general log 不沿用全局 level，由后续初始化填入约定级别
    general.level.clear();
    if !cfg.general_log_file.is_empty() {
        general.file = cfg.file.clone();
        general.file.filename = cfg.general_log_file.clone();
    }
    general
}

/// Go 风格别名：`newGeneralLogConfig`。
pub use new_general_log_config as newGeneralLogConfig;
/// Go 风格别名：`newGeneralLogger`。
pub use new_general_logger as newGeneralLogger;
