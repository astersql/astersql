// Copyright 2026 AsterSQL.

// InfoSchema 指标子 crate 根模块。
//
// 定义 InfoCache / load schema 相关的 Prometheus 向量，并重新导出
// `metrics.rs` 中按 label 绑定的计数器与直方图句柄。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_infoschema_metrics;

use prometheus::{CounterVec, Gauge, Opts};
use std::sync::LazyLock;

/// InfoSchema V2 cache outcomes, labelled with `evict`, `hit`, or `miss`.
pub static InfoSchemaV2CacheCounter: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new(
            "infoschema_v2_cache",
            "infoschema cache v2 hit, evict and miss number",
        )
        .namespace("tidb")
        .subsystem("domain"),
        &["type"],
    )
    .unwrap()
});

/// Current number of tables in the InfoSchema V2 cache.
pub static InfoSchemaV2CacheObjCnt: LazyLock<Gauge> = LazyLock::new(|| {
    Gauge::with_opts(
        Opts::new(
            "infoschema_v2_cache_count",
            "infoschema cache v2 table count",
        )
        .namespace("tidb")
        .subsystem("domain"),
    )
    .unwrap()
});

/// Current InfoSchema V2 cache size in bytes.
pub static InfoSchemaV2CacheMemUsage: LazyLock<Gauge> = LazyLock::new(|| {
    Gauge::with_opts(
        Opts::new("infoschema_v2_cache_size", "infoschema cache v2 size")
            .namespace("tidb")
            .subsystem("domain"),
    )
    .unwrap()
});

/// Configured InfoSchema V2 cache memory limit in bytes.
pub static InfoSchemaV2CacheMemLimit: LazyLock<Gauge> = LazyLock::new(|| {
    Gauge::with_opts(
        Opts::new("infoschema_v2_cache_limit", "infoschema cache v2 limit")
            .namespace("tidb")
            .subsystem("domain"),
    )
    .unwrap()
});

/// 包内原始 Prometheus 指标向量（CounterVec / HistogramVec）。
pub mod metrics {
    use prometheus::{CounterVec, HistogramOpts, HistogramVec, Opts};
    use std::sync::LazyLock;

    /// InfoCache 读写计数：label 为 action（get/hit）与 type（latest/ts/version）。
    pub static InfoCacheCounters: LazyLock<CounterVec> = LazyLock::new(|| {
        CounterVec::new(
            Opts::new("infocache_counters", "Counters of infoCache: get/hit.")
                .namespace("tidb")
                .subsystem("domain"),
            &["action", "type"],
        )
        .unwrap()
    });
    /// 加载 schema 次数，按 type（如 snapshot）区分。
    pub static LoadSchemaCounter: LazyLock<CounterVec> = LazyLock::new(|| {
        CounterVec::new(
            Opts::new("load_schema_total", "Counter of load schema")
                .namespace("tidb")
                .subsystem("domain"),
            &["type"],
        )
        .unwrap()
    });
    /// 加载 schema 耗时直方图（秒），按 action（total/load-diff/load-all）分桶。
    pub static LoadSchemaDuration: LazyLock<HistogramVec> = LazyLock::new(|| {
        HistogramVec::new(
            HistogramOpts::new(
                "load_schema_duration_seconds",
                "Bucketed histogram of processing time (s) in load schema.",
            )
            .namespace("tidb")
            .subsystem("domain")
            .buckets(prometheus::exponential_buckets(0.001, 2.0, 20).unwrap()),
            &["action"],
        )
        .unwrap()
    });
}

#[path = "../../../pkg/infoschema/metrics/metrics.rs"]
mod implementation;
pub use implementation::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "lib_test.rs"]
mod lib_test;
