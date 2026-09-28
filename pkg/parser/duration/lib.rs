// Copyright 2026 AsterSQL.

// parser/duration crate 入口。
//
// 导出时长解析 API，并挂接单元测试与迁移对照测试模块。

/// 时长解析实现。
pub mod duration;

pub use duration::*;

#[cfg(test)]
mod duration_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
