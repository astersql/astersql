// Copyright 2026 AsterSQL.

//! benchkv 的库入口只负责把可执行入口、测试和本地桩模块组织在一起，
//! 让二进制包装层和单测复用同一套启动流程。
//! 这里不承载压测逻辑本身；真正与 Go `main.go` 对齐的参数解析、初始化和批量写入流程都在 `entry` 中。
//! `stubs` 提供可替换的 TiKV/HTTP/指标依赖，`parity_test` 则直接通过本文件暴露的入口校验 Rust 与 Go 的行为一致性。

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

/// 公开实际入口实现，二进制入口和对齐测试都通过该模块共享主流程。
#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// 二进制真正执行时只做一层转发，保持 crate 根入口稳定，同时复用 `entry::main()` 的 Go 对齐实现。
/// Shared process entrypoint called by the binary wrapper.
pub fn main() {
    entry::main();
}
