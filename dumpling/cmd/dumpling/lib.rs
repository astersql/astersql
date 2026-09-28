// Copyright 2026 AsterSQL.

//! Crate entry for `dumpling/cmd/dumpling` (Go package main).
//!
//! 这个 crate 把 Go 单文件 `main.go` 拆成入口、参数解析和 stub 三层，
//! 方便在 Rust 侧分别复用、测试和替换底层依赖。
//! 对外仍然只暴露一个 `main()`，保持与 Go 二进制入口一致。

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
// pflag 与 prometheus 的 arm64-safe 替身集中放在这里，供入口和测试共享。
pub mod stubs;

#[path = "config_flags.rs"]
// 参数定义与解析留在单独模块，避免主流程被大段 flag 细节淹没。
pub mod config_flags;

#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "config_flags_test.rs"]
mod config_flags_test;

/// Binary / library process entry matching Go `main`.
pub fn main() {
    // 统一经由 entry::main 进入，确保 bin 和测试看到的是同一套控制流。
    entry::main();
}
