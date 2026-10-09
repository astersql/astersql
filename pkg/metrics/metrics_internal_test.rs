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

// Package-internal tests corresponding to Go `package metrics` tests in
// `metrics_internal_test.go`.
//
// 对应 Go `metrics_internal_test.go` 的包内测试：覆盖结果标签、
// RU v2 执行器计数缓存、语句摘要指标标签，以及 gRPC channelz 单例与 intest 跳过逻辑。

use prometheus::core::Collector;
use prometheus::{Counter, Gauge, Registry};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::main_test::ensure_test_env;
use crate::metrics::{
    GRPC_CHANNELZ_TEST_LOCK, OP_FAILED, OP_SUCC, RetLabel,
    cleanup_grpc_channelz_collector_for_test, init_grpc_channelz_collector_locked,
    register_external_metrics, setup_channelz_collector, with_grpc_channelz_collector_locked,
};
use crate::ru_v2::{
    InitRUV2Metrics, RUV2ByEngine, RUV2ByEngineTiKV, RUV2BySQLType, RUV2BySQLTypeDDL,
    RUV2Statements, RUV2TTLTotal, RUV2Total, RUV2Unit,
};
use crate::stmtsummary::{
    InitStmtSummaryMetrics, SetStmtSummaryWindowMetrics, StmtSummaryEvictedLogCounter,
    StmtSummaryEvictedLogResultDropped, StmtSummaryEvictedLogResultPersisted, StmtSummaryTypeV1,
    StmtSummaryTypeV2, StmtSummaryWindowEvictedCount, StmtSummaryWindowRecordCount,
};

/// 读取 Gauge 当前值。
fn read_gauge_value(gauge: &Gauge) -> f64 {
    gauge.get()
}

/// 读取 Counter 当前值。
fn read_counter_value(counter: &Counter) -> f64 {
    counter.get()
}

#[test]
fn ru_v2_metrics_initialize_once_under_concurrency() {
    let start = Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let start = start.clone();
        workers.push(std::thread::spawn(move || {
            start.wait();
            crate::ru_v2::InitRUV2Metrics();
            crate::ru_v2::AddRUV2Results(1.0, 1.0, 1.0, 3.0, "select");
        }));
    }
    start.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    unsafe {
        assert!(RUV2Total.is_some());
        assert!(RUV2BySQLType.is_some());
        assert!(RUV2ByEngine.is_some());
    }
}

#[test]
fn go_merge_6_ru_results_use_go_label_and_engine_contract() {
    if crate::main_test::run_in_isolated_process(
        "metrics_internal_test::go_merge_6_ru_results_use_go_label_and_engine_contract",
    ) {
        return;
    }
    ensure_test_env();
    use crate::ru_v2::{AddRUV2Results, RUV2ByEngine, RUV2BySQLType, RUV2Total};
    InitRUV2Metrics();
    AddRUV2Results(3.0, 4.0, 5.0, 12.0, "select");
    AddRUV2Results(1.0, 2.0, 3.0, 6.0, "unexpected");
    unsafe {
        assert_eq!(RUV2Total.as_ref().unwrap().get(), 18.0);
        assert_eq!(
            RUV2BySQLType
                .as_ref()
                .unwrap()
                .with_label_values(&["select"])
                .get(),
            12.0
        );
        assert_eq!(
            RUV2BySQLType
                .as_ref()
                .unwrap()
                .with_label_values(&["other"])
                .get(),
            6.0
        );
        assert_eq!(
            RUV2ByEngine
                .as_ref()
                .unwrap()
                .with_label_values(&["tikv"])
                .get(),
            4.0
        );
        assert_eq!(
            RUV2ByEngine
                .as_ref()
                .unwrap()
                .with_label_values(&["tidb"])
                .get(),
            6.0
        );
        assert_eq!(
            RUV2ByEngine
                .as_ref()
                .unwrap()
                .with_label_values(&["tiflash"])
                .get(),
            8.0
        );
    }
}

