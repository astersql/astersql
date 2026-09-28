// Copyright 2026 AsterSQL.

// vitess crate 根：Vitess 风格分片键哈希工具。
//
// 再导出 `vitess_hash`；测试下挂载 TestMain、迁移回归与 Go 对照哈希单测。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Vitess null-key DES 分片哈希实现。
pub mod vitess_hash;
pub use vitess_hash::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// AsterSQL 迁移回归：数值向量与确定性。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// Go 对照：大端十六进制哈希样例。
#[cfg(test)]
#[path = "vitess_hash_test.rs"]
mod vitess_hash_test;
