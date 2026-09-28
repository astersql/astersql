// Copyright 2026 AsterSQL.

// 选择算法（selection）crate 入口。
//
// 导出 introselect / quickselect 相关 API，并在测试配置下挂载
// main_test、selection_test 与 migration_aster_unit_test。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 选择算法实现模块（introselect 等）。
pub mod selection;
/// 重新导出 selection 模块的公共 API。
pub use selection::*;

/// 选择算法功能与基准辅助测试。
#[cfg(test)]
#[path = "selection_test.rs"]
mod selection_test;

/// 迁移对照单元测试：与 Go 侧排名语义对齐。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
