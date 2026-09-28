// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 该文件只负责把 `tidb-server` 需要的子模块组装成一个可测试的库入口。
// Go 版本直接以 `main.go` 作为二进制入口；Rust 这里额外提供 `lib.rs`，
// 这样集成测试可以复用同一套初始化和入口转发逻辑，而不必启动独立进程。
// Allow in-lib `#[cfg(test)]` modules to share the crate root as
// `astersql_cmd_tidb_server::…` when needed by integration harnesses.
extern crate self as astersql_cmd_tidb_server;

// 按 Go 的职责拆分路径模块：桩实现、FIPS 相关逻辑，以及实际的入口实现。
// `main()` 本身只做一次薄转发，避免库入口和二进制入口在测试中产生分叉。
#[path = "stubs.rs"]
pub mod stubs;

#[path = "fips.rs"]
pub mod fips;

#[path = "main.rs"]
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 二进制入口保持极薄，只把控制权交给 `entry::main()`，
/// 以保证测试、库调用和最终可执行文件走到同一条启动路径。
/// Shared process entrypoint called by the binary wrapper.
pub fn main() {
    fips::enable_fips_only();
    entry::main();
}
