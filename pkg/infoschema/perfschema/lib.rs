// Copyright 2026 AsterSQL.

// PERFORMANCE_SCHEMA（性能模式）crate 入口。
//
// 聚合常量 DDL、初始化注册与虚拟表实现；对外再导出 Init / 预定义表查询等符号。
// Performance Schema：MySQL 兼容的运行时性能统计虚拟库。

#![allow(dead_code)]

/// 建表 SQL 与表名常量（模块文件名为 const.rs）。
#[path = "const.rs"]
pub mod consts;
/// 解析静态 DDL 并注册 PERFORMANCE_SCHEMA 虚拟库。
pub mod init;
/// 虚拟表包装、行数据源与远端 profile 拉取。
pub mod tables;

pub use init::{
    Init, build_performance_schema, init, registered_databases, set_eval_simple_ast_ready,
};
pub use tables::{IsPredefinedTable, PerfSchemaTable, is_predefined_table, table_from_meta};

#[cfg(test)]
#[path = "init_test.rs"]
mod init_test;
#[cfg(test)]
#[path = "tables_test.rs"]
mod tables_test;
