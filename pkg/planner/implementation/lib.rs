// Copyright 2026 AsterSQL.

// Cascades Memo 物理 Implementation 包入口。
//
// 将逻辑算子的物理候选封装为带代价估算的 Implementation，供 Memo
// 在物理优化阶段择优。子模块按算子类别划分：
// - `base`：公共代价基座与 CostPlan trait；
// - `datasource`：表/索引扫描与 Reader；
// - `join`：HashJoin / MergeJoin；
// - `simple_plans`：Projection、Selection、Agg、TopN、Apply 等；
// - `sort`：Sort / NominalSort。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 公共代价基座、CostPlan trait 与 Implementation 宏。
mod base;
pub use base::*;
/// 数据源类 Implementation（Table/Index Scan & Reader）。
mod datasource;
pub use datasource::*;
/// 二元 Join Implementation。
mod join;
pub use join::*;
/// 投影、过滤、聚合、TopN、Apply、Union 等简单计划。
mod simple_plans;
pub use simple_plans::*;
/// Sort / NominalSort Implementation。
mod sort;
pub use sort::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "base_test.rs"]
mod base_test;
