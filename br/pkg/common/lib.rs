// Copyright 2026 AsterSQL.

//! BR 公共常量包入口；再导出 `consts`。
//! 与 Go `br/pkg/common` 边界对齐，供各子系统共享并发上限等常量。
//! 测试经 path 挂载 parity；`pub use` 扁平化对外符号。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 常量定义模块：当前承载 MaxStoreConcurrency。
#[path = "consts.rs"]
pub mod consts;

// 扁平重导出，使调用方可用 `astersql_br_pkg_common::MaxStoreConcurrency`。
pub use consts::*;

#[cfg(test)]
#[path = "consts_test.rs"]
mod consts_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
