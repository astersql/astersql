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

// logutil 迁移回归单测：Hex 格式、慢/通用日志配置、采样、初始化与 Trace 事件。
//
// 对照 Go 侧零值规则、字段顺序与慢查询编码格式，验证机械迁移后的行为一致性。

use crate::general_logger::new_general_log_config;
use crate::hex::{Hex, ProtoField, ProtoValue};
use crate::log::{
    FileLogConfig, LogConfig, LogField, LogLevel, Logger, TraceInfo, TraceSpan, event,
    fields_from_trace_info, initialize_loggers, proxy_fields_from, set_tag,
};
use crate::slow_query_logger::{SlowLogEncoder, new_slow_query_log_config};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 校验 Region 类消息的 Hex 展示：字节键转十六进制，且忽略 XXX_ 内部字段。
#[test]
fn test_hex_matches_go_field_and_byte_formatting() {
    // Region 是 TiKV 中一段连续 key 范围的元数据；StartKey/EndKey 为边界。
    let region = ProtoValue::message(vec![
        ProtoField::new("Id", 6662_u64),
        ProtoField::new(
            "StartKey",
            b"t\xc8\\\0\0\0\\\0\0\0%-\0\0\0\0\0\0\0%".as_slice(),
        ),
        ProtoField::new("EndKey", b"3asg3asd".as_slice()),
        ProtoField::new("XXX_Internal", 99_u64),
        ProtoField::new("Peers", ProtoValue::List(vec![])),
    ]);
    assert_eq!(
        Hex(&region).to_string(),
        "{Id:6662 StartKey:74c85c0000005c000000252d0000000000000025 EndKey:3361736733617364 Peers:[]}"
    );
}

/// 慢查询/通用日志配置应复制滚动策略但改写文件名，且不改动源配置。
#[test]
fn test_dedicated_configs_copy_file_settings_without_mutating_source() {
    let file = FileLogConfig {
        filename: "tidb.log".into(),
        max_size: 10,
        max_days: 11,
        max_backups: 12,
        compression: "gzip".into(),
    };
    let cfg = LogConfig::new(
        "error",
        "text",
        "slow.log",
        "general.log",
        file.clone(),
        false,
    );
    let slow = new_slow_query_log_config(&cfg);
    let general = new_general_log_config(&cfg);

    assert_eq!(cfg.level, "error");
    assert_eq!(slow.level, "");
    assert_eq!(general.level, "");
    assert_eq!(
        slow.file,
        FileLogConfig {
            filename: "slow.log".into(),
            ..file.clone()
        }
    );
    assert_eq!(
        general.file,
        FileLogConfig {
            filename: "general.log".into(),
            ..file
        }
    );
}

/// TraceInfo 零值/空值不产生字段；非零连接 ID 与 session_alias 按 Go 顺序输出。
#[test]
fn test_trace_fields_follow_go_zero_value_rules() {
    assert!(fields_from_trace_info(None).is_empty());
    assert!(fields_from_trace_info(Some(&TraceInfo::default())).is_empty());
    assert_eq!(
        fields_from_trace_info(Some(&TraceInfo {
            connection_id: 1,
            session_alias: String::new(),
        })),
        vec![LogField::U64("conn".into(), 1)]
    );
    assert_eq!(
        fields_from_trace_info(Some(&TraceInfo {
            connection_id: 0,
            session_alias: "alias123".into(),
        })),
        vec![LogField::String("session_alias".into(), "alias123".into())]
    );
    assert_eq!(
        fields_from_trace_info(Some(&TraceInfo {
            connection_id: 1,
            session_alias: "alias123".into()
        })),
        vec![
            LogField::U64("conn".into(), 1),
            LogField::String("session_alias".into(), "alias123".into())
        ]
    );
}

