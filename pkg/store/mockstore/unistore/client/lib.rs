// Copyright 2026 AsterSQL.

// UniStore client crate 入口：导出 Client trait，并挂载迁移单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Client trait 定义模块。
pub mod client;
/// 再导出 client 公共项。
pub use client::*;

/// 迁移相关单元测试（仅在 test 配置下编译）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
