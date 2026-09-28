// Copyright 2026 AsterSQL.

// `util/prefetch` crate 入口：带预取缓冲的异步 Reader 封装。
//
// 对应 Go `pkg/util/prefetch`。在后台填充缓冲，降低顺序读大对象时的等待。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 预取 Reader 实现（缓冲池与后台填充）。
pub mod reader;
/// 再导出 NewReader / ReadCloser 等公共 API。
pub use reader::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充的 prefetch 单元测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "reader_test.rs"]
/// 对应 Go reader_test 的预取 Reader 测试。
mod reader_test;
