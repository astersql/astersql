// Copyright 2026 AsterSQL.

//! BR 集成测试公共库入口：导出 `utils` 与桩模块。
//! 供各 `br_tests/*` 子包复用进程入口与测试辅助；非生产路径。
//! `main` 转发给 utils，便于二进制包装器统一拉起。
//! stubs 提供测试替身，避免测试依赖真实集群时强耦合。

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

#[path = "utils.rs"]
pub mod utils;

pub use utils::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// Shared process entrypoint called by the binary wrapper.
/// 二进制包装器共享入口，实际逻辑在 utils::main。
pub fn main() {
    utils::main();
}
