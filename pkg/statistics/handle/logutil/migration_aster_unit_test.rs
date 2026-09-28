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

// 统计 Logger 迁移期单元测试。
//
// 校验 Go->Rust 映射后：category 字段为 `stats`，采样 Logger 进程内复用且仅放行首条。

use astersql_statistics_handle_logutil::log::{LogField, LogLevel};
use astersql_statistics_handle_logutil::stats_logutil::{
    StatsErrVerboseLogger, StatsErrVerboseSampleLogger, StatsLogger, StatsSampleLogger,
};

/// 从日志字段列表中按 key 取出字符串值。
fn string_field<'a>(fields: &'a [LogField], key: &str) -> Option<&'a str> {
    fields.iter().find_map(|field| match field {
        LogField::String(field_key, value) if field_key == key => Some(value.as_str()),
        _ => None,
    })
}

/// 普通与详细错误 Logger 均应附带 Go 侧约定的 `category=stats`。
#[test]
fn stats_loggers_attach_the_go_category() {
    for logger in [StatsLogger(), StatsErrVerboseLogger()] {
        logger.info("category probe");
        let entries = logger.entries();
        let entry = entries
            .iter()
            .find(|entry| entry.message == "category probe")
            .expect("logger should emit the probe");
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(string_field(&entry.fields, "category"), Some("stats"));
    }
}

/// 采样 Logger 应复用同一采样器，并在窗口内只保留第一条；字段含 category 与 sampled。
#[test]
fn stats_sample_loggers_reuse_sampler_and_keep_go_fields() {
    let cases = [StatsSampleLogger(), StatsErrVerboseSampleLogger()];
    for (index, logger) in cases.into_iter().enumerate() {
        let message = format!("sample probe {index}");
        logger.info(&message);
        // 再次取同类型 Logger，应命中同一 OnceLock 实例
        let same_sampler = if index == 0 {
            StatsSampleLogger()
        } else {
            StatsErrVerboseSampleLogger()
        };
        same_sampler.info(&message);

        let matching: Vec<_> = logger
            .entries()
            .into_iter()
            .filter(|entry| entry.message == message)
            .collect();
        assert_eq!(matching.len(), 1, "Go sampler permits only the first entry");
        assert_eq!(string_field(&matching[0].fields, "category"), Some("stats"));
        assert_eq!(string_field(&matching[0].fields, "sampled"), Some(""));
    }
}
