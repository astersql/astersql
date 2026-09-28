// Copyright 2026 AsterSQL.

// DXF 指标（dxfmetric）crate 根模块。
//
// 汇出自定义 Collector 与 DistTask 包级指标向量（Init/Register）。

#![allow(non_snake_case, non_upper_case_globals)]

/// 自引用别名，供迁移期单元测试引用本 crate。
extern crate self as astersql_dxf_framework_dxfmetric;

/// 任务/子任务快照的自定义 Prometheus Collector。
pub mod collector;
/// DistTask 包级 Gauge/Counter 定义与注册。
pub mod metric;
/// 再导出 collector 公共 API。
pub use collector::*;
/// 再导出 metric 公共 API。
pub use metric::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// Aster 迁移单元测试（仅 test）。
mod migration_aster_unit_test;
