// Copyright 2026 AsterSQL.

// `pkg/util/promutil` crate 入口：Prometheus 指标工厂与 Registry 封装。
//
// 再导出 `factory` / `registry`；用于 TiDB/AsterSQL 各子系统统一创建与注册指标。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

pub mod factory;
pub mod registry;
pub use factory::*;
pub use registry::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;
