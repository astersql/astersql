// Copyright 2026 AsterSQL.

//! Crate entry for `tools/check/xprog` (Go package main — test binary relocator).
//! 该模块只负责组装 `stubs` 与 `xprog`，保持与 Go `main` 包一致的入口形态。
//! `stubs` 提供 Go 标准库路径语义所需的兼容层，`xprog` 承载真正的搬运逻辑。
//! 测试模块直接挂在 crate 根部，便于以与生产入口相同的可见性校验 Go/Rust 对齐结果。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "xprog.rs"]
pub mod xprog;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "xprog_test.rs"]
mod xprog_test;

/// Binary / library process entry matching Go `main`.
/// 对外只暴露这一层薄包装，避免调用方依赖内部模块布局，同时把实际退出码处理继续委托给 `xprog::main()`。
pub fn main() {
    xprog::main();
}
