// Copyright 2026 AsterSQL.

// `util/compress` crate 入口：Gzip 对象池与编解码封装。
//
// 对应 Go `pkg/util/compress`。再导出 `gzip` 模块中的 Writer/Reader 池，
// 供需要复用压缩器的调用方使用。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// Gzip 压缩读写与对象池实现。
pub mod gzip;
pub use gzip::*;

/// AsterSQL 迁移补充的 compress 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
