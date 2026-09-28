// Copyright 2026 AsterSQL.

// Ingest 指标（metric）子包入口。
//
// 重导出 `metric` 模块中的 Prometheus 直方图与初始化/注册函数，
// 供 ingestor 统计 write/ingest API 耗时。

#![allow(non_snake_case, non_upper_case_globals)]

pub mod metric;
pub use metric::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
