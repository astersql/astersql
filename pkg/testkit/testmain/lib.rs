// Copyright 2026 AsterSQL.

// testmain：测试入口包装与基准短路。
//
// 再导出 `TestingM` / `WrapTestingM` 以及 `ShortCircuitForBench`，
// 用于在 Rust 侧提供测试退出码回调与 bench 标志行为。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 基准测试 `-test.bench` 解析与短路退出。
pub mod bench;
/// `TestingM` 接口与退出码回调包装。
pub mod wrapper;
pub use bench::*;
pub use wrapper::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：包装回调与 bench 标志解析。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

#[cfg(test)]
#[path = "wrapper_test.rs"]
mod wrapper_test;
