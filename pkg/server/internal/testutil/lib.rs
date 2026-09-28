// Copyright 2026 AsterSQL.

// server/internal/testutil crate 根模块：服务端测试替身工具。
//
// 再导出内存连接 `BytesConn` 与 TCP 端口提取等辅助，供协议层单测
// 在不真实建连的情况下模拟读写。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 测试工具实现（BytesConn 等）。
pub mod testutil;
pub use testutil::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
