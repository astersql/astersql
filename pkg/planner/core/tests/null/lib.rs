// Copyright 2026 AsterSQL.

// planner/core/tests/null 测试 crate 入口。
//
// 覆盖 ISNULL / `<=>` NULL 等空值谓词下的规划与语法相关用例。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// ISNULL / null-safe 等值等空值相关规划测试。
#[cfg(test)]
#[path = "null_test.rs"]
mod null_test;
