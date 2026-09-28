// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Aster 迁移回归：对齐 Go TTL metrics 中相位、水位分桶与上下文行为。

use astersql_ttl_metrics::metrics;
use astersql_ttl_metrics::ttl_metrics::*;
use prometheus::core::Collector;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// 相位切换累积上报、同名再进入与 EndPhase 清空，与 Go 行为一致。
#[test]
fn phase_tracer_matches_go_transition_and_end_behavior() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let reports = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::clone(&now);
    let output = Arc::clone(&reports);
    let mut tracer = newPhaseTracer(
        move || *clock.lock().unwrap(),
        move |phase, duration| output.lock().unwrap().push((phase.to_owned(), duration)),
    );

    *now.lock().unwrap() += Duration::from_secs(2);
    tracer.EnterPhase("p1");
    assert!(reports.lock().unwrap().is_empty());
    assert_eq!(tracer.Phase(), "p1");

    *now.lock().unwrap() += Duration::from_secs(5);
    tracer.EnterPhase("p2");
    assert_eq!(
        reports.lock().unwrap().as_slice(),
        &[("p1".to_owned(), Duration::from_secs(5))]
    );

    reports.lock().unwrap().clear();
    *now.lock().unwrap() += Duration::from_secs(10);
    tracer.EnterPhase("p2");
    assert_eq!(
        reports.lock().unwrap().as_slice(),
        &[("p2".to_owned(), Duration::from_secs(10))]
    );

    reports.lock().unwrap().clear();
    *now.lock().unwrap() += Duration::from_secs(20);
    tracer.EndPhase();
    assert_eq!(
        reports.lock().unwrap().as_slice(),
        &[("p2".to_owned(), Duration::from_secs(20))]
    );
    assert_eq!(tracer.Phase(), "");
}

/// 水位分桶阈值与顺序保持 Go 侧 WaterMarkScheduleDelayNames 语义。
#[test]
fn watermark_buckets_preserve_go_thresholds_and_order() {
    assert_eq!(
        getWaterMarkScheduleDelayName(Duration::from_secs(3600)),
        "01 hour"
    );
    // 超过 1h 阈值后落入下一有效桶（Go 中 02 hour 与 01 hour 同阈值，实际跳到 06 hour）。
    assert_eq!(
        getWaterMarkScheduleDelayName(Duration::from_secs(3600) + Duration::from_nanos(1)),
        "06 hour"
    );
    assert_eq!(
        getWaterMarkScheduleDelayName(Duration::from_secs(72 * 3600)),
        "72 hour"
    );
    assert_eq!(
        getWaterMarkScheduleDelayName(Duration::from_secs(72 * 3600) + Duration::from_nanos(1)),
        "others"
    );
    assert_eq!(
        WaterMarkScheduleDelayNames[1].Delay,
        Duration::from_secs(3600)
    );
    assert_eq!(
        WaterMarkScheduleDelayNames[6].Delay,
        Duration::from_secs(72 * 3600)
    );
}

/// UpdateDelayMetrics 分桶计数与 ClearDelayMetrics 清空行为对齐 Go。
#[test]
fn update_and_clear_delay_metrics_match_go_behavior() {
    ClearDelayMetrics();
    let mut records = HashMap::new();
    records.insert(
        1,
        DelayMetricsRecord::new(
            1,
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
            Duration::from_secs(10),
        ),
    );
    records.insert(
        2,
        DelayMetricsRecord::new(
            2,
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
            Duration::from_secs(7 * 3600),
        ),
    );
    UpdateDelayMetrics(&records);

    assert_eq!(
        metrics::TTLWatermarkDelay
            .with_label_values(&["schedule", "01 hour"])
            .get(),
        1.0
    );
    assert_eq!(
        metrics::TTLWatermarkDelay
            .with_label_values(&["schedule", "12 hour"])
            .get(),
        1.0
    );
    assert_eq!(
        metrics::TTLWatermarkDelay
            .with_label_values(&["schedule", "others"])
            .get(),
        0.0
    );

    // 空 map 更新会把各分桶重置为 0。
    UpdateDelayMetrics(&HashMap::new());
    assert_eq!(
        metrics::TTLWatermarkDelay
            .with_label_values(&["schedule", "01 hour"])
            .get(),
        0.0
    );
    ClearDelayMetrics();
    assert!(
        metrics::TTLWatermarkDelay
            .collect()
            .iter()
            .all(|family| family.get_metric().is_empty())
    );
}

/// PhaseContext 挂载/取出 tracer，以及缺失时返回 None。
#[test]
fn phase_context_round_trip_and_missing_value_match_go_behavior() {
    let context = PhaseContext::default();
    assert!(PhaseTracerFromCtx(&context).is_none());
    let tracer = Arc::new(Mutex::new(newPhaseTracer(Instant::now, |_, _| {})));
    let context = CtxWithPhaseTracer(context, Arc::clone(&tracer));
    assert!(Arc::ptr_eq(&PhaseTracerFromCtx(&context).unwrap(), &tracer));
}

/// 指标描述符与 Go InitTTLMetrics 的 namespace/subsystem/help/labels/buckets 一致。
#[test]
fn metric_descriptors_and_query_buckets_match_go() {
    let query_desc = metrics::TTLQueryDuration.desc();
    assert_eq!(query_desc.len(), 1);
    assert_eq!(query_desc[0].fq_name, "tidb_server_ttl_query_duration");
    assert_eq!(
        query_desc[0].help,
        "Bucketed histogram of processing time (s) of handled TTL queries."
    );
    assert_eq!(
        query_desc[0].variable_labels,
        vec![
            metrics::LblSQLType.to_owned(),
            metrics::LblResult.to_owned()
        ]
    );

    let metric = metrics::TTLQueryDuration.with_label_values(&["select", metrics::LblOK]);
    metric.observe(0.01);
    let buckets = metric
        .collect()
        .into_iter()
        .flat_map(|family| family.get_metric().iter().cloned().collect::<Vec<_>>())
        .next()
        .expect("query histogram metric")
        .get_histogram()
        .get_bucket()
        .iter()
        .map(|bucket| bucket.get_upper_bound())
        .collect::<Vec<_>>();
    assert_eq!(buckets.len(), 20);
    assert_eq!(buckets[0], 0.01);
    assert_eq!(buckets[19], 0.01 * 2_f64.powi(19));

    let descriptors = [
        (
            metrics::TTLProcessedExpiredRowsCounter.desc(),
            "tidb_server_ttl_processed_expired_rows",
            "The count of expired rows processed in TTL jobs",
        ),
        (
            metrics::TTLJobStatus.desc(),
            "tidb_server_ttl_job_status",
            "The jobs count in the specified status",
        ),
        (
            metrics::TTLTaskStatus.desc(),
            "tidb_server_ttl_task_status",
            "The tasks count in the specified status",
        ),
        (
            metrics::TTLPhaseTime.desc(),
            "tidb_server_ttl_phase_time",
            "The time spent in each phase",
        ),
        (
            metrics::TTLWatermarkDelay.desc(),
            "tidb_server_ttl_watermark_delay",
            "Bucketed delay time in seconds for TTL tables.",
        ),
    ];
    for (desc, name, help) in descriptors {
        assert_eq!(desc.len(), 1);
        assert_eq!(desc[0].fq_name, name);
        assert_eq!(desc[0].help, help);
    }
}
