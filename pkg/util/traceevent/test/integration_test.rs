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

//! `pkg/util/traceevent/test` 的集成测试。
//!
//! Go 包通过 session/testkit 验证这些契约；这里同样使用标准 mock-store 会话，
//! 确保语句生命周期事件、`PrevTraceID` 和 `@@global.tidb_trace_event`
//! 都经过真实 SQL 执行链路。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_util_traceevent::adapter::{
    ClientCategory, TraceControlFlags, client_go_registered, handle_client_go_trace_event,
    handle_trace_control_extractor, register_with_client_go,
};
use astersql_util_traceevent::flightrecorder::{
    DumpTriggerConfig, FlightRecorderConfig, SuspiciousEventConfig, Trace, UserCommandConfig,
    check_flight_recorder_dump_trigger, close_flight_recorder, start_http_flight_recorder,
    start_log_flight_recorder,
};
use astersql_util_traceevent::traceevent::{
    Context, Event, Field, GENERAL, KV_REQUEST, STMT_LIFECYCLE, TIKV_READ_DETAILS, TIKV_REQUEST,
    TIKV_WRITE_DETAILS, current_mode, current_sink, generate_trace_id, set_mode, set_sink,
    trace_event,
};

/// 串行执行会修改进程级 trace-event recorder 或模式的测试，避免全局状态相互干扰。
fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 构造测试所需的最小 flight recorder 配置。
fn recorder_config(categories: &[&str], kind: &str, sampling: i64) -> FlightRecorderConfig {
    FlightRecorderConfig {
        enabled_categories: categories.iter().map(|value| (*value).to_owned()).collect(),
        dump_trigger: DumpTriggerConfig {
            kind: kind.to_owned(),
            sampling,
            ..DumpTriggerConfig::default()
        },
    }
}

/// 清空通道中前一阶段遗留的事件批次，避免影响后续触发器断言。
fn drain_events(event_ch: &Receiver<Vec<Event>>) {
    loop {
        match event_ch.try_recv() {
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

/// 判断批次中所有带 ID 的事件是否都属于指定 trace，并要求至少命中一个事件。
fn events_belong_to_trace(events: &[Event], trace_id: &[u8]) -> bool {
    if events.is_empty() || trace_id.is_empty() {
        return false;
    }
    let mut matched = false;
    for event in events {
        if event.trace_id.is_empty() {
            continue;
        }
        if event.trace_id != trace_id {
            return false;
        }
        matched = true;
    }
    matched
}

/// 在统一截止时间内跳过无关批次，等待指定 trace 的完整事件批次。
fn wait_trace_events(event_ch: &Receiver<Vec<Event>>, trace_id: &[u8]) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "failed to find trace events: trace_id={trace_id:?}"
        );
        let events = event_ch.recv_timeout(remaining).unwrap_or_else(|error| {
            panic!("failed to find trace events: trace_id={trace_id:?}, error={error}")
        });
        if events_belong_to_trace(&events, trace_id) {
            return events;
        }
    }
}

/// 判断批次首个带 ID 的事件是否属于候选 trace 集合，保持 Go 辅助函数语义。
fn events_belong_to_any_trace(events: &[Event], trace_ids: &HashSet<Vec<u8>>) -> bool {
    if events.is_empty() || trace_ids.is_empty() {
        return false;
    }
    for event in events {
        if event.trace_id.is_empty() {
            continue;
        }
        return trace_ids.contains(&event.trace_id);
    }
    false
}

/// 对应 Go `TestPrevTraceIDPersistence`：验证语句事件携带上一条语句的 trace ID，
/// 且当前语句会生成不同的 trace ID。
#[test]
fn test_prev_trace_id_persistence() {
    let _guard = test_guard();
    close_flight_recorder();
    set_mode("base").unwrap();
    start_log_flight_recorder(recorder_config(&["stmt_lifecycle"], "sampling", 1)).unwrap();

    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("CREATE TABLE trace_test (id INT PRIMARY KEY)", Vec::new());
    let session = testkit.Session();
    session.ResetPrevTraceIDForTest().unwrap();
    let trace = Arc::new(Trace::new_with_random(0x0102_0304));
    let previous_sink = current_sink();
    set_sink(Some(trace.clone()));
    testkit.MustExec("INSERT INTO trace_test VALUES (1)", Vec::new());
    let first_trace_id = session.PrevTraceIDForTest();
    testkit.MustExec("INSERT INTO trace_test VALUES (2)", Vec::new());
    let second_trace_id = session.PrevTraceIDForTest();
    set_sink(Some(previous_sink));
    assert!(!first_trace_id.is_empty());
    assert_ne!(first_trace_id, second_trace_id);

    let event = trace
        .events()
        .into_iter()
        .find(|event| event.name == "stmt.start" && event.trace_id == second_trace_id)
        .expect("second statement must produce stmt.start");
    let previous = event
        .fields
        .iter()
        .find(|field| field.key == "prev_trace_id")
        .and_then(|field| field.value.as_str())
        .expect("stmt.start must include prev_trace_id");
    assert_eq!(previous, hex(&first_trace_id));
    close_flight_recorder();
}

