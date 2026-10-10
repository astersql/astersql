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

// traceevent 单元测试：类别开关、模式、环形缓冲、dump 冷却与基准形状。
//
// 对齐 Go `traceevent_test.go`；全局状态经 `test_support` 串行保护。

use astersql_util_traceevent::flightrecorder::{
    FlightRecorderConfig, close_flight_recorder, start_log_flight_recorder,
};
use astersql_util_traceevent::traceevent::*;
use std::sync::Arc;

/// 关闭旧实例后按类别列表启动日志飞行记录器。
fn start_recorder(categories: &[&str]) {
    close_flight_recorder();
    let mut config = FlightRecorderConfig::default();
    config.initialize();
    config.enabled_categories = categories.iter().map(|value| (*value).to_owned()).collect();
    start_log_flight_recorder(config).expect("flight recorder must start");
}

/// 构造测试用 Instant 事件（类别固定 TXN_LIFECYCLE）。
fn event(name: &str, timestamp_micros: i64, fields: Vec<Field>) -> Event {
    Event {
        category: TXN_LIFECYCLE,
        name: name.to_owned(),
        phase: Phase::Instant,
        timestamp_micros,
        trace_id: Vec::new(),
        fields,
    }
}

/// 重置 sink、模式、环形缓冲与全局飞行记录器。
fn reset_state() {
    set_sink(None);
    let _ = set_mode(MODE_BASE);
    flight_recorder().discard_or_flush();
    reset_last_dump_time_for_test();
    close_flight_recorder();
}

/// 综合用例：类别启用、模式过滤、trace_id 透传、日志开关与 dump 冷却。
#[test]
fn test_suite() {
    let _guard = super::test_support::test_guard();
    reset_state();

    start_recorder(&["*"]);
    assert!(is_enabled(TXN_LIFECYCLE));
    start_recorder(&["-", "txn_lifecycle"]);
    assert!(!is_enabled(TXN_LIFECYCLE));
    start_recorder(&["*"]);
    assert!(is_enabled(TXN_LIFECYCLE));
    start_recorder(&[]);
    assert!(!is_enabled(TXN_LIFECYCLE));
    start_recorder(&["*"]);
    assert!(is_enabled(TXN_LIFECYCLE));

    start_recorder(&[]);
    set_mode(MODE_FULL).unwrap();
    flight_recorder().discard_or_flush();
    let sink = Arc::new(RingBufferSink::new(8));
    set_sink(Some(sink.clone()));
    trace_event(
        &Context::default(),
        TXN_LIFECYCLE,
        "should-not-record",
        vec![Field::i64("value", 1)],
    );
    assert!(flight_recorder().snapshot().is_empty());
    assert!(sink.snapshot().is_empty());

    start_recorder(&["*"]);
    set_mode(MODE_FULL).unwrap();
    flight_recorder().discard_or_flush();
    let sink = Arc::new(RingBufferSink::new(8));
    set_sink(Some(sink.clone()));
    trace_event(
        &Context::default(),
        TXN_LIFECYCLE,
        "test-event",
        vec![Field::i64("count", 42), Field::string("scope", "unit-test")],
    );
    let recorded = flight_recorder().snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].category, TXN_LIFECYCLE);
    assert_eq!(recorded[0].name, "test-event");
    assert!(recorded[0].timestamp_micros > 0);
    assert_eq!(recorded[0].fields.len(), 2);
    assert_eq!(sink.snapshot().len(), 1);

    flight_recorder().discard_or_flush();
    let trace_id = vec![0x01, 0x10, 0xfe, 0xaa];
    let ctx = Context::default().with_trace_id(trace_id.clone());
    trace_event(
        &ctx,
        TXN_2PC,
        "trace-id-check",
        vec![Field::i64("value", 7)],
    );
    let recorded = flight_recorder().snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].trace_id, trace_id);

    flight_recorder().discard_or_flush();
    let sink = Arc::new(RingBufferSink::new(8));
    set_sink(Some(sink.clone()));
    set_mode(MODE_BASE).unwrap();
    trace_event(
        &Context::default(),
        TXN_LIFECYCLE,
        "disabled-log",
        vec![Field::i64("value", 1)],
    );
    assert_eq!(flight_recorder().snapshot().len(), 1);
    let disabled_logged = sink.snapshot().len();
    set_mode(MODE_FULL).unwrap();
    trace_event(
        &Context::default(),
        TXN_LIFECYCLE,
        "enabled-log",
        vec![Field::i64("value", 2)],
    );
    assert_eq!(flight_recorder().snapshot().len(), 2);
    let logged = sink.snapshot();
    assert_eq!(logged.len(), disabled_logged + 1);
    assert_eq!(logged.last().unwrap().name, "enabled-log");

    set_sink(None);
    flight_recorder().discard_or_flush();
    trace_event(
        &Context::default(),
        TXN_LIFECYCLE,
        "cooloff-test-event",
        vec![Field::i64("value", 1)],
    );
    let now = 1_000;
    assert!(dump_flight_recorder_to_logger_at(now) > 0);
    assert_eq!(dump_flight_recorder_to_logger_at(now), 0);
    assert!(dump_flight_recorder_to_logger_at(now + 11) > 0);
    reset_state();
}

