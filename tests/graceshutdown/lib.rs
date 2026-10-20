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

extern crate self as astersql_tests_graceshutdown;

const SIGINT: i32 = 2;

/// Exit status used by `tidb-server` after a graceful SIGINT shutdown.
pub const GRACEFUL_SIGINT_EXIT_CODE: i32 = 128 + SIGINT;

/// Apply the same `cmd.Wait` result handling as Go's `stopService`.
///
/// A successful wait is always graceful. A failed wait is graceful only when
/// the process reports `128 + SIGINT`; every other wait error is preserved.
pub fn classify_stop_wait(wait_error: Option<&str>, exit_code: i32) -> Result<(), String> {
    match wait_error {
        None => Ok(()),
        Some(_) if exit_code == GRACEFUL_SIGINT_EXIT_CODE => Ok(()),
        Some(error) => Err(error.to_owned()),
    }
}

#[cfg(test)]
#[path = "graceshutdown_test.rs"]
// 模块 `graceshutdown_test` 在这里被显式接线，方便按既定边界编译。
mod graceshutdown_test;

#[cfg(test)]
#[path = "main_test.rs"]
// 模块 `main_test` 在这里被显式接线，方便按既定边界编译。
mod main_test;
