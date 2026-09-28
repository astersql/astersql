// Copyright 2026 AsterSQL.

// teststore crate 入口：再导出真实 mockstore 包装器，供独立测试使用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
/// Go teststore 包的 mockstore 构造包装模块。
mod store;
/// 对外再导出 store 中的公共 API。
pub use store::*;

#[cfg(test)]
#[path = "store_aster_unit_test.rs"]
mod store_aster_unit_test;
