// Copyright 2026 AsterSQL.

// executor 内部测试工具库入口。
//
// 聚合聚合算子（Agg）、Limit、Sort、通用 Mock 数据源与窗口函数（Window）等
// 测试用例构造模块，供执行器单元/性能测试复用。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 聚合（Aggregation）算子测试用例定义。
pub mod agg;
/// Limit 算子测试用例定义。
pub mod limit;
/// Sort 算子测试用例定义。
pub mod sort;
/// 通用 Mock 数据源、Chunk、会话变量等测试基础设施。
pub mod testutil;
/// 窗口函数（Window）算子测试用例定义。
pub mod window;

pub use agg::*;
pub use limit::*;
pub use sort::*;
pub use testutil::*;
pub use window::*;

#[cfg(test)]
#[path = "testutil_test.rs"]
mod testutil_test;
