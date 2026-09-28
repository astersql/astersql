// Copyright 2026 AsterSQL.

// 退避（backoff）工具 crate 入口。
//
// 导出指数退避类型与 `Backoffer` trait；测试配置下挂载 Go 对应测试
// 与迁移回归用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 指数退避实现模块。
pub mod backoff;
pub use backoff::*;

#[cfg(test)]
#[path = "backoff_test.rs"]
/// 对应 Go `backoff_test.go` 的单元测试。
mod backoff_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;
