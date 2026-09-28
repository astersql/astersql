// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// adapter Aster 单元测试：类别映射、控制标志、未知事件与真值表/模式布局。

use astersql_util_traceevent::adapter::{
    ClientCategory, TraceControlFlags, handle_client_go_is_category_enabled,
    handle_client_go_trace_event, handle_trace_control_extractor, map_category,
};
use astersql_util_traceevent::flightrecorder::{
    DumpTriggerConfig, FlightRecorderConfig, SuspiciousEventConfig, Trace, UserCommandConfig,
    check_truth_table, close_flight_recorder, start_log_flight_recorder,
};
use astersql_util_traceevent::traceevent::{
    ALL_CATEGORIES, Context, Event, Field, GENERAL, Phase, RingBufferSink, Sink, TIKV_READ_DETAILS,
    TIKV_REQUEST, TIKV_WRITE_DETAILS, TXN_2PC, TraceCategory, UNKNOWN_CLIENT, current_mode,
    generate_trace_id, normalize_mode, set_mode,
};
use std::sync::Arc;

/// 构造启用指定类别、采样触发的飞行记录器配置。
fn recorder_config(categories: Vec<&str>) -> FlightRecorderConfig {
    FlightRecorderConfig {
        enabled_categories: categories.into_iter().map(str::to_owned).collect(),
        dump_trigger: DumpTriggerConfig {
            kind: "sampling".to_owned(),
            sampling: 1,
            ..DumpTriggerConfig::default()
        },
    }
}

/// 验证类别映射与启用查询。
#[test]
fn adapter_maps_categories_and_reports_enablement() {
    let _guard = super::test_support::test_guard();
    close_flight_recorder();
    start_log_flight_recorder(recorder_config(vec!["txn_2pc"])).unwrap();

    assert_eq!(map_category(ClientCategory::Txn2Pc), TXN_2PC);
    assert_eq!(map_category(ClientCategory::Other(77)), UNKNOWN_CLIENT);
    assert!(handle_client_go_is_category_enabled(ClientCategory::Txn2Pc));
    assert!(!handle_client_go_is_category_enabled(
        ClientCategory::KvRequest
    ));
}

/// 无 sink 时仍返回类别标志且不置 IMMEDIATE_LOG。
#[test]
fn extractor_keeps_category_flags_without_a_context_sink() {
    let _guard = super::test_support::test_guard();
    close_flight_recorder();
    start_log_flight_recorder(recorder_config(vec![
        "tikv_request",
        "tikv_write_details",
        "tikv_read_details",
    ]))
    .unwrap();

    let flags = handle_trace_control_extractor(&Context::default());
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));
    assert!(flags.contains(TraceControlFlags::TIKV_WRITE_DETAILS));
    assert!(flags.contains(TraceControlFlags::TIKV_READ_DETAILS));
    assert!(!flags.contains(TraceControlFlags::IMMEDIATE_LOG));

    assert_eq!(
        TIKV_REQUEST | TIKV_WRITE_DETAILS | TIKV_READ_DETAILS,
        TraceCategory(flags.category_bits())
    );
}

/// 命中真值表时置 IMMEDIATE_LOG。
#[test]
fn extractor_sets_immediate_log_when_trace_matches_truth_table() {
    let _guard = super::test_support::test_guard();
    close_flight_recorder();
    start_log_flight_recorder(recorder_config(vec!["*"])).unwrap();
    let trace = Arc::new(Trace::new_with_random(7));
    trace.mark_bits(0);
    let ctx = Context::default().with_sink(trace);

    let flags = handle_trace_control_extractor(&ctx);
    assert!(flags.contains(TraceControlFlags::IMMEDIATE_LOG));
    assert_eq!(
        ALL_CATEGORIES,
        astersql_util_traceevent::traceevent::get_enabled_categories()
    );
}

