// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/precheck`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/precheck`).
//! 这里只负责把桩实现、核心类型与对齐测试重新导出。
//! 调用方通常只依赖 crate 根路径，不需要知道 `stubs.rs` 与
//! `precheck.rs` 的拆分细节。
//! 测试模块单独挂载，确保生产导出面与 Go 包接口保持一致。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 先暴露最小依赖边界，避免 `precheck.rs` 直接依赖真实外部包。
pub use stubs::*;

#[path = "precheck.rs"]
mod precheck;
// 核心类型与 trait 在这里定义，供 importinto 等上层流程复用。
pub use precheck::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
