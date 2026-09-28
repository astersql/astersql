// Copyright 2026 AsterSQL.
#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

// Hash Join 版本选择与平台能力探测的 crate 根模块。
//
// 重新导出 `join_version` 中的常量与函数，供执行器按会话变量在 Hash Join v1（legacy）
// 与 v2（optimized）之间切换；测试模块挂接迁移单元测试。

pub mod join_version;
pub use join_version::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod join_version_test;
