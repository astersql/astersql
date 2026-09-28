// Copyright 2026 AsterSQL.

// 规划器 issue 回归测试 crate 的库入口。
//
// 聚合与历史 planner issue、参数化 panic 风险相关的测试模块；
// 仅在 `cfg(test)` 下编译，用于迁移期隔离依赖边界并复现 AST / 规范化问题。

#![allow(dead_code)]

/// 测试入口与 fixture 批解析 harness。
#[cfg(test)]
mod main_test;
/// 零参日期函数等参数化路径的 panic 风险回归。
#[cfg(test)]
mod panicrisk_tier2_test;
/// 具体 planner issue SQL 的解析与 NormalizeDigest 覆盖。
#[cfg(test)]
mod planner_issue_test;
