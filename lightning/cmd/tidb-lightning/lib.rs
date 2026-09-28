// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/cmd/tidb-lightning` (Go package main).
//! 这个 crate 只负责把 Go `main` 包装成可复用的 Rust 模块边界，不承载具体业务逻辑。
//! `stubs` 和 `fips` 先补齐入口侧依赖，再由 `entry` 承接真实启动流程。
//! 对外公开的 `main()` 仅做一次转发，保持与 Go 版本“入口函数负责组装、实现落在同目录模块”的结构一致。
//! 测试模块按原入口文件挂载，方便与 Go 邻近测试保持名称和职责上的一一对应。

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
// 迁移尚未完全落地的入口依赖先集中在这里，避免污染主流程模块。
pub mod stubs;

#[path = "fips.rs"]
// FIPS 相关副作用通过独立模块接入，保持入口文件只负责装配依赖。
pub mod fips;

#[path = "main.rs"]
// 真实启动逻辑放在与 Go `main.go` 对齐的实现文件中，库入口只做重导出。
pub mod entry;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// Binary / library process entry matching Go `main`.
/// 统一暴露给二进制和测试调用，确保两侧都走同一条启动路径。
pub fn main() {
    entry::main();
}
