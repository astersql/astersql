// Copyright 2026 AsterSQL.

// 存储目录可用容量查询的 crate 入口。
//
// 按目标 OS 条件编译并再导出平台实现：POSIX（linux/macos）走 `statfs`，
// Windows 走专用实现，其余平台使用返回 `i64::MAX` 的兜底；测试模块一并挂载于此。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 非 linux/windows/macos 平台的容量查询兜底模块。
#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "windows"),
    not(target_os = "macos")
))]
pub mod sys_other;
#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "windows"),
    not(target_os = "macos")
))]
pub use sys_other::*;

/// POSIX（linux/macos）平台通过 `statfs` 查询目录可用容量。
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod sys_posix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use sys_posix::*;

/// Windows 平台的目录容量查询实现。
#[cfg(target_os = "windows")]
pub mod sys_windows;
#[cfg(target_os = "windows")]
pub use sys_windows::*;

/// 包级 TestMain 风格公共测试配置。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 迁移后的容量 API 跨平台单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 与 Go `sys_test` 对应的容量查询冒烟测试。
#[cfg(test)]
#[path = "sys_test.rs"]
mod sys_test;