/// 对应 Go `TestTraceControlIntegration`：没有 sink 时仍可提取类别标志；
/// 存在匹配的 flight recorder trace 时还会请求立即记录日志。
#[test]
fn test_trace_control_integration() {
    let _guard = test_guard();
    close_flight_recorder();
    start_log_flight_recorder(recorder_config(&["tikv_request"], "sampling", 1)).unwrap();

    register_with_client_go();
    assert!(client_go_registered());
    let flags = handle_trace_control_extractor(&Context::default());
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));
    assert!(!flags.contains(TraceControlFlags::TIKV_WRITE_DETAILS));
    assert!(!flags.contains(TraceControlFlags::TIKV_READ_DETAILS));
    assert!(!flags.contains(TraceControlFlags::IMMEDIATE_LOG));

    let trace = Arc::new(Trace::new_with_random(7));
    trace.mark_bits(0);
    let flags = handle_trace_control_extractor(&Context::default().with_sink(trace));
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));
    assert!(flags.contains(TraceControlFlags::IMMEDIATE_LOG));

    close_flight_recorder();
}

/// 对应 Go `TestFlightRecorder`：覆盖类别过滤、trace 匹配、采样，
/// 以及用户命令和可疑事件两类转储触发器。
#[test]
fn test_flight_recorder() {
    let _guard = test_guard();
    close_flight_recorder();
    set_mode("base").unwrap();
    let (event_tx, event_ch) = sync_channel::<Vec<Event>>(1024);

    // 基础转储与类别过滤：未启用的类别不产生批次，启用的类别按 trace ID 返回。
    let recorder = start_http_flight_recorder(
        event_tx.clone(),
        recorder_config(&["kv_request"], "sampling", 1),
    )
    .unwrap();
    let trace = Arc::new(Trace::new_with_random(11));
    let ctx = Context::default()
        .with_trace_id(vec![1, 2, 3])
        .with_sink(trace.clone());
    trace_event(&ctx, GENERAL, "not-enabled", Vec::new());
    trace.discard_or_flush(&ctx);
    assert!(event_ch.try_recv().is_err());

    trace_event(&ctx, KV_REQUEST, "kv-request", Vec::new());
    trace.mark_bits(0);
    trace.discard_or_flush(&ctx);
    let events = wait_trace_events(&event_ch, &[1, 2, 3]);
    assert!(events.iter().all(|event| event.category == KV_REQUEST));
    recorder.close();

    // 辅助判定函数保持 Go 版本对空批次、混合批次及首个非空事件的处理语义。
    assert!(!events_belong_to_trace(&[], &[1]));
    assert!(!events_belong_to_trace(&events, &[]));
    assert!(events_belong_to_trace(&events, &[1, 2, 3]));
    let mut trace_ids = HashSet::new();
    trace_ids.insert(vec![1, 2, 3]);
    assert!(events_belong_to_any_trace(&events, &trace_ids));

    // 与 Go 测试一致，采样间隔为 5 时，每五次触发只保留一次。
    let sampling =
        start_http_flight_recorder(event_tx.clone(), recorder_config(&["*"], "sampling", 5))
            .unwrap();
    drain_events(&event_ch);
    let trace_ids: HashSet<Vec<u8>> = (0..10).map(|value| vec![value]).collect();
    let mut matched_events = Vec::new();
    for trace_id in &trace_ids {
        let trace = Arc::new(Trace::new_with_random(17));
        let ctx = Context::default()
            .with_trace_id(trace_id.clone())
            .with_sink(trace.clone());
        trace_event(&ctx, GENERAL, "select", Vec::new());
        check_flight_recorder_dump_trigger(&ctx, "dump_trigger.sampling", |config| {
            sampling.check_sampling(config)
        });
        trace.discard_or_flush(&ctx);
    }
    while let Ok(events) = event_ch.try_recv() {
        if events_belong_to_any_trace(&events, &trace_ids) {
            matched_events.push(events);
        }
    }
    assert_eq!(matched_events.len(), 2);
    sampling.close();

    // 用户命令触发器：匹配的命令转储一个批次，不匹配时通道保持为空。
    let user = start_http_flight_recorder(
        event_tx.clone(),
        FlightRecorderConfig {
            enabled_categories: vec!["*".to_owned()],
            dump_trigger: DumpTriggerConfig {
                kind: "user_command".to_owned(),
                user_command: Some(UserCommandConfig {
                    kind: "sql_regexp".to_owned(),
                    sql_regexp: r"select \* from t".to_owned(),
                    ..UserCommandConfig::default()
                }),
                ..DumpTriggerConfig::default()
            },
        },
    )
    .unwrap();
    let trace = Arc::new(Trace::new_with_random(23));
    let ctx = Context::default()
        .with_trace_id(vec![9])
        .with_sink(trace.clone());
    trace_event(&ctx, GENERAL, "insert", Vec::new());
    check_flight_recorder_dump_trigger(&ctx, "dump_trigger.user_command.sql_regexp", |_| false);
    trace.discard_or_flush(&ctx);
    assert!(event_ch.try_recv().is_err());
    trace_event(&ctx, GENERAL, "select", Vec::new());
    check_flight_recorder_dump_trigger(&ctx, "dump_trigger.user_command.sql_regexp", |_| true);
    trace.discard_or_flush(&ctx);
    assert_eq!(event_ch.try_recv().unwrap().len(), 1);
    user.close();

    // 可疑查询失败复用相同的事件收集与转储链路。
    let suspicious = start_http_flight_recorder(
        event_tx.clone(),
        FlightRecorderConfig {
            enabled_categories: vec!["*".to_owned()],
            dump_trigger: DumpTriggerConfig {
                kind: "suspicious_event".to_owned(),
                event: Some(SuspiciousEventConfig {
                    kind: "query_fail".to_owned(),
                    ..SuspiciousEventConfig::default()
                }),
                ..DumpTriggerConfig::default()
            },
        },
    )
    .unwrap();
    let trace = Arc::new(Trace::new_with_random(29));
    let ctx = Context::default()
        .with_trace_id(vec![10])
        .with_sink(trace.clone());
    trace_event(&ctx, GENERAL, "query-failed", Vec::new());
    check_flight_recorder_dump_trigger(&ctx, "dump_trigger.suspicious_event", |_| true);
    trace.discard_or_flush(&ctx);
    assert_eq!(event_ch.try_recv().unwrap().len(), 1);
    suspicious.close();
}

