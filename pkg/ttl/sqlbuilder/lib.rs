// Copyright 2026 AsterSQL.

// TTL SQL 构建器 crate 根。
//
// 负责为 TTL 扫描与删除任务拼装 SELECT / DELETE 等语句片段；
// 具体构建逻辑在 `sql` 子模块，本文件仅做模块声明与 re-export。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// TTL SQL 语句构建实现。
pub mod sql;
pub use sql::*;

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod sql_test;
