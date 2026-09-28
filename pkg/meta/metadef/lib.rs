// Copyright 2026 AsterSQL.

// metadef crate 根模块：元数据定义（meta definition）的公共入口。
//
// 汇总系统库名判定、保留全局对象 ID、以及 `mysql` 系统表建表 SQL 常量。
// 这些定义供 bootstrap、DDL 与 infoschema 在启动或升级时创建/识别系统对象。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as astersql_meta_metadef;

/// 解析器 AST 中的大小写不敏感字符串（CIStr：同时保存原始与小写形式）。
pub mod ast {
    pub use parser_ast::model::{CIStr, NewCIStr};
}
/// MySQL 系统库名常量（`mysql` / `sys` / `workload_schema`）。
pub mod mysql {
    pub use parser_mysql::r#const::{SysDB, SystemDB, WorkloadSchema};
}
/// 库名分类：内存 schema、系统库、BR 临时库等判定函数。
pub mod db;
/// 系统保留全局对象 ID 边界与各 `mysql.*` 表的固定 ID。
pub mod system;
/// `mysql` / `sys` 系统表与视图的建表 SQL 字符串常量。
pub mod system_tables_def;
pub use db::*;
pub use system::*;
pub use system_tables_def::*;

#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "system_test.rs"]
mod system_test;
