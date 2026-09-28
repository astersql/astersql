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

// traceevent adapter 单元测试：控制标志提取、类别解析与默认配置。

use astersql_util_traceevent::adapter::{TraceControlFlags, handle_trace_control_extractor};
use astersql_util_traceevent::flightrecorder::{
    FlightRecorderConfig, Trace, close_flight_recorder, parse_categories, start_log_flight_recorder,
};
use astersql_util_traceevent::traceevent::{
    Context, GENERAL, TIKV_READ_DETAILS, TIKV_REQUEST, TIKV_WRITE_DETAILS, TXN_LIFECYCLE,
    parse_trace_category,
};
use std::sync::Arc;

/// 关闭旧记录器并以给定类别启动日志飞行记录器。
fn start_with_categories(categories: &[&str]) {
    close_flight_recorder();
    let mut config = FlightRecorderConfig::default();
    config.initialize();
    config.enabled_categories = categories.iter().map(|value| (*value).to_owned()).collect();
    start_log_flight_recorder(config).expect("flight recorder must start");
}

/// 验证类别标志、IMMEDIATE_LOG 与并发提取器稳定性。
#[test]
fn test_trace_control_extractor() {
    let _guard = super::test_support::test_guard();

    start_with_categories(&["tikv_request"]);
    let flags = handle_trace_control_extractor(&Context::default());
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));
    assert!(!flags.contains(TraceControlFlags::IMMEDIATE_LOG));

    let trace = Arc::new(Trace::new_with_random(7));
    let ctx = Context::default().with_sink(trace.clone());
    let flags = handle_trace_control_extractor(&ctx);
    assert!(!flags.contains(TraceControlFlags::IMMEDIATE_LOG));
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));

    trace.mark_bits(0);
    let flags = handle_trace_control_extractor(&ctx);
    assert!(flags.contains(TraceControlFlags::IMMEDIATE_LOG));
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));

    for (category, expected) in [
        ("tikv_request", TraceControlFlags::TIKV_REQUEST),
        ("tikv_write_details", TraceControlFlags::TIKV_WRITE_DETAILS),
        ("tikv_read_details", TraceControlFlags::TIKV_READ_DETAILS),
    ] {
        start_with_categories(&[category]);
        let flags = handle_trace_control_extractor(&Context::default());
        assert_eq!(
            flags.contains(TraceControlFlags::TIKV_REQUEST),
            expected == TraceControlFlags::TIKV_REQUEST
        );
        assert_eq!(
            flags.contains(TraceControlFlags::TIKV_WRITE_DETAILS),
            expected == TraceControlFlags::TIKV_WRITE_DETAILS
        );
        assert_eq!(
            flags.contains(TraceControlFlags::TIKV_READ_DETAILS),
            expected == TraceControlFlags::TIKV_READ_DETAILS
        );
    }

    start_with_categories(&["tikv_request", "tikv_write_details", "tikv_read_details"]);
    let trace = Arc::new(Trace::new_with_random(9));
    trace.mark_bits(0);
    let flags = handle_trace_control_extractor(&Context::default().with_sink(trace));
    assert!(flags.contains(TraceControlFlags::IMMEDIATE_LOG));
    assert!(flags.contains(TraceControlFlags::TIKV_REQUEST));
    assert!(flags.contains(TraceControlFlags::TIKV_WRITE_DETAILS));
    assert!(flags.contains(TraceControlFlags::TIKV_READ_DETAILS));

    let trace = Arc::new(Trace::new_with_random(11));
    let ctx = Context::default().with_sink(trace.clone());
    let mut threads = Vec::new();
    for _ in 0..100 {
        let ctx = ctx.clone();
        threads.push(std::thread::spawn(move || {
            let _ = handle_trace_control_extractor(&ctx);
        }));
    }
    for _ in 0..10 {
        let trace = trace.clone();
        threads.push(std::thread::spawn(move || trace.mark_bits(0)));
    }
    for thread in threads {
        thread.join().expect("concurrent extractor must not panic");
    }
    close_flight_recorder();
}

/// 验证 TikV 相关类别名称解析与 Display。
#[test]
fn test_category_parsing() {
    let _guard = super::test_support::test_guard();
    for (input, expected) in [
        ("tikv_request", TIKV_REQUEST),
        ("tikv_write_details", TIKV_WRITE_DETAILS),
        ("tikv_read_details", TIKV_READ_DETAILS),
    ] {
        let category = parse_trace_category(input);
        assert_eq!(category, expected);
        assert_eq!(category.to_string(), input);
    }
}

/// 验证默认启用类别：请求、事务生命周期与 GENERAL。
#[test]
fn test_default_configuration() {
    let _guard = super::test_support::test_guard();
    let mut config = FlightRecorderConfig::default();
    config.initialize();
    let categories = parse_categories(&config.enabled_categories);
    assert!(categories.contains(TIKV_REQUEST));
    assert!(!categories.contains(TIKV_WRITE_DETAILS));
    assert!(!categories.contains(TIKV_READ_DETAILS));
    assert!(categories.contains(TXN_LIFECYCLE));
    assert!(categories.contains(GENERAL));
}
