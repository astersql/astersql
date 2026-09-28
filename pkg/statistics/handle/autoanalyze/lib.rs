// Copyright 2026 AsterSQL.

// `autoanalyze` crate 入口。
//
// 对外导出自动统计信息收集（auto-analyze）核心逻辑：判断表是否需要 ANALYZE、
// 清理损坏作业、随机挑选表/分区，以及分析时间窗口解析等。

#![allow(dead_code)]

/// 自动 ANALYZE 主实现模块（对应 Go `autoanalyze` 包）。
pub mod autoanalyze;

pub use autoanalyze::*;

#[cfg(test)]
#[path = "autoanalyze_test.rs"]
/// 自动 ANALYZE 纯逻辑与算法相关单元测试。
mod autoanalyze_test;
