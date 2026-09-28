// Copyright 2026 AsterSQL.

// 规划器核心用法工具：聚合 CAST 包装与相关列抽取。
//
// 相关列（correlated column）指外层查询列被内层子查询引用的列；
// 抽取后用于 Apply/子查询解相关等改写。本 crate 对应 Go 的 `coreusage` 包。

#![allow(non_snake_case)]

/// 聚合函数参数的 CAST 包装逻辑。
mod cast_misc;
/// 逻辑/物理计划上相关列的递归抽取与按 Schema 去重。
mod correlated_misc;

pub use cast_misc::*;
pub use correlated_misc::*;

#[cfg(test)]
#[path = "coreusage_aster_unit_test.rs"]
mod coreusage_aster_unit_test;
