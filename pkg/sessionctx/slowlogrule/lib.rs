// Copyright 2026 AsterSQL.

// 慢日志规则（slowlogrule）子 crate 入口。
//
// 导出慢查询日志匹配规则（见 `rules` 子模块），用于按字段条件过滤慢日志条目；
// 测试模块仅在 `cfg(test)` 下编译。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 慢日志规则定义与匹配逻辑。
pub mod rules;
pub use rules::*;

/// 迁移期单元测试（与 Go 行为对照）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
