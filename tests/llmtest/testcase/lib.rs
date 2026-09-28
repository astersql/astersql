// Copyright 2026 AsterSQL.

//! Crate entry for `tests/llmtest/testcase`
//! (Go package `github.com/pingcap/tidb/tests/llmtest/testcase`).

// 本文件对应 `tests/llmtest/testcase/lib.rs`，本次任务只补中文解释，不改行为。
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

#[path = "testcase.rs"]
// 模块 `testcase` 在这里被显式接线，方便按既定边界编译。
mod testcase;

#[path = "run.rs"]
// 模块 `run` 在这里被显式接线，方便按既定边界编译。
mod run;

pub use stubs::{AnyValue, Db, DbRows, QueryOutcome, SqlError, json};
pub use testcase::{Case, Manager, open};

// Ensure run.rs `impl Manager` is linked.
#[allow(unused_imports)]
use run::*;

#[cfg(test)]
#[path = "parity_test.rs"]
// 模块 `parity_test` 在这里被显式接线，方便按既定边界编译。
mod parity_test;