/// 统计 Collector 产出的全部 metric 序列条数。
fn count_collected_metrics(collector: &dyn Collector) -> usize {
    collector
        .collect()
        .iter()
        .map(|family| family.get_metric().len())
        .sum()
}

/// 按名称查找 MetricFamily。
fn find_metric_family<'a>(
    families: &'a [prometheus::proto::MetricFamily],
    name: &str,
) -> Option<&'a prometheus::proto::MetricFamily> {
    families.iter().find(|family| family.name() == name)
}

/// 判断 metric 是否带有指定 name=value 标签。
fn metric_has_label_value(metric: &prometheus::proto::Metric, name: &str, value: &str) -> bool {
    metric
        .get_label()
        .iter()
        .any(|label| label.name() == name && label.value() == value)
}

/// 验证 RetLabel 在成功 / 失败时分别返回 ok / err。
#[test]
fn test_ret_label() {
    ensure_test_env();
    assert_eq!(OP_SUCC, RetLabel::<String>(None));
    assert_eq!(OP_FAILED, RetLabel(Some(&String::from("test error"))));
}

/// The original RU metric definitions survive the RUv3-to-RUv2 rename.
#[test]
fn ru_metric_definitions_preserve_labels_values_and_registration() {
    if crate::main_test::run_in_isolated_process(
        "metrics_internal_test::ru_metric_definitions_preserve_labels_values_and_registration",
    ) {
        return;
    }
    use crate::session::{
        LblEngine, LblEngineTiFlash, LblEngineTiKV, LblSQLType, LblSQLTypeAnalyze, LblSQLTypeDDL,
        LblSQLTypeOther, LblSQLTypeRead, LblSQLTypeWrite,
    };
    assert_eq!(
        [
            LblSQLTypeDDL,
            LblSQLTypeRead,
            LblSQLTypeWrite,
            LblSQLTypeAnalyze,
            LblSQLTypeOther
        ],
        ["ddl", "read", "write", "analyze", "other"],
    );
    assert_eq!([LblEngineTiKV, LblEngineTiFlash], ["tikv", "tiflash"]);
    ensure_test_env();
    unsafe {
        crate::metrics::InitMetrics().unwrap();
        RUV2Total.as_ref().unwrap().inc_by(1.0);
        RUV2BySQLType
            .as_ref()
            .unwrap()
            .with_label_values(&[LblSQLTypeRead])
            .inc_by(2.0);
        RUV2ByEngine
            .as_ref()
            .unwrap()
            .with_label_values(&[LblEngineTiKV])
            .inc_by(3.0);
        crate::metrics::RegisterMetrics().unwrap();
    }
    let families = prometheus::gather();
    for (name, help, label, value) in [
        (
            "tidb_ruv2_ru_total",
            "Counter of resource unit consumption for RU v2.",
            None,
            1.0,
        ),
        (
            "tidb_ruv2_ru_by_sql_type_total",
            "Counter of resource unit consumption by SQL type for RU v2.",
            Some((LblSQLType, LblSQLTypeRead)),
            2.0,
        ),
        (
            "tidb_ruv2_ru_by_engine_total",
            "Counter of resource unit consumption by engine for RU v2.",
            Some((LblEngine, LblEngineTiKV)),
            3.0,
        ),
    ] {
        let family = find_metric_family(&families, name).expect(name);
        assert_eq!(
            family.get_field_type(),
            prometheus::proto::MetricType::COUNTER
        );
        assert_eq!(family.help(), help);
        let metric = family
            .get_metric()
            .iter()
            .find(|metric| match label {
                Some((key, expected)) => metric_has_label_value(metric, key, expected),
                None => metric.get_label().is_empty(),
            })
            .expect(name);
        assert_eq!(metric.get_label().len(), usize::from(label.is_some()));
        assert_eq!(metric.get_counter().value(), value);
    }
    assert!(
        !families
            .iter()
            .any(|family| family.name().starts_with("tidb_ruv3_"))
    );
}

