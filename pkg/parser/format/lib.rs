// Copyright 2026 AsterSQL.
// `parser_format` crate 入口：导出 SQL 格式化与 AST 恢复相关 API，
// 并在测试配置下挂载单元测试与迁移对照测试模块。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 核心实现模块：格式化器、恢复标志与 `RestoreCtx`。
pub mod format;
/// 将 `format` 模块的公开项再导出到 crate 根，便于 `use parser_format::*`。
pub use format::*;

/// 与 Go `format_test.go` 对齐的单元测试。
#[cfg(test)]
#[path = "format_test.rs"]
mod format_test;

/// 迁移过程中补充的对照/边界测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
