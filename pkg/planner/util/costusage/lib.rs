// Copyright 2026 AsterSQL.

// 计划代价（cost）计算辅助类型与运算。
//
// 代价模型（cost model）为优化器比较候选执行计划提供数值估计；
// 本模块独立于 `base` 接口，避免与 planner 形成循环依赖。对应 Go 的 `costusage` 包。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// CostVer2、代价标志位与加减乘除等代价运算。
pub mod cost_misc;
pub use cost_misc::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "cost_misc_test.rs"]
mod cost_misc_test;
