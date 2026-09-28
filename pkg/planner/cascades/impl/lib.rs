// Copyright 2026 AsterSQL.

// Cascades 物理实现（implementation）与代价（cost）子模块入口。
//
// 聚合 `find_best_task_router`（为逻辑算子路由最优物理任务）与
// `impl_and_cost`（按物理属性枚举并比较代价），并挂载相关单元测试。

#![allow(dead_code)]

mod find_best_task_router;
mod impl_and_cost;

pub use find_best_task_router::*;
pub use impl_and_cost::*;

#[cfg(test)]
mod find_best_task_router_aster_unit_test;
#[cfg(test)]
mod find_best_task_router_test;
