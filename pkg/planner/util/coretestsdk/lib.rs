// Copyright 2026 AsterSQL.

// 规划器 core 测试 SDK crate 根。
//
// 汇总 mock 与 testkit 子模块，为 core/util 相关单元测试提供公共脚手架。

#![allow(dead_code)]

/// Mock 辅助（假数据源、假上下文等）。
pub mod mock;
/// 测试工具包（断言、建表/建计划辅助等）。
pub mod testkit;

#[cfg(test)]
mod coretestsdk_aster_unit_test;
