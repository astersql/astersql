// Copyright 2026 AsterSQL.

// unistore config crate 入口。
//
// 导出 `config` 模块的公共类型与函数，并在测试时挂入迁移单元测试。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 配置定义与解析实现模块。
pub mod config;
pub use config::*;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// Aster 迁移后的配置相关单元测试。
mod migration_aster_unit_test;