/// 慢查询编码器忽略结构化字段，仅输出 `# Time:` 行与 SQL 文本两行。
#[test]
fn test_slow_encoder_ignores_fields_and_writes_two_lines() {
    let output = SlowLogEncoder.encode(
        SystemTime::UNIX_EPOCH + Duration::from_nanos(123_456_789),
        "select 1;",
        &[LogField::String("ignored".into(), "value".into())],
    );
    assert_eq!(
        output,
        "# Time: 1970-01-01T00:00:00.123456789Z\nselect 1;\n"
    );
}

/// 代理环境字段仅收录已设置项，并保持 Go 侧键名顺序。
#[test]
fn test_proxy_fields_only_include_present_values_in_go_order() {
    let envs = ["http_proxy", "https_proxy", "no_proxy"];
    let values = [
        "http://127.0.0.1:8080",
        "https://127.0.0.1:8443",
        "localhost,127.0.0.1",
    ];

    // Match Go's exhaustive mask: every subset must contain exactly the selected
    // fields, in http/https/no_proxy order.
    for mask in 0_u8..=0b111 {
        let env: HashMap<String, String> = envs
            .iter()
            .zip(values)
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, (key, value))| ((*key).to_owned(), value.to_owned()))
            .collect();
        let expected = envs
            .iter()
            .zip(values)
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, (key, value))| LogField::String((*key).into(), value.into()))
            .collect::<Vec<_>>();

        assert_eq!(proxy_fields_from(|key| env.get(key).cloned()), expected);
    }

    let env = HashMap::from([
        ("HTTP_PROXY".to_owned(), "http://upper.example".to_owned()),
        ("http_proxy".to_owned(), "http://lower.example".to_owned()),
    ]);
    assert_eq!(
        proxy_fields_from(|key| env.get(key).cloned()),
        vec![LogField::String(
            "http_proxy".into(),
            "http://upper.example".into()
        )]
    );
}

/// 采样日志：同一 tick 内相同消息最多保留 N 条，不同消息不受限。
#[test]
fn test_sample_logger_allows_only_first_matching_entries_per_tick() {
    let logger = Arc::new(Logger::memory(LogLevel::Info));
    let sampled = logger.sample(Duration::from_secs(60), 3);
    for _ in 0..100 {
        sampled.info("sample log test");
    }
    sampled.info("another message");
    assert_eq!(
        logger
            .entries()
            .iter()
            .filter(|entry| entry.message == "sample log test")
            .count(),
        3
    );
    assert_eq!(
        logger
            .entries()
            .iter()
            .filter(|entry| entry.message == "another message")
            .count(),
        1
    );
}

/// 同名日志文件共享 sink；不支持的压缩算法在初始化时被拒绝。
#[test]
fn test_init_reuses_sink_for_equal_file_names_and_rejects_bad_compression() {
    let file = FileLogConfig {
        filename: "same.log".into(),
        ..FileLogConfig::default()
    };
    let cfg = LogConfig::new("info", "text", "same.log", "same.log", file, false);
    let globals = initialize_loggers(&cfg).expect("logger config should be valid");
    globals.slow_query.info("slow");
    globals.general.info("general");

    let bad = LogConfig::new(
        "info",
        "text",
        "",
        "",
        FileLogConfig {
            compression: "zip".into(),
            ..FileLogConfig::default()
        },
        false,
    );
    assert!(
        initialize_loggers(&bad)
            .unwrap_err()
            .contains("unsupported log compression")
    );
    let _ = std::fs::remove_file("same.log");
}

/// 无 span 时 event/set_tag 为空操作；有 span 时记录事件与标签。
#[test]
fn test_trace_events_and_tags_are_noops_without_span_and_record_with_span() {
    event(None, "ignored");
    set_tag(None, "ignored", 1);
    let span = TraceSpan::default();
    event(Some(&span), "compile");
    set_tag(Some(&span), "region", 42);
    assert_eq!(span.events(), vec![("event".into(), "compile".into())]);
    assert_eq!(span.tags().get("region").map(String::as_str), Some("42"));
}
