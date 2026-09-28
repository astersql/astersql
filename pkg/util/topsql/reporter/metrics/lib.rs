// Copyright 2026 AsterSQL.

// TopSQL reporter 指标 crate：父级 Prometheus 向量与子模块绑定入口。
//
// 父向量（ignored/duration/data）在此初始化；`reporter_metrics` 再按标签切出
// 与 Go 一致的全局 Counter/Histogram 句柄，供上报路径打点。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as topsql_reporter_metrics;

/// 父级 Prometheus 指标定义与初始化。
pub mod metrics {
    use prometheus::{CounterVec, HistogramOpts, HistogramVec, Opts};

    /// 上报成功结果标签。
    pub const LblOK: &str = "ok";
    /// 上报失败结果标签。
    pub const LblError: &str = "error";
    /// 被忽略样本计数（按 type 标签细分）。
    pub static mut TopSQLIgnoredCounter: Option<CounterVec> = None;
    /// 上报耗时直方图（按 type/result 细分）。
    pub static mut TopSQLReportDurationHistogram: Option<HistogramVec> = None;
    /// 上报数据量直方图（按 type 细分）。
    pub static mut TopSQLReportDataHistogram: Option<HistogramVec> = None;

    /// 创建并挂载三个父级向量到静态全局。
    pub fn init_parent_metrics() {
        unsafe {
            TopSQLIgnoredCounter = Some(
                CounterVec::new(
                    Opts::new(
                        "ignored_total",
                        "Counter of ignored top-sql metrics (register-sql, register-plan, collect-data and report-data), normally it should be 0.",
                    )
                    .namespace("tidb")
                    .subsystem("topsql"),
                    &["type"],
                )
                .unwrap(),
            );
            TopSQLReportDurationHistogram = Some(
                HistogramVec::new(
                    HistogramOpts::new(
                        "report_duration_seconds",
                        "Bucket histogram of reporting time (s) to the top-sql agent",
                    )
                    .namespace("tidb")
                    .subsystem("topsql")
                    .buckets(
                        prometheus::exponential_buckets(0.001, 2.0, 24)
                            .expect("valid top-sql report duration buckets"),
                    ),
                    &["type", "result"],
                )
                .unwrap(),
            );
            TopSQLReportDataHistogram = Some(
                HistogramVec::new(
                    HistogramOpts::new(
                        "report_data_total",
                        "Bucket histogram of reporting records/sql/plan count to the top-sql agent.",
                    )
                    .namespace("tidb")
                    .subsystem("topsql")
                    .buckets(
                        prometheus::exponential_buckets(1.0, 2.0, 20)
                            .expect("valid top-sql report data buckets"),
                    ),
                    &["type"],
                )
                .unwrap(),
            );
        }
    }
}

#[path = "metrics.rs"]
pub mod reporter_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
