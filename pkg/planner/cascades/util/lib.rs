// Copyright 2026 AsterSQL.

// Cascades 工具包入口。
//
// 聚合级联优化器（Cascades）调试/描述用的字符串缓冲 writer，
// 并在测试配置下挂接迁移回归用例。

/// 带缓冲的字符串 writer（StrBuffer / StrBufferWriter）。
pub mod string_writer;
pub use string_writer::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期 StrBuffer 行为回归测试。
mod migration_aster_unit_test;
