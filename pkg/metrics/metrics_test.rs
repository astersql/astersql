// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// External-style smoke tests corresponding to Go `package metrics_test`.
//
// 对应 Go `package metrics_test` 的外部冒烟测试：覆盖 PanicCounter 自增、
// InitMetrics/RegisterMetrics 注册路径，以及执行错误到标签的映射。

use astersql_parser_terror::ErrResultUndetermined;

use crate::main_test::ensure_test_env;
use crate::metrics::{self, LabelDomain, PanicCounter};
use crate::server::ExecuteErrorToLabel;

/// TestMetrics corresponds to Go's TestMetrics: Inc must not panic.
/// 对应 Go TestMetrics：对 PanicCounter 自增不得 panic。
#[test]
fn test_metrics() {
    ensure_test_env();
    PanicCounter.with_label_values(&[LabelDomain]).inc();
}

/// TestRegisterMetrics corresponds to Go's TestRegisterMetrics.
/// 对应 Go TestRegisterMetrics：在独立注册表中完整初始化并注册。
#[test]
fn test_register_metrics() {
    if crate::main_test::run_in_isolated_process("metrics_test::test_register_metrics") {
        return;
    }
    ensure_test_env();
    unsafe {
        metrics::InitMetrics().expect("InitMetrics");
        crate::ddl::BatchAddIdxHistogram
            .as_ref()
            .expect("BatchAddIdxHistogram initialized")
            .with_label_values(&["add-index"])
            .observe(0.001);
        metrics::RegisterMetrics().expect("register all package metrics");
    }

    assert!(
        prometheus::gather()
            .iter()
            .any(|family| family.name() == "tidb_ddl_batch_add_idx_duration_seconds"),
        "RegisterMetrics must expose Go's BatchAddIdxHistogram collector"
    );
}

/// TestExecuteErrorToLabel corresponds to Go's TestExecuteErrorToLabel.
///
/// Go passes `errors.New` / `terror.ErrResultUndetermined` into
/// `ExecuteErrorToLabel(error)`; the Rust port takes the already-extracted RFC
/// code (`None` for plain errors).
///
/// 对应 Go TestExecuteErrorToLabel：普通错误映射 unknown，
/// ErrResultUndetermined 映射其 RFC code。
#[test]
fn test_execute_error_to_label() {
    ensure_test_env();
    assert_eq!("unknown", ExecuteErrorToLabel(None));
    assert_eq!(
        "global:2",
        ExecuteErrorToLabel(Some(ErrResultUndetermined.RFCCode().as_str()))
    );
}

#[test]
fn ia_scan_collectors_register_sql_and_database_labels() {
    if crate::main_test::run_in_isolated_process(
        "metrics_test::ia_scan_collectors_register_sql_and_database_labels",
    ) {
        return;
    }
    ensure_test_env();
    unsafe {
        metrics::InitMetrics().unwrap();
        metrics::RegisterMetrics().unwrap();
    }
    crate::server::RecordQueryScanMetrics(
        "Select",
        "db1",
        2,
        Some((11, 7, 3, 4096, std::time::Duration::from_millis(5))),
    );
    let families = prometheus::gather();
    for (name, expected) in [
        ("tidb_server_ia_cache_hit_count", 7.0),
        ("tidb_server_ia_remote_read_segment_count", 3.0),
        ("tidb_server_ia_remote_read_segment_size_bytes", 4096.0),
    ] {
        let metric = &families
            .iter()
            .find(|family| family.name() == name)
            .unwrap()
            .get_metric()[0];
        assert_eq!(metric.get_counter().value(), expected);
        assert!(
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "sql_type" && label.value() == "Select")
        );
        assert!(
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "db" && label.value() == "db1")
        );
    }
    let histogram = families
        .iter()
        .find(|family| family.name() == "tidb_server_ia_remote_read_segment_wait_duration_seconds")
        .unwrap()
        .get_metric()[0]
        .get_histogram();
    assert_eq!(histogram.sample_count(), 1);
    assert_eq!(histogram.sample_sum(), 0.005);
    assert_eq!(histogram.get_bucket().len(), 20);
    for (index, bucket) in histogram.get_bucket().iter().enumerate() {
        assert_eq!(bucket.upper_bound(), 0.00005 * 2f64.powi(index as i32));
    }
}

#[test]
fn statement_and_command_durations_use_independent_histograms() {
    if crate::main_test::run_in_isolated_process(
        "metrics_test::statement_and_command_durations_use_independent_histograms",
    ) {
        return;
    }
    ensure_test_env();
    unsafe {
        metrics::InitMetrics().unwrap();
        metrics::RegisterMetrics().unwrap();
    }
    crate::server::RecordQueryDuration("Insert", "app", "rg", 1.25);
    crate::server::RecordCommandDuration("MultiStmt", "app", "rg", 2.5);

    let families = prometheus::gather();
    for (name, sql_type, sum) in [
        ("tidb_server_handle_query_duration_seconds", "Insert", 1.25),
        (
            "tidb_server_handle_command_duration_seconds",
            "MultiStmt",
            2.5,
        ),
    ] {
        let metric = families
            .iter()
            .find(|family| family.name() == name)
            .and_then(|family| {
                family.get_metric().iter().find(|metric| {
                    metric
                        .get_label()
                        .iter()
                        .any(|label| label.name() == "sql_type" && label.value() == sql_type)
                })
            })
            .expect("duration metric with requested SQL type");
        assert_eq!(metric.get_histogram().sample_count(), 1);
        assert_eq!(metric.get_histogram().sample_sum(), sum);
    }
}
