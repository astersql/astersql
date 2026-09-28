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

// DDL logutil 的 Aster 迁移单元测试。
//
// 验证各类 DDL Logger 附加的 category 与 Go 一致，以及 SampleLogger
// 在窗口内仅保留前 N 条匹配消息的采样行为。

use astersql_ddl_logutil::ddl_logutil::{
    DDLIngestLogger, DDLLogger, DDLUpgradingLogger, SampleLogger,
};
use astersql_ddl_logutil::util::logutil::log::{InitLogger, LogConfig, LogField, LogFieldCategory};

/// 从日志字段中取出 category 字符串值。
fn category_value(fields: &[LogField]) -> Option<&str> {
    fields.iter().find_map(|field| match field {
        LogField::String(key, value) if key == LogFieldCategory => Some(value.as_str()),
        _ => None,
    })
}

/// 三类专用 Logger 写入后，条目上的 category 应分别为 ddl / ddl-upgrading / ddl-ingest。
#[test]
fn ddl_loggers_attach_the_same_categories_as_go() {
    InitLogger(&LogConfig::default()).expect("default logger config should be valid");

    for (logger, expected) in [
        (DDLLogger(), "ddl"),
        (DDLUpgradingLogger(), "ddl-upgrading"),
        (DDLIngestLogger(), "ddl-ingest"),
    ] {
        logger.info(expected);
        let entries = logger.entries();
        let entry = entries.last().expect("logger should retain the entry");
        assert_eq!(category_value(&entry.fields), Some(expected));
    }
}

/// 采样 Logger 对同一消息最多保留 3 条，不同消息不受影响，且 category 均为 ddl。
#[test]
fn ddl_sample_logger_keeps_only_first_three_matching_messages() {
    InitLogger(&LogConfig::default()).expect("default logger config should be valid");
    let logger = SampleLogger();

    for _ in 0..100 {
        SampleLogger().info("sample log test");
    }
    SampleLogger().info("another message");

    let entries = logger.entries();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.message == "sample log test")
            .count(),
        3
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.message == "another message")
            .count(),
        1
    );
    assert!(
        entries
            .iter()
            .filter(|entry| matches!(
                entry.message.as_str(),
                "sample log test" | "another message"
            ))
            .all(|entry| category_value(&entry.fields) == Some("ddl"))
    );
}
