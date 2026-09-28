// Copyright 2026 AsterSQL.

// `testfailpoint` crate 入口：导出 failpoint 测试辅助，并挂接迁移单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// failpoint 启停、求值与暂停型注入的实现模块。
pub mod failpoint;
/// 将 `failpoint` 模块公开 API 提升到 crate 根。
pub use failpoint::*;

/// 迁移对照用单元测试（仅在 `cfg(test)` 下编译）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
