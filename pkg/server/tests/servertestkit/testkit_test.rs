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

// `servertestkit` 的基础契约测试。
//
// 验证租约时长解析与 Go `time.ParseDuration` 的兼容边界，并确认默认测试套件
// 已启动 SQL 与 status 监听器，能够对外提供健康状态。

use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_util_topsql_state::GlobalState;

use super::{create_tidb_test_suite, create_tidb_test_top_sql_suite, parse_duration};

/// 覆盖 Go `parseDuration` 接受的常用时长写法，以及 Rust 不支持负时长的边界。
#[test]
fn parse_duration_accepts_go_duration_syntax_and_rejects_negative_values() {
    assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
    assert_eq!(parse_duration("-0").unwrap(), Duration::ZERO);
    assert_eq!(parse_duration("2").unwrap(), Duration::from_secs(2));
    assert_eq!(parse_duration("3s").unwrap(), Duration::from_secs(3));
    assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
    assert_eq!(
        parse_duration("1h30m").unwrap(),
        Duration::from_secs(90 * 60)
    );
    assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
    assert_eq!(parse_duration("500us").unwrap(), Duration::from_micros(500));
    assert_eq!(parse_duration("1µs").unwrap(), Duration::from_micros(1));
    assert!(parse_duration("-1").is_err());
    assert!(parse_duration("abc").is_err());
}

/// 套件构造完成后必须暴露可用的 SQL 与 status 监听器。
#[test]
fn create_tidb_test_suite_binds_listeners_and_status() {
    let suite = create_tidb_test_suite();
    assert!(suite.server.listener_addr().is_some());
    assert!(suite.server.status_listener_addr().is_some());
    assert!(suite.server.health());
    assert_ne!(suite.test_server_client.port, 0);
    assert_ne!(suite.test_server_client.status_port, 0);
    let status = suite
        .test_server_client
        .fetch_status("/status")
        .expect("status endpoint");
    assert!(status.is_success());
}

/// Go 通过全局系统变量将 TopSQL 每个时间序列的语句上限收紧为 5。
#[test]
fn create_tidb_test_top_sql_suite_limits_time_series_statement_count() {
    let suite = create_tidb_test_top_sql_suite();
    assert_eq!(GlobalState.MaxStatementCount.load(Ordering::SeqCst), 5);
    drop(suite);
}
