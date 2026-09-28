// Copyright 2026 AsterSQL.

//! Crate entry for `tests/llmtest/logger`
//! (Go package `github.com/pingcap/tidb/tests/llmtest/logger`).

// 本文件对应 `tests/llmtest/logger/lib.rs`，本次任务只补中文解释，不改行为。
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

#[path = "stubs.rs"]
// 模块 `stubs` 在这里被显式接线，方便按既定边界编译。
pub mod stubs;

#[path = "log.rs"]
// 模块 `log` 在这里被显式接线，方便按既定边界编译。
mod log;

pub use log::{Global, ensure_init};
pub use stubs::zap;

#[cfg(test)]
#[path = "parity_test.rs"]
// 模块 `parity_test` 在这里被显式接线，方便按既定边界编译。
mod parity_test;
