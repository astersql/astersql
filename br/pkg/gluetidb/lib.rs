// Copyright 2026 AsterSQL.

//! BR 的 TiDB Glue 包入口：挂载 glue / infoschema_filter 实现并扁平再导出。
//! 与 Go `br/pkg/gluetidb` 对应——在 TiDB session/domain 之上实现 `glue.Glue`，
//! 供 backup/restore 走 SQL 与 DDL 路径；测试用 `#[path]` 挂载 parity/glue
//! 及 test_support，避免与实现文件同目录混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 显式 path 固定模块文件，便于与 Go 包布局一一对照。
#[path = "infoschema_filter.rs"]
pub mod infoschema_filter;

#[path = "glue.rs"]
pub mod glue;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "glue_test.rs"]
mod glue_test;

#[cfg(test)]
#[path = "test_support_test.rs"]
mod test_support;

// 对外扁平导出公开符号，调用方不必写 gluetidb::glue:: 前缀。
pub use glue::*;
pub use infoschema_filter::*;
