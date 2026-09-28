// Copyright 2026 AsterSQL.

// parser/util 包入口：导出转义与 64 位哈希等解析期工具。
//
// 子模块对照 Go `pkg/parser/util`：`escape` 处理 MySQL 反斜杠字面量，
// `hash64` 声明规划器用增量哈希接口；测试模块仅在 `cfg(test)` 下编译。

#![allow(non_snake_case)]

/// MySQL 字符串反斜杠转义还原。
pub mod escape;
/// 规划器 64 位增量哈希接口。
pub mod hash64;

pub use escape::*;
pub use hash64::*;

/// UnescapeChar 等转义相关单元测试。
#[cfg(test)]
mod escape_test;
/// 迁移对照用 Aster 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