/// set_mode / current_mode / normalize_mode 合法与非法路径。
#[test]
fn test_trace_event_modes() {
    let _guard = super::test_support::test_guard();
    assert_eq!(set_mode("base").unwrap(), MODE_BASE);
    assert_eq!(current_mode(), MODE_BASE);
    assert_eq!(set_mode("full").unwrap(), MODE_FULL);
    assert_eq!(current_mode(), MODE_FULL);
    assert_eq!(set_mode("off").unwrap(), MODE_OFF);
    assert_eq!(current_mode(), MODE_OFF);
    assert!(normalize_mode("invalid").is_err());
    reset_state();
}

/// 环形缓冲写满后 snapshot 按从旧到新顺序返回。
#[test]
fn test_ring_buffer_snapshot_order() {
    let _guard = super::test_support::test_guard();
    let recorder = RingBufferSink::new(2);
    recorder.record(&Context::default(), event("first", 1, vec![]));
    recorder.record(&Context::default(), event("second", 2, vec![]));
    assert_eq!(extract_names(&recorder.snapshot()), vec!["first", "second"]);
    recorder.record(&Context::default(), event("third", 3, vec![]));
    assert_eq!(extract_names(&recorder.snapshot()), vec!["second", "third"]);
}

/// 单次写入后 snapshot 字段完整保留。
#[test]
fn test_ring_buffer_flush_to() {
    let _guard = super::test_support::test_guard();
    let recorder = RingBufferSink::new(4);
    let expected = event(
        "flush",
        123_456,
        vec![Field::string("status", "ok"), Field::i64("count", 2)],
    );
    recorder.record(&Context::default(), expected.clone());
    let events = recorder.snapshot();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "flush");
    assert_eq!(events[0].category, TXN_LIFECYCLE);
    assert_eq!(events[0].timestamp_micros, expected.timestamp_micros);
    assert_eq!(events[0].fields.len(), 2);
}

/// Display 名称与已知类别及 unknown(N) 对齐。
#[test]
fn test_category_names() {
    let _guard = super::test_support::test_guard();
    for (category, name) in [
        (TXN_LIFECYCLE, "txn_lifecycle"),
        (TXN_2PC, "txn_2pc"),
        (TXN_LOCK_RESOLVE, "txn_lock_resolve"),
        (STMT_LIFECYCLE, "stmt_lifecycle"),
        (STMT_PLAN, "stmt_plan"),
        (KV_REQUEST, "kv_request"),
        (UNKNOWN_CLIENT, "unknown_client"),
        (TraceCategory(999), "unknown(999)"),
    ] {
        assert_eq!(category.to_string(), name);
    }
}

/// Go extractRandFromTraceID rejects every trace ID whose length is not exactly 20 bytes.
#[test]
fn rendering_rejects_oversized_trace_id() {
    let _guard = super::test_support::test_guard();
    let mut oversized_trace_id = vec![0; 21];
    oversized_trace_id[16..20].copy_from_slice(&1_u32.to_ne_bytes());
    let mut oversized = event("oversized", 1, vec![]);
    oversized.trace_id = oversized_trace_id;

    let rendered = convert_events_for_rendering(&[oversized]);

    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].tid, 0);
}

/// 基准形状：循环调用 trace_event（用于 off/full 对比）。
fn run_benchmark_shape(mode: &str) {
    start_recorder(&["*"]);
    set_mode(mode).unwrap();
    let ctx = Context::default();
    for iteration in 0..100 {
        trace_event(
            &ctx,
            TXN_LIFECYCLE,
            "benchmark",
            vec![
                Field::string("key", "value"),
                Field::i64("iteration", iteration),
            ],
        );
    }
}

/// MODE_OFF 下不应向环形缓冲写入。
#[test]
fn benchmark_trace_event_disabled() {
    let _guard = super::test_support::test_guard();
    reset_state();
    run_benchmark_shape(MODE_OFF);
    assert!(flight_recorder().snapshot().is_empty());
    reset_state();
}

/// MODE_FULL 且类别全开时应记录全部迭代事件。
#[test]
fn benchmark_trace_event_enabled() {
    let _guard = super::test_support::test_guard();
    reset_state();
    run_benchmark_shape(MODE_FULL);
    assert_eq!(flight_recorder().snapshot().len(), 100);
    reset_state();
}

/// 提取事件名列表便于断言顺序。
fn extract_names(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.name.as_str()).collect()
}
