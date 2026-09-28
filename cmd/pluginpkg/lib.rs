// Copyright 2026 AsterSQL.
//! `cmd/pluginpkg` 的 Rust 库入口。
//!
//! 这里保持和 Go 版 `cmd/pluginpkg` 相同的分层：`stubs` 提供命令行、
//! 文件系统与进程边界的替身，`pluginpkg` 承载实际打包流程；本文件只负责
//! 组装模块并为二进制包装层提供共享入口，避免入口层掺入业务逻辑。

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

/// 打包器主逻辑模块，对齐 Go `pluginpkg.go` 的命令执行流程。
#[path = "pluginpkg.rs"]
pub mod pluginpkg;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "pluginpkg_test.rs"]
mod pluginpkg_test;

/// 共享进程入口，由二进制包装层调用并转发到 `pluginpkg::main()`。
pub fn main() {
    pluginpkg::main();
}
