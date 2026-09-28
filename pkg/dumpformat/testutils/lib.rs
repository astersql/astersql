// Copyright 2026 AsterSQL.

// dumpformat 测试工具 crate：导出 Parquet 测试写入辅助，并挂接迁移单元测试。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 基于 objstore + parquet-rs 的测试向 Parquet 写入辅助。
mod parquet_writer;
/// 再导出 parquet_writer 公开 API。
pub use parquet_writer::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
// 迁移对照单元测试（仅 cfg(test)）。
mod migration_aster_unit_test;
