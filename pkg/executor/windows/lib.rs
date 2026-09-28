// Copyright 2026 AsterSQL.

// 窗口函数（Window Function）执行器模块。
//
// 实现 SQL `OVER` 子句：按分区（PARTITION BY）与排序（ORDER BY）在帧（frame）上
// 计算 ROW_NUMBER、SUM、AVG、LAG 等窗口聚合。含缓冲式与流水线两种执行路径。

#![allow(dead_code)]

/// 物理窗口计划到执行器的构建入口。
pub mod builder;
/// 流水线窗口执行器：边读边产出，降低内存占用。
pub mod pipelined_window;
/// 缓冲式窗口执行器与窗口函数、帧边界等核心类型。
pub mod window;

pub use builder::*;
pub use pipelined_window::*;
pub use window::Average;
pub use window::MaxValue;
pub use window::MinValue;
pub use window::*;
pub use window::{Decimal, DecimalAverage, DecimalSum};

#[cfg(test)]
mod window_executor_test;
#[cfg(test)]
mod window_sql_test;
#[cfg(test)]
mod window_test;
