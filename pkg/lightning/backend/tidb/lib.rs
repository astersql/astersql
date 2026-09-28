// Copyright 2026 AsterSQL.

// Lightning TiDB 逻辑导入后端子 crate 入口。
//
// 再导出 `tidb` 模块：通过 SQL（INSERT/REPLACE）将行写入目标 TiDB，
// 而非直接写 TiKV 键值；适用于逻辑导入路径。

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

mod tidb;
pub use tidb::*;

#[cfg(test)]
#[path = "tidb_test.rs"]
mod tidb_test;
