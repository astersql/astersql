// Copyright 2026 AsterSQL.

// `pkg/util/format` crate 入口。
//
// 导出缩进/扁平 Formatter 与 `OutputFormat`；测试模块在 `cfg(test)` 下挂载。

/// 核心格式化实现（Indent/Flat/OutputFormat）。
pub mod format;

pub use format::*;

#[cfg(test)]
mod format_test;

#[cfg(test)]
mod main_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
