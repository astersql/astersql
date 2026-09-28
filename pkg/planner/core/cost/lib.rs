// Copyright 2026 AsterSQL.

// 优化器代价（cost）crate 入口。
//
// 再导出 AST 聚合函数名，并暴露 `factors_thresholds` 中的代价因子与阈值，
// 供规划器在比较物理计划成本时统一读取。

extern crate self as astersql_planner_core_cost;

pub use parser_ast::functions as ast;
pub mod factors_thresholds;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
