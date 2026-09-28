// Copyright 2026 AsterSQL.

//! BR utils 列族常量 crate 入口；再导出 `consts` 供备份/恢复统一引用。
//! 与 Go `br/pkg/utils/consts` 包边界对齐，测试经 path 挂载。
//! 本文件无业务逻辑，仅固定模块布局与对外符号面。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "consts.rs"]
pub mod consts;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

pub use consts::*;
