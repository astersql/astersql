// Copyright 2026 AsterSQL.

// 规划器 core 包的 Prometheus 指标 crate 入口。
//
// 绑定 prometheus 1.3 API，导出 `planner_core_metrics` 模块中的计划缓存
// （plan cache）命中/未命中、伪估计与耗时直方图等指标句柄。

extern crate prometheus13 as prometheus;
extern crate self as astersql_planner_core_metrics;

/// 规划器指标定义与访问器（对应 Go `planner/core/metrics`）。
#[path = "metrics.rs"]
pub mod planner_core_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
