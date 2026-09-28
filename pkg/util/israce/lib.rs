// Copyright 2026 AsterSQL.

// `israce` crate：运行时查询是否启用竞态（race）检测。
//
// 对齐 Go `pkg/util/israce`：按构建条件导出常量 `RaceEnabled`。
// Rust 侧用 Cargo feature `race` 模拟 Go 的 `race` / `!race` build tag。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// race 构建变体：`RaceEnabled = true`。
pub mod israce;
/// 非 race 构建变体：`RaceEnabled = false`。
pub mod norace;
/// 启用 `race` feature 时导出 race 变体符号。
#[cfg(feature = "race")]
pub use israce::*;
/// 未启用 `race` feature 时导出 norace 变体符号。
#[cfg(not(feature = "race"))]
pub use norace::*;

/// 迁移对照单测：断言 `RaceEnabled` 与 feature 一致。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