/// 对应 Go `TestTiDBTraceEventVariable`：验证通过 SQL 变量进入 full 模式后，
/// 事件能够跨越标准会话执行边界进入 trace sink。
#[test]
fn test_ti_db_trace_event_variable() {
    // Go skips this SQL integration test for the classic kernel.
    if astersql_config_kerneltype::IsClassic() {
        return;
    }
    let _guard = test_guard();
    close_flight_recorder();
    set_mode("off").unwrap();
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    let trace = Arc::new(Trace::new_with_random(31));
    let previous_sink = current_sink();
    set_sink(Some(trace.clone()));
    testkit.MustExec(
        "SET @@global.tidb_trace_event = '{\"enabled_categories\":[\"*\"],\"dump_trigger\":{\"type\":\"sampling\",\"sampling\":1}}'",
        Vec::new(),
    );
    let config = testkit.MustQuery("SELECT @@global.tidb_trace_event", Vec::new());
    assert!(config.Rows()[0][0].contains("enabled_categories"));
    testkit.MustExec("USE test", Vec::new());
    testkit.MustExec("CREATE TABLE trace_variable (id INT)", Vec::new());
    testkit.MustQuery("SELECT * FROM trace_variable", Vec::new());
    let events = trace.events();
    set_sink(Some(previous_sink));
    assert!(
        events
            .iter()
            .any(|event| event.category == STMT_LIFECYCLE && event.name == "stmt.start"),
        "expected session trace events, got {events:?}"
    );
    assert_eq!(current_mode(), "full");
    close_flight_recorder();
}

/// 包迁移时保留的标准模式、类别和上下文 API 冒烟测试；
/// 该用例不依赖进程级 recorder 状态。
#[test]
fn trace_modes_categories_and_trace_identity_flow_through_canonical_api() {
    let _guard = test_guard();
    assert_eq!(set_mode("off").unwrap(), "off");
    assert_eq!(set_mode("BASE").unwrap(), "base");
    assert_eq!(set_mode("full").unwrap(), "full");
    let context = Context::default().with_trace_id(vec![1, 2, 3, 4]);
    assert_eq!(context.trace_id(), &[1, 2, 3, 4]);
    assert_eq!(handle_trace_control_extractor(&context), Default::default());

    close_flight_recorder();
    handle_client_go_trace_event(
        &context,
        ClientCategory::Other(99),
        "unregistered",
        vec![Field::string("source", "client-go")],
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
