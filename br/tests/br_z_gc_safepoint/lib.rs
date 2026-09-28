// Copyright 2026 AsterSQL.

//! GC safepoint 集成测试包入口：导出 `gc` 用例与 stubs。
//! 验证备份/恢复路径与 PD GC safepoint 交互；非生产路径。
//! `main` 转发给 gc::main，供测试二进制包装器调用。
//! stubs 提供测试替身，parity_test 对照 Go 契约。
//! 实现细节见 `gc` 模块。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "gc.rs"]
pub mod gc;

pub use gc::*;
pub use stubs::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "gc_test.rs"]
mod gc_test;

/// Shared process entrypoint called by the binary wrapper.
/// 测试二进制入口，委托 gc::main。
pub fn main() {
    gc::main();
}
