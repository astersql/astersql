// Copyright 2026 AsterSQL.

//! Crate entry for `tools/check` (Go package main — ut driver).
//!
//! 这个库 crate 把 Go 版 `tools/check` 的 `main` 包拆成可复用的 Rust 模块，
//! 让 `bin_main.rs` 只负责进程入口，真正的单测发现、构建与执行流程都集中在库内。
//! 这里本身不承载业务逻辑，而是固定模块装配顺序和对外导出面，确保命令行入口、
//! parity 测试与未来复用方都能走到同一套 `ut::main()` 启动路径。

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

/// 长测清单与长测专用并发策略，供 `ut` 在 `--long` 模式下切换执行模型。
#[path = "longtests.rs"]
pub mod longtests;

/// 单测驱动主体，负责参数解析、测试二进制构建、执行调度与结果汇总。
#[path = "ut.rs"]
pub mod ut;

/// parity 测试只在测试编译目标中接入，避免把测试辅助代码暴露给生产入口。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// 与 Go `main` 对齐的统一入口，二进制壳层和测试都通过这里复用启动流程。
pub fn main() {
    ut::main();
}
