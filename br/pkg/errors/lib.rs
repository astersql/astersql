// Copyright 2026 AsterSQL.
//! `astersql_br_pkg_errors` crate 入口：导出 BR RFC 错误码与判定辅助。
//!
//! 实现集中在 `errors.rs`；测试模块按 path 挂载 `parity_test` / `errors_test`，
//! 与 Go `package errors` / `errors_test` 分工一致。`allow` 保留 Go 风格命名。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

#[path = "errors.rs"]
mod errors;
pub use errors::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;