#[test]
fn ddl_job_ru_updates_total_sql_type_and_tikv_counters() {
    if crate::main_test::run_in_isolated_process(
        "metrics_internal_test::ddl_job_ru_updates_total_sql_type_and_tikv_counters",
    ) {
        return;
    }
    ensure_test_env();
    InitRUV2Metrics();
    crate::ru_v2::AddDDLJobRU(12.5);
    unsafe {
        assert_eq!(RUV2Total.as_ref().unwrap().get(), 12.5);
        assert_eq!(RUV2BySQLTypeDDL.as_ref().unwrap().get(), 12.5);
        assert_eq!(RUV2ByEngineTiKV.as_ref().unwrap().get(), 12.5);
    }
}

/// Go RU v2 collector names and label dimensions are part of the monitoring contract.
#[test]
fn go_merge_6_ru_metric_definitions() {
    if crate::main_test::run_in_isolated_process(
        "metrics_internal_test::go_merge_6_ru_metric_definitions",
    ) {
        return;
    }
    ensure_test_env();
    InitRUV2Metrics();
    let registry = Registry::new();
    unsafe {
        registry
            .register(Box::new(RUV2Total.as_ref().unwrap().clone()))
            .unwrap();
        registry
            .register(Box::new(RUV2TTLTotal.as_ref().unwrap().clone()))
            .unwrap();
        registry
            .register(Box::new(RUV2BySQLType.as_ref().unwrap().clone()))
            .unwrap();
        registry
            .register(Box::new(RUV2ByEngine.as_ref().unwrap().clone()))
            .unwrap();
        registry
            .register(Box::new(RUV2Unit.as_ref().unwrap().clone()))
            .unwrap();
        registry
            .register(Box::new(RUV2Statements.as_ref().unwrap().clone()))
            .unwrap();
        RUV2Total.as_ref().unwrap().inc();
        RUV2TTLTotal.as_ref().unwrap().inc();
        RUV2BySQLType
            .as_ref()
            .unwrap()
            .with_label_values(&["select"])
            .inc();
        RUV2ByEngine
            .as_ref()
            .unwrap()
            .with_label_values(&["tikv"])
            .inc();
        RUV2Unit
            .as_ref()
            .unwrap()
            .with_label_values(&["tikv", "hash_agg", "cpu_work"])
            .inc();
        RUV2Statements
            .as_ref()
            .unwrap()
            .with_label_values(&["success", "incomplete"])
            .inc();
    }
    let families = registry.gather();
    for name in [
        "tidb_ruv2_ru_total",
        "tidb_ruv2_ttl_ru_total",
        "tidb_ruv2_ru_by_sql_type_total",
        "tidb_ruv2_ru_by_engine_total",
        "tidb_ruv2_unit_total",
        "tidb_ruv2_statements_total",
    ] {
        assert!(find_metric_family(&families, name).is_some(), "{name}");
    }
    let has_label = |family: &str, name: &str, value: &str| {
        find_metric_family(&families, family)
            .unwrap()
            .get_metric()
            .iter()
            .any(|metric| metric_has_label_value(metric, name, value))
    };
    assert!(has_label(
        "tidb_ruv2_ru_by_sql_type_total",
        "sql_type",
        "select"
    ));
    assert!(has_label("tidb_ruv2_ru_by_engine_total", "engine", "tikv"));
    assert!(has_label("tidb_ruv2_unit_total", "opclass", "hash_agg"));
    assert!(has_label("tidb_ruv2_unit_total", "unit", "cpu_work"));
    assert!(has_label(
        "tidb_ruv2_statements_total",
        "reason",
        "incomplete"
    ));
}

