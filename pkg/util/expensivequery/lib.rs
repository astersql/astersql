// Copyright 2026 AsterSQL.

// 昂贵查询（expensive query）监控 crate 入口。
//
// 导出 `expensivequery` 模块中的句柄、阈值与会话巡检能力；测试通过
// `expensivequery_test` 挂载。

#![allow(dead_code, non_snake_case)]

/// 昂贵查询监控实现模块。
pub mod expensivequery;

pub use expensivequery::*;

#[cfg(test)]
#[path = "expensivequery_test.rs"]
mod expensivequery_test;
