// Copyright 2026 AsterSQL.

// 统计处理子系统 Prometheus 指标 crate 入口。
//
// 再导出健康度分桶 Gauge、历史统计导出计数器及初始化函数。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_statistics_handle_metrics;

#[path = "metrics.rs"]
/// 指标定义与 `InitMetricsVars` 实现模块。
mod implementation;
pub use implementation::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：分桶配置、初始化与句柄复用。
mod migration_aster_unit_test;