/// 未知 client 事件保留原始类别字段。
#[test]
fn unknown_client_event_preserves_original_category() {
    let _guard = super::test_support::test_guard();
    close_flight_recorder();
    start_log_flight_recorder(recorder_config(vec!["unknown_client"])).unwrap();
    set_mode("base").unwrap();
    let trace = Arc::new(Trace::new_with_random(9));
    let ctx = Context::default().with_sink(trace.clone());

    handle_client_go_trace_event(
        &ctx,
        ClientCategory::Other(1234),
        "future event",
        vec![Field::string("source", "client-go")],
    );

    let events = trace.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].category, UNKNOWN_CLIENT);
    assert!(
        events[0]
            .fields
            .contains(&Field::u32("client_go_category", 1234))
    );
}

/// and/or 触发器编译保留真值表语义。
#[test]
fn trigger_compilation_preserves_and_or_truth_table() {
    let _guard = super::test_support::test_guard();
    let user = DumpTriggerConfig {
        kind: "user_command".to_owned(),
        user_command: Some(UserCommandConfig {
            kind: "sql_digest".to_owned(),
            sql_digest: "digest".to_owned(),
            ..UserCommandConfig::default()
        }),
        ..DumpTriggerConfig::default()
    };
    let slow = DumpTriggerConfig {
        kind: "suspicious_event".to_owned(),
        event: Some(SuspiciousEventConfig {
            kind: "slow_query".to_owned(),
            ..SuspiciousEventConfig::default()
        }),
        ..DumpTriggerConfig::default()
    };
    let config = FlightRecorderConfig {
        enabled_categories: vec!["general".to_owned()],
        dump_trigger: DumpTriggerConfig {
            kind: "and".to_owned(),
            and: vec![
                user,
                DumpTriggerConfig {
                    kind: "or".to_owned(),
                    or: vec![
                        slow,
                        DumpTriggerConfig {
                            kind: "sampling".to_owned(),
                            sampling: 3,
                            ..DumpTriggerConfig::default()
                        },
                    ],
                    ..DumpTriggerConfig::default()
                },
            ],
            ..DumpTriggerConfig::default()
        },
    };

    let compiled = config.compile().unwrap();
    assert_eq!(compiled.name_mapping.len(), 3);
    assert_eq!(compiled.truth_table, vec![0b011, 0b101]);
    assert!(check_truth_table(0b111, &compiled.truth_table));
    assert!(!check_truth_table(0b010, &compiled.truth_table));
}

/// 模式、环形缓冲与 trace id 布局对齐 Go。
#[test]
fn mode_ring_buffer_and_trace_id_match_go_layout() {
    let _guard = super::test_support::test_guard();
    assert_eq!(normalize_mode(" FALSE ").unwrap(), "off");
    set_mode("base").unwrap();
    assert_eq!(current_mode(), "base");
    assert!(set_mode("verbose").is_err());
    assert_eq!(
        current_mode(),
        "base",
        "an invalid mode must not mutate state"
    );

    let trace = Arc::new(Trace::new_with_random(0x0102_0304));
    let ctx = Context::default().with_sink(trace);
    let trace_id = generate_trace_id(&ctx, 7, 11);
    assert_eq!(trace_id.len(), 20);
    assert_eq!(&trace_id[0..8], &7_u64.to_be_bytes());
    assert_eq!(&trace_id[8..16], &11_u64.to_be_bytes());
    assert_eq!(&trace_id[16..20], &0x0102_0304_u32.to_be_bytes());

    let ring = RingBufferSink::new(2);
    for name in ["one", "two", "three"] {
        ring.record(
            &Context::default(),
            Event {
                category: GENERAL,
                name: name.to_owned(),
                phase: Phase::Instant,
                timestamp_micros: 0,
                trace_id: Vec::new(),
                fields: Vec::new(),
            },
        );
    }
    let names: Vec<_> = ring
        .snapshot()
        .into_iter()
        .map(|event| event.name)
        .collect();
    assert_eq!(names, vec!["two", "three"]);
}
