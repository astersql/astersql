// Copyright 2026 AsterSQL.

//! Crate entry for `dumpling/log` (Go package `github.com/pingcap/tidb/dumpling/log`).
//! dumpling/log 模块入口：对应 Go 包 `github.com/pingcap/tidb/dumpling/log`，
//! 聚合日志实现与测试子模块，对外 re-export 全部公开符号。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 核心日志实现，对应 Go `log.go`。
#[path = "log.rs"]
mod log;

// 与 Go 包级导出一致，调用方可直接 `use dumpling_log::*`。
pub use log::*;

// 权限/路径边界单测，对应 Go `log_test.go` TestInitLogNoPermission。
#[cfg(test)]
#[path = "log_test.rs"]
mod log_test;

// Go/Rust 公开契约 parity 测试，覆盖 InitAppLogger、Zap 等对外 API。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
