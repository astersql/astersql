// Copyright 2026 AsterSQL.

//! Crate entry for `tests/graceshutdown`
//! (Go package `github.com/pingcap/tidb/tests/graceshutdown`).
//!
//! Test-only package: integration targets are wired via `Cargo.toml` `[[test]]`
//! and also reachable as lib unit-test modules below.

// 本文件对应 `tests/graceshutdown/lib.rs`，本次任务只补中文解释，不改行为。
// 本文件主要负责模块接线和导出关系说明。
// 阅读时关注哪些模块只在测试条件下启用。
// 中文注释只帮助快速判断依赖方向。
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

#[cfg(test)]
#[path = "graceshutdown_test.rs"]
// 模块 `graceshutdown_test` 在这里被显式接线，方便按既定边界编译。
mod graceshutdown_test;

#[cfg(test)]
#[path = "main_test.rs"]
// 模块 `main_test` 在这里被显式接线，方便按既定边界编译。
mod main_test;
