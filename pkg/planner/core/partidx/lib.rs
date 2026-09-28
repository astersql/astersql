// Copyright 2026 AsterSQL.

// 部分索引（partial index）约束检查子包入口。
//
// 再导出 `check_constraint` 中的表达式/区间类型与蕴含判定函数，
// 供分区索引选择与规划器约束校验使用。

#![allow(dead_code, non_snake_case)]

/// 约束蕴含与区间证明实现模块。
mod check_constraint;

/// 将约束检查类型与函数提升到 crate 根。
pub use check_constraint::*;

#[cfg(test)]
#[path = "check_constraint_aster_unit_test.rs"]
mod check_constraint_aster_unit_test;
