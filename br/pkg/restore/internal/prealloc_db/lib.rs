// Copyright 2026 AsterSQL.

//! BR 还原侧 PreallocDB 包入口：挂载 `db` 实现并扁平再导出。
//! 与 Go `br/pkg/restore/internal/prealloc_db` 对应——在还原前通过
//! session/glue 预创建库表与 placement policy，并配合 table ID 预分配。
//! 测试用 `#[path]` 挂载 parity/单元测试，避免与实现文件同目录混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 显式 path 固定模块文件，便于与 Go 包布局一一对照。
#[path = "db.rs"]
pub mod db;

// 对外扁平导出 DB/NewDB 等符号，调用方不必写 prealloc_db::db::。
pub use db::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;
