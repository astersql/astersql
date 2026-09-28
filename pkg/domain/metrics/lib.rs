// Copyright 2026 AsterSQL.

// Domain metrics（指标）迁移 crate 入口。
//
// 通过 `#[path]` 挂载共享的 Prometheus 封装、promutil、bindinfo、stats，
// 以及本目录下的 `domain_metrics` 具体指标绑定。测试配置下再挂载
// `migration_aster_unit_test`。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as astersql_domain_metrics;

/// 通用 metrics 包装层。
#[path = "../../../pkg/metrics/common/wrapper.rs"]
pub mod metricscommon;

#[path = "../../../pkg/util/promutil/factory.rs"]
mod promutil_factory;
#[path = "../../../pkg/util/promutil/registry.rs"]
mod promutil_registry;
/// Prometheus 工厂与注册表工具的再导出。
pub mod promutil {
    pub use crate::promutil_factory::*;
    pub use crate::promutil_registry::*;
}

#[path = "../../../pkg/metrics/bindinfo.rs"]
mod bindinfo;

/// 指标 label 名：类型。
pub const LblType: &str = "type";
/// 指标 label 名：结果。
pub const LblResult: &str = "result";

/// 统计相关共享 Counter / Gauge 定义。
#[path = "../../../pkg/metrics/stats.rs"]
pub mod stats;

/// 将 stats 中 Plan Replayer / HistoricalStats 相关句柄导出为 `metrics`。
pub mod metrics {
    pub use crate::stats::{
        HistoricalStatsCounter, PlanReplayerRegisterTaskGauge, PlanReplayerTaskCounter,
    };
}

/// Domain 专用指标变量绑定（InitMetricsVars）。
#[path = "../../../pkg/domain/metrics/metrics.rs"]
pub mod domain_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
