// Copyright 2026 AsterSQL.

// 并发位图工具 crate 入口。
//
// 对应 Go `util/bitmap`：导出 `ConcurrentBitmap` 及相关构造/操作；
// 测试模块覆盖并发正确性、TestMain 配置与迁移期语义。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 并发安全位图实现。
pub mod concurrent;
pub use concurrent::*;

#[cfg(test)]
#[path = "concurrent_test.rs"]
/// 对应 Go 的并发置位 / 唯一 setter / Reset 测试。
mod concurrent_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：构造、跨段置位、Clone/Reset/内存估算。
mod migration_aster_unit_test;
