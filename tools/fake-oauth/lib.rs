// Copyright 2026 AsterSQL.

//! Crate entry for `tools/fake-oauth` (Go package main — fake OAuth token server).
//! 该入口只负责把实现模块、对齐测试与对外入口组装在一起，
//! 便于调用方像 Go `main` 包一样从单一 crate 表面访问行为。
//! 真实的路由注册、固定 JSON 响应与监听流程都落在 `main.rs`，
//! 这里保持最薄的一层转发，避免把可执行语义分散到多个入口文件。

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

#[path = "main.rs"]
pub mod main;

/// parity 测试单独挂载在库入口侧，专门校验导出面与 Go `main.go` 的行为一致。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// Binary / library process entry matching Go `main`.
/// 对外统一暴露 `entry()`，使二进制包装或测试夹具都复用同一条启动路径。
pub fn entry() {
    main::main();
}
