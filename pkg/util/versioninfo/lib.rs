// Copyright 2026 AsterSQL.

// versioninfo crate 根：构建期版本/分支/发行版元数据。
//
// 再导出 `versioninfo` 模块中的包级可变字符串；测试下挂载迁移回归单测。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 版本信息常量与可覆盖的包级静态量。
pub mod versioninfo;
pub use versioninfo::*;

/// AsterSQL 迁移回归：默认值与可覆盖语义。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
