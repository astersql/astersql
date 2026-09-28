// Copyright 2026 AsterSQL.

// DDL 索引 ingest（批量写入）测试工具库入口。
//
// 重新导出 `testutil` 子模块中的 mock 后端注入与泄漏检查辅助接口。

#![allow(non_snake_case)]

/// ingest 测试工具实现（mock 后端、泄漏检查等）。
pub mod testutil;

pub use testutil::*;

#[cfg(test)]
mod testutil_aster_unit_test;
