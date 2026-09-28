// Copyright 2026 AsterSQL.

// testsetup：测试套件公共初始化入口。
//
// 再导出 `SetupForCommonTest` / `apply_os_log_level` 等桥接符号，
// 供各包 TestMain 在跑测前配置日志等全局状态。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 日志级别应用与 SetupForCommonTest 实现。
pub mod bridge;
pub use bridge::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：环境变量日志级别的 noop / 配置 / 非法值。
mod migration_aster_unit_test;
