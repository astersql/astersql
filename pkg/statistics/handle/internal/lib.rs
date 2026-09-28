// Copyright 2026 AsterSQL.

// 统计句柄内部测试辅助 crate 根。
//
// 导出 `testutil` 中的表级统计断言工具，供其它 statistics handle 测试复用。

#![allow(non_snake_case)]

mod testutil;
pub use testutil::*;

#[cfg(test)]
#[path = "testutil_test.rs"]
mod testutil_test;