#[test]
fn test_stmt_summary_metric_labels() {
    if crate::main_test::run_in_isolated_process(
        "metrics_internal_test::test_stmt_summary_metric_labels",
    ) {
        return;
    }
    ensure_test_env();
    unsafe {
        InitStmtSummaryMetrics();
        assert_eq!(
            0,
            count_collected_metrics(StmtSummaryWindowRecordCount.as_ref().unwrap())
        );
        assert_eq!(
            0,
            count_collected_metrics(StmtSummaryWindowEvictedCount.as_ref().unwrap())
        );
        assert_eq!(
            0,
            count_collected_metrics(StmtSummaryEvictedLogCounter.as_ref().unwrap())
        );

        // 写入 V1 后应各产生一条带标签的序列。
        SetStmtSummaryWindowMetrics(StmtSummaryTypeV1, 3.0, 1.0);
        assert_eq!(
            1,
            count_collected_metrics(StmtSummaryWindowRecordCount.as_ref().unwrap())
        );
        assert_eq!(
            1,
            count_collected_metrics(StmtSummaryWindowEvictedCount.as_ref().unwrap())
        );
        assert_eq!(
            3.0,
            read_gauge_value(
                &StmtSummaryWindowRecordCount
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV1])
            )
        );
        assert_eq!(
            1.0,
            read_gauge_value(
                &StmtSummaryWindowEvictedCount
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV1])
            )
        );

        // 再写入 V2，采集条数变为 2，且 V2 值独立。
        SetStmtSummaryWindowMetrics(StmtSummaryTypeV2, 5.0, 2.0);
        assert_eq!(
            2,
            count_collected_metrics(StmtSummaryWindowRecordCount.as_ref().unwrap())
        );
        assert_eq!(
            2,
            count_collected_metrics(StmtSummaryWindowEvictedCount.as_ref().unwrap())
        );
        assert_eq!(
            5.0,
            read_gauge_value(
                &StmtSummaryWindowRecordCount
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV2])
            )
        );
        assert_eq!(
            2.0,
            read_gauge_value(
                &StmtSummaryWindowEvictedCount
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV2])
            )
        );

        // 淘汰日志按 persisted / dropped 结果标签分别计数。
        StmtSummaryEvictedLogCounter
            .as_ref()
            .unwrap()
            .with_label_values(&[StmtSummaryTypeV2, StmtSummaryEvictedLogResultPersisted])
            .inc_by(3.0);
        StmtSummaryEvictedLogCounter
            .as_ref()
            .unwrap()
            .with_label_values(&[StmtSummaryTypeV2, StmtSummaryEvictedLogResultDropped])
            .inc();
        assert_eq!(
            2,
            count_collected_metrics(StmtSummaryEvictedLogCounter.as_ref().unwrap())
        );
        assert_eq!(
            3.0,
            read_counter_value(
                &StmtSummaryEvictedLogCounter
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV2, StmtSummaryEvictedLogResultPersisted,])
            )
        );
        assert_eq!(
            1.0,
            read_counter_value(
                &StmtSummaryEvictedLogCounter
                    .as_ref()
                    .unwrap()
                    .with_label_values(&[StmtSummaryTypeV2, StmtSummaryEvictedLogResultDropped,])
            )
        );
    }
}

/// 验证 channelz 采集器单例：重复 init 复用同一句柄，cleanup 后状态清空。
#[test]
fn test_grpc_channelz_collector_singleton() {
    ensure_test_env();
    let _serial = GRPC_CHANNELZ_TEST_LOCK
        .lock()
        .expect("channelz test lock poisoned");
    cleanup_grpc_channelz_collector_for_test();
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            cleanup_grpc_channelz_collector_for_test();
        }
    }
    let _guard = Guard;

    let first_id = with_grpc_channelz_collector_locked(|state| {
        init_grpc_channelz_collector_locked(state).expect("init channelz collector");
        let first = state.collector().expect("collector created").handle_id();
        init_grpc_channelz_collector_locked(state).expect("init channelz collector twice");
        let second = state
            .collector()
            .expect("collector still present")
            .handle_id();
        assert_eq!(first, second, "singleton collector must be reused");
        first
    });
    let _ = first_id;

    cleanup_grpc_channelz_collector_for_test();

    with_grpc_channelz_collector_locked(|state| {
        assert!(state.collector().is_none());
        assert!(!state.registered());
    });
}

