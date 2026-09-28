// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/cmd/tidb-lightning-ctl` (Go package main).
//! 中文补充：这个 crate 把命令行入口、FIPS 钩子和测试桩组织成与 Go `main` 包一致的装配层。

#![allow(
    // 中文补充：该目录仍以 Go 对齐和机械迁移为主，先放宽命名与未使用约束，避免掩盖真正的语义差异。
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
// 中文补充：对外重导出桩模块，方便入口和 parity test 共享同一组兼容层符号。
pub use stubs::*;

#[path = "fips.rs"]
pub mod fips;
// 中文补充：显式暴露 FIPS 初始化钩子，让入口按 Go 的启动顺序先触发相关约束。

#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// Binary / library process entry matching Go `main`.
/// 中文补充：这里仅做一层转发，保证二进制入口与库模式复用同一套主流程。
pub fn main() {
    entry::main();
}
