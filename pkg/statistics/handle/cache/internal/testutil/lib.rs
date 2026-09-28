// Copyright 2026 AsterSQL.

// 统计信息缓存测试工具库入口。
//
// 对外再导出 `testutil` 模块中的 mock 表构造辅助函数，
// 并在测试配置下挂载同目录的单元测试。

#![allow(non_snake_case)]

mod testutil;
pub use testutil::*;

#[cfg(test)]
#[path = "testutil_test.rs"]
mod testutil_test;
