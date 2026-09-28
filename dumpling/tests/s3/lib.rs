// Copyright 2026 AsterSQL.

// dumpling/tests/s3 crate 入口：聚合 stubs、import 与 parity 测试子模块。
// 对应 Go 侧 import.go 包及集成测试目录布局；bin 经 main() 委托 entry::main()。

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

// MySQL/context/errgroup/cobra 边界桩，arm64 安全、无重依赖。
#[path = "stubs.rs"]
pub mod stubs;

// 数据导入核心逻辑，对应 Go import.go。
#[path = "import.rs"]
pub mod entry;

// Go/Rust 公开契约 parity 测试。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// Shared process entrypoint called by the binary wrapper.
// 二进制 wrapper 调用的统一入口，转发至 entry::main()。
pub fn main() {
    entry::main();
}
