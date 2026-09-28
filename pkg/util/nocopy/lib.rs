// Copyright 2026 AsterSQL.

// `util/nocopy` crate 入口：禁止拷贝的零大小标记类型。
//
// 对应 Go `pkg/util/nocopy`。通过嵌入 `NoCopy` 并实现空的 Locker 风格方法，
// 在 Go 中触发 `go vet -copylocks`；Rust 侧则刻意不实现 `Clone`/`Copy`。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// NoCopy 标记类型与 Locker 风格空方法。
pub mod nocopy;
/// 再导出 nocopy 模块公开 API。
pub use nocopy::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移回归：零大小、非 Clone/Copy、lock/unlock 可重复调用。
mod migration_aster_unit_test;
