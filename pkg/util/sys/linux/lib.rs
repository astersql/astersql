// Copyright 2026 AsterSQL.

// Linux / 非 Linux / Windows 系统工具 crate 入口：按目标 OS 选择实现子模块。
//
// 对应 Go `pkg/util/sys/linux` 等平台相关封装。Linux 用 `sys_linux`，
// Windows 用 `sys_windows`，其余类 Unix 用 `sys_other`；测试通过 `#[path]` 挂载。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Linux 平台系统信息与资源查询实现。
#[cfg(target_os = "linux")]
pub mod sys_linux;
/// 再导出 Linux 实现中的公共 API。
#[cfg(target_os = "linux")]
pub use sys_linux::*;

/// 非 Linux 且非 Windows 的类 Unix 平台实现。
#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
pub mod sys_other;
/// 再导出其它 Unix 平台实现中的公共 API。
#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
pub use sys_other::*;

/// Windows 平台系统信息实现。
#[cfg(target_os = "windows")]
pub mod sys_windows;
/// 再导出 Windows 实现中的公共 API。
#[cfg(target_os = "windows")]
pub use sys_windows::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 迁移对齐单测。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 系统工具单元测试。
#[cfg(test)]
#[path = "sys_test.rs"]
mod sys_test;
