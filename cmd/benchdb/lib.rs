// Copyright 2026 AsterSQL.

//! `benchdb` 的库入口只负责把二进制主流程拆成可复用模块，便于测试与 `[[bin]]`
//! 入口共享同一套装配关系。
//! 这里不承载基准逻辑本身，而是把真实执行放进 `entry`，把外部依赖桩
//! 集中在 `stubs`，从而让 parity test 能在不启动真实 TiKV 的前提下验证调用契约。
//! 这种分层保持了与 Go `main.go` 的单入口语义一致，同时把 Rust 包装层压缩到最薄。

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

/// `entry` 保留与 Go `main.go` 对齐的命令主流程；`lib.rs` 自己只做转发，
/// 避免把 CLI 入口和可测试逻辑耦合在同一个文件里。
#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// Shared process entrypoint called by the binary wrapper.
/// 这里显式调用 `entry::main()`，保证
/// 可执行入口与测试复用同一份实现，而不是复制一套独立启动逻辑。
pub fn main() {
    entry::main();
}
