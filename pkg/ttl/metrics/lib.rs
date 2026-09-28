// Copyright 2026 AsterSQL.

// TTL（Time To Live，生存时间）指标子系统的 crate 根。
//
// 声明 Prometheus 向量指标（Histogram/Counter/Gauge）与标签常量，
// 并通过 `ttl_metrics` 子模块暴露相位追踪（PhaseTracer）与水位延迟聚合等实现。
// TTL 用于按列过期时间自动清理表数据；本模块只负责观测面，不执行清理逻辑。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_ttl_metrics;

use std::sync::LazyLock;

/// Prometheus 指标定义与标签常量（对应 Go `ttl/metrics` 包内的变量声明）。
pub mod metrics {
    use super::LazyLock;
    use prometheus::{CounterVec, GaugeVec, HistogramOpts, HistogramVec, Opts};

    /// SQL 类型标签键（如 select / delete）。
    pub const LblSQLType: &str = "sql_type";
    /// 执行结果标签键。
    pub const LblResult: &str = "result";
    /// 成功结果标签值。
    pub const LblOK: &str = "ok";
    /// 失败结果标签值。
    pub const LblError: &str = "error";
    /// 通用类型标签键（worker / job 状态等）。
    pub const LblType: &str = "type";
    /// Worker 相位标签键。
    pub const LblPhase: &str = "phase";
    /// 名称标签键（水位延迟分桶名等）。
    pub const LblName: &str = "name";

    /// TTL 查询耗时直方图：按 SQL 类型与结果维度观测。
    pub static TTLQueryDuration: LazyLock<HistogramVec> = LazyLock::new(|| {
        HistogramVec::new(
            HistogramOpts::new(
                "ttl_query_duration",
                "Bucketed histogram of processing time (s) of handled TTL queries.",
            )
            .namespace("tidb")
            .subsystem("server")
            .buckets(
                prometheus::exponential_buckets(0.01, 2.0, 20)
                    .expect("Go TTL query histogram uses valid exponential buckets"),
            ),
            &[LblSQLType, LblResult],
        )
        .unwrap()
    });
    /// 已处理过期行计数：扫描/删除路径累计行数。
    pub static TTLProcessedExpiredRowsCounter: LazyLock<CounterVec> = LazyLock::new(|| {
        CounterVec::new(
            Opts::new(
                "ttl_processed_expired_rows",
                "The count of expired rows processed in TTL jobs",
            )
            .namespace("tidb")
            .subsystem("server"),
            &[LblSQLType, LblResult],
        )
        .unwrap()
    });
    /// TTL Job 状态 Gauge（running / cancelling 等）。
    pub static TTLJobStatus: LazyLock<GaugeVec> = LazyLock::new(|| {
        GaugeVec::new(
            Opts::new("ttl_job_status", "The jobs count in the specified status")
                .namespace("tidb")
                .subsystem("server"),
            &[LblType],
        )
        .unwrap()
    });
    /// TTL Task 状态 Gauge（scanning / deleting 等）。
    pub static TTLTaskStatus: LazyLock<GaugeVec> = LazyLock::new(|| {
        GaugeVec::new(
            Opts::new("ttl_task_status", "The tasks count in the specified status")
                .namespace("tidb")
                .subsystem("server"),
            &[LblType],
        )
        .unwrap()
    });
    /// Worker 各相位耗时累计 Counter（按 worker 类型与 phase 标签）。
    pub static TTLPhaseTime: LazyLock<CounterVec> = LazyLock::new(|| {
        CounterVec::new(
            Opts::new("ttl_phase_time", "The time spent in each phase")
                .namespace("tidb")
                .subsystem("server"),
            &[LblType, LblPhase],
        )
        .unwrap()
    });
    /// 水位（watermark，上次成功调度时间相对当前的延迟）分桶 Gauge。
    pub static TTLWatermarkDelay: LazyLock<GaugeVec> = LazyLock::new(|| {
        GaugeVec::new(
            Opts::new(
                "ttl_watermark_delay",
                "Bucketed delay time in seconds for TTL tables.",
            )
            .namespace("tidb")
            .subsystem("server"),
            &[LblType, LblName],
        )
        .unwrap()
    });
}

/// 相位追踪、预绑定直方图句柄与水位延迟更新逻辑。
#[path = "metrics.rs"]
pub mod ttl_metrics;

#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
