// Copyright 2026 AsterSQL.

// 导入 SDK 的测试用 mock 子 crate 入口。
//
// 提供可预期调用队列的 `FileScanner`/`JobManager`/`SQLGenerator`/`SDK` 替身，
// 供上层单元测试在不连接真实数据库或对象存储时验证导入流程编排。

#![allow(dead_code, non_snake_case)]

/// 基于期望队列的 SDK/组件 mock 实现。
pub mod sdk_mock;

pub use sdk_mock::*;

#[cfg(test)]
mod sdk_mock_test;
