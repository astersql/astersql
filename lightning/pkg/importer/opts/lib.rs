// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/importer/opts`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/importer/opts`).
//!
//! 该入口把 importer 预检相关的选项拆成三层：
//! 最底层是 `stubs` 提供的 mydump option 占位类型，
//! 中间层是获取预检信息与构建 precheck 的具体选项定义，
//! crate 根则统一 re-export，方便上层像使用 Go 包一样直接引用。

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
// 先导出依赖边界占位类型，给后续 option 定义提供统一基础。
pub use stubs::*;

#[path = "get_pre_info_opts.rs"]
mod get_pre_info_opts;
// 这组 option 面向预检信息采集本身。
pub use get_pre_info_opts::*;

#[path = "precheck_opts.rs"]
mod precheck_opts;
// 这组 option 面向 precheck builder，把多类子选项打包给调用方。
pub use precheck_opts::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