/// 验证测试环境下 setup_channelz_collector 直接跳过，不创建也不注册。
#[test]
fn test_setup_channelz_collector_skipped_in_test() {
    ensure_test_env();
    let _serial = GRPC_CHANNELZ_TEST_LOCK
        .lock()
        .expect("channelz test lock poisoned");
    cleanup_grpc_channelz_collector_for_test();
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            cleanup_grpc_channelz_collector_for_test();
        }
    }
    let _guard = Guard;

    assert!(
        cfg!(test) || astersql_util_intest::InTest.load(std::sync::atomic::Ordering::SeqCst),
        "Go requires intest.InTest; cargo test sets cfg(test)"
    );
    setup_channelz_collector().expect("setup skipped in test returns Ok");

    with_grpc_channelz_collector_locked(|state| {
        assert!(state.collector().is_none());
        assert!(!state.registered());
    });
}

/// 验证采集器 gather 含 fetch_errors 指标，且过滤内部 bufnet / 空 remote。
#[test]
fn test_grpc_channelz_collector_gather() {
    ensure_test_env();
    let _serial = GRPC_CHANNELZ_TEST_LOCK
        .lock()
        .expect("channelz test lock poisoned");
    cleanup_grpc_channelz_collector_for_test();
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            cleanup_grpc_channelz_collector_for_test();
        }
    }
    let _guard = Guard;

    let collector = with_grpc_channelz_collector_locked(|state| {
        init_grpc_channelz_collector_locked(state).expect("init channelz collector");
        state.collector().cloned().expect("collector")
    });

    let registry = Registry::new();
    registry
        .register(Box::new(collector))
        .expect("register channelz collector");
    let families = registry.gather();

    assert!(find_metric_family(&families, "tidb_grpc_channelz_fetch_errors_total").is_some());
    for family in &families {
        for metric in family.get_metric() {
            assert!(!metric_has_label_value(metric, "target", "bufnet"));
            assert!(!metric_has_label_value(
                metric,
                "target",
                "passthrough:///bufnet"
            ));
            if family.name().starts_with("tidb_grpc_channelz_socket_") {
                assert!(!metric_has_label_value(metric, "remote", ""));
            }
        }
    }
}

/// Go InitMetrics/RegisterMetrics also initializes and registers DXF, ingest and timer metrics.
#[test]
fn test_external_subsystem_metrics_are_initialized_and_registered() {
    ensure_test_env();
    unsafe { crate::metrics::InitMetrics().expect("initialize package metrics") };

    let registry = Registry::new();
    register_external_metrics(&registry).expect("register external subsystem metrics");

    astersql_dxf_framework_dxfmetric::InitDistTaskMetrics()
        .UsedSlotsGauge
        .with_label_values(&["default"])
        .set(1.0);
    astersql_ingestor_ingestmetric::WriteAPIDuration
        .read()
        .unwrap()
        .as_ref()
        .expect("ingest metrics initialized")
        .observe(0.001);
    astersql_timer_metrics::TimerScopeCounter("test", "tick").inc();

    let names = registry
        .gather()
        .into_iter()
        .map(|family| family.name().to_owned())
        .collect::<BTreeSet<_>>();
    assert!(names.contains("tidb_disttask_used_slots"));
    assert!(names.contains("tidb_ingestor_write_ingest_api_duration"));
    assert!(names.contains("tidb_server_timer_event_count"));
}

