// Copyright 2026 AsterSQL.

// DDL 可执行性检查器（ddl-checker）crate 入口。
//
// 提供：将上游表结构同步到本地检查会话（`ddl_syncer`），以及在隔离
// session 中解析/执行 SQL 以判断 DDL（数据定义语言）是否可执行
// （`executable_checker`）。用于同步或迁移前预检。

#![allow(dead_code, non_snake_case)]

pub mod ddl_syncer;
pub mod executable_checker;

pub use ddl_syncer::{
    DBConfig, DDLSyncer, NewDDLSyncer, UpstreamDatabase, UpstreamDatabaseFactory,
};
pub use executable_checker::{
    CheckerError, CheckerParser, CheckerResult, CheckerSession, ExecutableChecker,
    ExecutableCheckerFactory, ExecutionContext, GetTablesNeededExist, GetTablesNeededNonExist,
    IsDDL, NewExecutableChecker, RenameTablePair, Statement, StatementFromAst,
};

#[cfg(test)]
#[path = "executable_checker_test.rs"]
mod executable_checker_test;
