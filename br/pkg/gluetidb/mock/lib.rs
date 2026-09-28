// Copyright 2026 AsterSQL.

//! BR TiDB Glue 的测试替身 crate 入口：挂载 `mock` 实现并扁平导出。
//! 对应 Go `br/pkg/gluetidb/mock`——用轻量 Session/Storage 桩替代真实 TiDB，
//! 供 backup/restore 单测注入；parity 测试以 `#[path]` 挂载，避免与实现混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 显式 path 固定 mock 实现文件，便于与 Go 包布局对照。
#[path = "mock.rs"]
mod mock;

// 对外扁平导出 MockGlue / MockSession 等符号。
pub use mock::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
