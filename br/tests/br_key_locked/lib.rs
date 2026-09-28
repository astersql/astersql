// Copyright 2026 AsterSQL.

//! key locked 集成测试包：导出 codec/locker 与 stubs。
//! 验证备份在遇到锁时的行为；非生产路径。
//! `main` 委托 locker::main，供测试二进制包装器调用。
//! codec 负责键编码；locker 驱动加锁场景。
//! parity_test 对照 Go 公开契约。

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

#[path = "codec.rs"]
pub mod codec;

#[path = "locker.rs"]
pub mod locker;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "locker_test.rs"]
mod locker_test;

pub use codec::*;
pub use locker::*;
pub use stubs::*;

/// Shared process entrypoint called by the binary wrapper.
/// 测试二进制入口，转交 locker 场景主流程。
pub fn main() {
    locker::main();
}
