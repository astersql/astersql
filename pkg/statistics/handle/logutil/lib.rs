// Copyright 2026 AsterSQL.

// 统计子系统日志工具 crate 入口。
//
// 再导出通用日志设施，并挂载带 `stats` 分类的统计专用 Logger 构造函数。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_statistics_handle_logutil;

/// 底层日志类型与函数的再导出（来自 `astersql_util_logutil`）。
pub mod log {
    pub use astersql_util_logutil::log::*;
}

#[path = "logutil.rs"]
/// 统计类别 Logger、采样 Logger 等实现。
pub mod stats_logutil;
pub use stats_logutil::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：校验 category / sampled 字段与采样行为。
mod migration_aster_unit_test;
