// Copyright 2026 AsterSQL.
// metricscommon crate 入口：导出 wrapper，并挂接迁移/wrapper 单元测试模块。
#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 常量标签与指标构造包装层。
pub mod wrapper;
pub use wrapper::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "wrapper_test.rs"]
mod wrapper_test;
