// Copyright 2026 AsterSQL.

use prometheus::core::Collector;

use crate::{
    InitMetricsVars,
    metrics::{InfoCacheCounters, LoadSchemaCounter, LoadSchemaDuration},
};

#[test]
fn collectors_preserve_go_fully_qualified_names() {
    assert_eq!(
        InfoCacheCounters.collect()[0].get_name(),
        "tidb_domain_infocache_counters"
    );
    assert_eq!(
        LoadSchemaCounter.collect()[0].get_name(),
        "tidb_domain_load_schema_total"
    );
    assert_eq!(
        LoadSchemaDuration.collect()[0].get_name(),
        "tidb_domain_load_schema_duration_seconds"
    );
}

#[test]
fn load_schema_duration_preserves_go_exponential_buckets() {
    InitMetricsVars();

    let family = LoadSchemaDuration.collect();
    let histogram = family[0]
        .get_metric()
        .iter()
        .find(|metric| metric.get_label()[0].get_value() == "total")
        .unwrap()
        .get_histogram();
    let upper_bounds: Vec<_> = histogram
        .get_bucket()
        .iter()
        .map(|bucket| bucket.get_upper_bound())
        .collect();
    let expected: Vec<_> = (0..20).map(|power| 0.001 * 2_f64.powi(power)).collect();

    assert_eq!(upper_bounds, expected);
}
