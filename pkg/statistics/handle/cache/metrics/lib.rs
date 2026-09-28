// Copyright 2026 AsterSQL.

// 统计缓存 Prometheus 指标包入口。
//
// 内嵌父级 `CounterVec`/`GaugeVec` 初始化（对应 Go 侧 `pkg/metrics`），
// 并通过路径再导出 `metrics.rs` 中的标签绑定句柄；测试配置下挂载迁移单元测试。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as astersql_statistics_handle_cache_metrics;

/// 父级指标向量定义与初始化，供 `cache_metrics` 按标签切分具体 Counter/Gauge。
pub mod metrics {
    use prometheus::{CounterVec, GaugeVec, Opts};

    /// 统计缓存操作计数向量，标签 `type` 区分 miss/hit/update 等。
    pub static mut StatsCacheCounter: Option<CounterVec> = None;
    /// 统计缓存数值度量向量，标签 `type` 区分 track/capacity 等。
    pub static mut StatsCacheGauge: Option<GaugeVec> = None;

    /// 初始化父级 CounterVec/GaugeVec，命名空间与子系统对齐 Go 的 tidb/statistics。
    pub fn init_parent_metrics() {
        unsafe {
            StatsCacheCounter = Some(
                CounterVec::new(
                    Opts::new("stats_cache_op", "Counter for statsCache operation")
                        .namespace("tidb")
                        .subsystem("statistics"),
                    &["type"],
                )
                .unwrap(),
            );
            StatsCacheGauge = Some(
                GaugeVec::new(
                    Opts::new("stats_cache_val", "gauge of stats cache value")
                        .namespace("tidb")
                        .subsystem("statistics"),
                    &["type"],
                )
                .unwrap(),
            );
        }
    }
}

#[path = "../../../../../pkg/statistics/handle/cache/metrics/metrics.rs"]
pub mod cache_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
