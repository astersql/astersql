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

// 统计信息（statistics）专用日志封装。
//
// 在后台/详细错误 Logger 上附加 `category=stats` 字段，并提供限流采样变体，
// 避免统计后台任务在高频路径上刷爆日志。

#![allow(non_snake_case)]

use crate::log::{LogField, LogFieldCategory, Logger, background_logger, err_verbose_logger};
use std::sync::OnceLock;
use std::time::Duration;

/// 统计日志统一使用的 category 字段值。
const STATS_CATEGORY: &str = "stats";

/// 为给定 Logger 注入 `category=stats` 字段后返回新实例。
fn with_stats_category(logger: Logger) -> Logger {
    logger.with_fields([LogField::String(
        LogFieldCategory.into(),
        STATS_CATEGORY.into(),
    )])
}

/// Returns the background logger with category `stats`.
///
/// 返回带 `stats` 分类的后台 Logger，用于常规统计任务信息输出。
pub fn StatsLogger() -> Logger {
    with_stats_category(background_logger())
}

/// Returns the verbose-error logger with category `stats`.
///
/// 返回带 `stats` 分类的详细错误 Logger，用于错误场景的冗长诊断信息。
pub fn StatsErrVerboseLogger() -> Logger {
    with_stats_category(err_verbose_logger())
}

/// Returns one shared five-minute sampler which admits the first matching log.
///
/// 进程内共享的五分钟采样 Logger：同窗口内仅放行首条匹配日志，并标记 `sampled` 字段。
pub fn StatsSampleLogger() -> Logger {
    static LOGGER: OnceLock<Logger> = OnceLock::new();
    LOGGER
        .get_or_init(|| {
            // 5 分钟窗口、阈值 1，与 Go 侧采样策略对齐
            StatsLogger()
                .with_fields([LogField::String("sampled".into(), String::new())])
                .sample(Duration::from_secs(5 * 60), 1)
        })
        .clone()
}

/// Returns one shared ten-minute verbose-error sampler with the same threshold.
///
/// 进程内共享的十分钟详细错误采样 Logger，阈值同样为 1。
pub fn StatsErrVerboseSampleLogger() -> Logger {
    static LOGGER: OnceLock<Logger> = OnceLock::new();
    LOGGER
        .get_or_init(|| {
            StatsErrVerboseLogger()
                .with_fields([LogField::String("sampled".into(), String::new())])
                .sample(Duration::from_secs(10 * 60), 1)
        })
        .clone()
}
