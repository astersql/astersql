// Copyright 2026 AsterSQL.

// `testflag` crate 入口：导出 `-long` 标志解析，并挂接迁移单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// `-long` 标志解析实现模块。
pub mod flag;
/// 将 `flag` 模块公开 API 提升到 crate 根。
pub use flag::*;

/// 迁移对照用单元测试（仅在 `cfg(test)` 下编译）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod flag_test;
