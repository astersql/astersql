// Copyright 2026 AsterSQL.

//! Crate entry for `tools/patch-go` (Go package main — patched-Go runtime check).
//! 该文件是 `tools/patch-go` 的库侧总入口：把探针所需的子模块组织在一起，
//! 并向二进制入口或测试暴露与 Go `main` 一致的最小调用路径。
//! `check` 承担真正的运行时探测逻辑，`stubs` 提供本地替身符号，
//! 这里本身不增加业务分支，避免入口层偏离 Go 版“只触发探测”的语义。

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

/// 本地桩模块，对应 Go 版通过 `linkname` 访问的运行时符号占位。
pub mod stubs;

#[path = "check.rs"]
/// 运行时补丁探针的核心实现，入口最终会转发到这里。
pub mod check;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// Binary / library process entry matching Go `main`.
/// 保持单层转发有助于让库模式、二进制模式和测试模式共享同一条调用链，
/// 同时把“实际探测发生在 `check::main()`”这一职责边界固定下来。
pub fn entry() {
    check::main();
}
