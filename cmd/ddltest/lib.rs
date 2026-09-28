// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

//! `ddltest` crate 根模块只负责拼装测试桩与各个用例模块，不承载独立业务逻辑。
//! 这里维持与 Go `cmd/ddltest` 包接近的入口形态，让测试仍按包级符号组织。
//! 对外公开的能力都来自 `stubs.rs`，本文件主要定义导出边界和测试文件挂载关系。

// Allow integration `[[test]]` targets and in-lib `#[cfg(test)]` modules to
// share the same source via `astersql_cmd_ddltest::…`.
// 中文别名说明：测试文件通过 crate 自别名统一引用公共桩，避免在不同编译入口下改导入路径。
extern crate self as astersql_cmd_ddltest;

// 真实的 DDL 测试桩、内存执行器和辅助断言都集中在该模块中。
#[path = "stubs.rs"]
pub mod stubs;

// 沿用 Go 包级导出习惯，把桩中的公开符号直接提升到 crate 根，减少测试迁移改动。
pub use stubs::*;

// 这些 `#[cfg(test)]` 模块按 Go 原始测试文件拆分挂载，便于逐文件对照行为与注释语义。
#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;

#[cfg(test)]
#[path = "ddl_test.rs"]
mod ddl_test;

#[cfg(test)]
#[path = "index_test.rs"]
mod index_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "random_test.rs"]
mod random_test;
