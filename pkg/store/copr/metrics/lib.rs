// Copyright 2026 AsterSQL.

// Coprocessor 相关 DistSQL 指标（metrics）crate 入口。
//
// 提供 Prometheus `CounterVec` 形式的 copr 缓存计数器，并 re-export
// `copr_metrics` 中按 label（hit/miss/evict）拆分的惰性全局计数器。
// DistSQL 指分布式 SQL 执行路径上的统计。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

/// DistSQL 命名空间下的 coprocessor 缓存计数器定义与初始化。
pub mod metrics {
    use prometheus::{CounterVec, Opts};

    /// 全局 `copr_cache` 计数器向量（label：`type`）；需先 `init_dist_sql_metrics`。
    pub static mut DistSQLCoprCacheCounter: Option<CounterVec> = None;

    /// 幂等初始化 DistSQL coprocessor 缓存计数器。
    pub fn init_dist_sql_metrics() {
        unsafe {
            if DistSQLCoprCacheCounter.is_none() {
                DistSQLCoprCacheCounter = Some(
                    CounterVec::new(
                        Opts::new("copr_cache", "coprocessor cache hit, evict and miss number")
                            .namespace("tidb")
                            .subsystem("distsql"),
                        &["type"],
                    )
                    .expect("valid DistSQL coprocessor cache counter options"),
                );
            }
        }
    }
}

/// 指向同目录 `metrics.rs` 的 copr 指标别名模块。
#[path = "../../../../pkg/store/copr/metrics/metrics.rs"]
pub mod copr_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
