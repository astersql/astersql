// Copyright 2026 AsterSQL.

// `util/dbutil` crate 入口：数据库配置、索引/表元数据、查询扫描、重试与变量查询。
//
// 对应 Go `pkg/util/dbutil`。子模块按职责拆分；对外再导出常用配置与执行器类型。
// 测试通过 `#[path]` 挂到本 crate，与 Go 同目录测试文件一一对应。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 通用配置、表名/列名拼装与连接辅助。
pub mod common;
/// 索引信息查询与索引列选择。
pub mod index;
/// 查询/事务执行器抽象与通用值类型。
pub mod interface;
/// 结果集扫描为二维值或按列名映射。
pub mod query;
/// 可重试错误码与消息判定。
pub mod retry;
/// 表模式与按名查列等表级辅助。
pub mod table;
/// MySQL 字段类型常量与类型归类。
pub mod types;
/// SHOW VARIABLES / SHOW GRANTS 等变量与权限查询。
pub mod variable;

pub use common::{ColumnName, DBConfig, TableName};
pub use interface::{DBExecutor, DbError, QueryExecutor, QueryResult, Transaction, Value};

#[cfg(test)]
#[path = "common_test.rs"]
mod common_test;
#[cfg(test)]
#[path = "index_test.rs"]
mod index_test;
#[cfg(test)]
#[path = "query_test.rs"]
mod query_test;
#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
#[cfg(test)]
#[path = "variable_test.rs"]
mod variable_test;
