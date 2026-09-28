// Copyright 2026 AsterSQL.

use prometheus::core::Collector;

use crate::gc_worker::{
    GCActionRegionResultCounter, GCConfigGauge, GCHistogram, GCJobFailureCounter,
    GCRegionTooManyLocksCounter, GCUnsafeDestroyRangeFailuresCounterVec, GCWorkerCounter,
    InitGCWorkerMetrics, StageTotal,
};

#[test]
fn gc_worker_metrics_match_go_descriptors_and_initialization() {
    if crate::main_test::run_in_isolated_process(
        "gc_worker_test::gc_worker_metrics_match_go_descriptors_and_initialization",
    ) {
        return;
    }
    InitGCWorkerMetrics();

    unsafe {
        let cases: [(&dyn Collector, &str, &[&str]); 7] = [
            (
                GCWorkerCounter.as_ref().unwrap(),
                "tidb_tikvclient_gc_worker_actions_total",
                &["type"],
            ),
            (
                GCHistogram.as_ref().unwrap(),
                "tidb_tikvclient_gc_seconds",
                &["stage"],
            ),
            (
                GCConfigGauge.as_ref().unwrap(),
                "tidb_tikvclient_gc_config",
                &["type"],
            ),
            (
                GCJobFailureCounter.as_ref().unwrap(),
                "tidb_tikvclient_gc_failure",
                &["type"],
            ),
            (
                GCActionRegionResultCounter.as_ref().unwrap(),
                "tidb_tikvclient_gc_action_result",
                &["type"],
            ),
            (
                GCRegionTooManyLocksCounter.as_ref().unwrap(),
                "tidb_tikvclient_gc_region_too_many_locks",
                &[],
            ),
            (
                GCUnsafeDestroyRangeFailuresCounterVec.as_ref().unwrap(),
                "tidb_tikvclient_gc_unsafe_destroy_range_failures",
                &["type"],
            ),
        ];

        for (collector, fq_name, labels) in cases {
            let desc = collector.desc()[0];
            assert_eq!(desc.fq_name, fq_name);
            assert_eq!(desc.variable_labels, labels);
        }
    }

    assert_eq!(StageTotal, "total");
}

#[test]
fn gc_histogram_uses_go_exponential_buckets() {
    if crate::main_test::run_in_isolated_process(
        "gc_worker_test::gc_histogram_uses_go_exponential_buckets",
    ) {
        return;
    }
    InitGCWorkerMetrics();

    let histogram = unsafe {
        GCHistogram
            .as_ref()
            .unwrap()
            .with_label_values(&[StageTotal])
    };
    histogram.observe(1.0);
    let metric = histogram.collect();
    let buckets = metric[0].get_metric()[0].get_histogram().get_bucket();

    assert_eq!(buckets.len(), 20);
    assert_eq!(buckets[0].upper_bound(), 1.0);
    assert_eq!(buckets[19].upper_bound(), 524_288.0);
}
