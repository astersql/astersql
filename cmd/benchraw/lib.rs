// Copyright 2026 AsterSQL.

//! 该文件只充当 `benchraw` 的共享库入口，本身不承载压测实现细节。
//! 真正与 Go `cmd/benchraw/main.go` 对齐的参数解析、并发写入和输出格式
//! 都收敛在 `entry` 模块，便于把入口转发与行为对齐测试拆开维护。
//! `stubs` 模块则隔离 RawKV、日志与 HTTP 边界，保证这里的职责只剩导出与转发。

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

#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// 二进制包装层通过这里统一转发到 `entry::main()`。
/// 这样既保留 Cargo 期望的入口签名，也让真实逻辑能被测试模块直接复用。
/// Shared process entrypoint called by the binary wrapper.
pub fn main() {
    entry::main();
}
