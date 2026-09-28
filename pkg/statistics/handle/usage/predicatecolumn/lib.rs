// Copyright 2026 AsterSQL.

// 谓词列（predicatecolumn）crate 入口。
//
// 导出基于受限 SQL 读写 `mysql.column_stats_usage` 的加载、清理与保存 API。

#![allow(non_snake_case)]

mod predicate_column;
pub use predicate_column::*;

#[cfg(test)]
#[path = "predicate_column_test.rs"]
mod predicate_column_test;
