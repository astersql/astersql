// Copyright 2026 AsterSQL.

// DDL job 参数版本（V1 / 默认）测试开关的 crate 入口。
//
// 按编译 feature `ddlargsv1` 在 [`force_v1`] 与 [`normal`] 之间二选一 re-export：
// - 启用 `ddlargsv1`：强制测试走 V1 参数路径；
// - 未启用：使用默认（非强制 V1）路径。
//
// 另包含参数格式迁移相关的单元测试模块。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 强制 V1 参数路径的常量定义（仅在启用 feature 时被 re-export）。
pub mod force_v1;
/// 默认（非强制 V1）参数路径的常量定义。
pub mod normal;
/// 启用 `ddlargsv1` 时导出强制 V1 开关。
#[cfg(feature = "ddlargsv1")]
pub use force_v1::*;
/// 未启用 `ddlargsv1` 时导出默认路径开关。
#[cfg(not(feature = "ddlargsv1"))]
pub use normal::*;

/// 参数版本迁移相关的单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