/// The TiKV collectors toggled by Go's simplified mode must exist with identical descriptors.
#[test]
fn test_tikv_simplified_collectors_match_go_descriptors() {
    let collectors = crate::tikv_client_metrics::unused_collectors();
    let names = collectors
        .iter()
        .flat_map(|collector| collector.desc())
        .map(|desc| desc.fq_name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "tidb_tikvclient_rawkv_kv_size_bytes",
            "tidb_tikvclient_rawkv_cmd_seconds",
            "tidb_sli_tikv_read_throughput",
            "tidb_sli_tikv_small_read_duration",
            "tidb_tikvclient_batch_wait_overload",
            "tidb_tikvclient_batch_client_reset",
            "tidb_tikvclient_request_retry_times",
            "tidb_tikvclient_kv_status_api_duration",
        ]
        .map(str::to_owned)
    );

    let registry = Registry::new();
    crate::tikv_client_metrics::RegisterMetrics(&registry).expect("register TiKV metrics");
    for collector in crate::tikv_client_metrics::unused_collectors() {
        registry
            .unregister(collector)
            .expect("unregister TiKV metric");
    }
    crate::tikv_client_metrics::RegisterMetrics(&registry).expect("re-register TiKV metrics");
}

/// A real channelz snapshot must emit leaf-subchannel and socket metrics, not only fetch errors.
#[test]
fn test_grpc_channelz_snapshot_emits_leaf_subchannel_and_socket_metrics() {
    let families = crate::metrics::collect_channelz_snapshots_for_test(
        r#"{
            "channel": [{
                "ref": {"channelId": "1"},
                "data": {"target": "dns:///tikv"},
                "subchannelRef": [{"subchannelId": "2"}]
            }],
            "end": true
        }"#,
        &[
            (
                "subchannel",
                2,
                r#"{
                    "subchannel": {
                        "ref": {"subchannelId": "2"},
                        "data": {
                            "target": "ipv4:127.0.0.1:20160",
                            "callsStarted": "4",
                            "callsSucceeded": "3",
                            "callsFailed": "1"
                        },
                        "socketRef": [{"socketId": "3"}]
                    }
                }"#,
            ),
            (
                "socket",
                3,
                r#"{
                    "socket": {
                        "ref": {"socketId": "3"},
                        "data": {
                            "streamsStarted": "5",
                            "streamsSucceeded": "4",
                            "streamsFailed": "1",
                            "messagesSent": "8",
                            "messagesReceived": "7",
                            "keepAlivesSent": "2"
                        },
                        "local": {"other_address": {"name": "ipv4:127.0.0.1:4000"}},
                        "remote": {"other_address": {"name": "ipv4:127.0.0.1:20160"}}
                    }
                }"#,
            ),
        ],
    );

    assert!(find_metric_family(&families, "tidb_grpc_channelz_channel_calls_total").is_some());
    assert!(find_metric_family(&families, "tidb_grpc_channelz_socket_streams_total").is_some());
    assert!(find_metric_family(&families, "tidb_grpc_channelz_socket_messages_total").is_some());
    assert!(find_metric_family(&families, "tidb_grpc_channelz_socket_keepalives_total").is_some());
    let calls = find_metric_family(&families, "tidb_grpc_channelz_channel_calls_total").unwrap();
    assert_eq!(calls.get_metric().len(), 3);
    assert_eq!(
        calls.get_metric()[0]
            .get_counter()
            .as_ref()
            .unwrap()
            .value(),
        4.0
    );
    assert!(metric_has_label_value(
        &calls.get_metric()[0],
        "kind",
        "subchannel"
    ));
    let streams = find_metric_family(&families, "tidb_grpc_channelz_socket_streams_total").unwrap();
    assert_eq!(streams.get_metric().len(), 3);
    assert!(metric_has_label_value(
        &streams.get_metric()[0],
        "remote",
        "ipv4:127.0.0.1:20160"
    ));
}
