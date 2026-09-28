// Copyright 2026 AsterSQL.

// `testfork` crate 入口：导出组合测试驱动，并挂接相关单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Pick / RunTest 组合枚举实现模块。
pub mod fork;
/// 将 `fork` 模块公开 API 提升到 crate 根。
pub use fork::*;

/// 组合枚举行为测试（仅在 `cfg(test)` 下编译）。
#[cfg(test)]
#[path = "fork_test.rs"]
mod fork_test;

/// 迁移对照用单元测试（仅在 `cfg(test)` 下编译）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
