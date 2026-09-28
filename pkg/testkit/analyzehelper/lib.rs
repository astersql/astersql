// Copyright 2026 AsterSQL.

// analyzehelper crate：ANALYZE 测试辅助的门面入口。
//
// 暴露 `helper` 模块中的运行时适配与谓词列收集触发 API。

#![allow(non_snake_case)]

/// 辅助实现（错误类型、运行时 trait、触发函数）。
pub mod helper;
pub use helper::*;

#[cfg(test)]
mod helper_aster_unit_test;
