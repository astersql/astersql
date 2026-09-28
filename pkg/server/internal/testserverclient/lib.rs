// Copyright 2026 AsterSQL.

// server/internal/testserverclient crate 根模块：集成测试用客户端工具。
//
// 对外再导出 `TestServerClient`、DSN/HTTP status、SQL 场景回归与
// 结果比对辅助，供 server 层端到端测试复用。

#![allow(dead_code)]

/// 测试服务器客户端与 SQL 场景实现。
pub mod server_client;

pub use server_client::{
    ConfigOverrider, ExecuteResult, GO_SCENARIO_NAMES, HttpResponse, MysqlConfig, QueryResult,
    Scenario, SqlExecutor, SqlValue, TestDatabase, TestServerClient, check_rows,
    columns_as_expected, regression_enabled, rows, set_regression_enabled,
};

#[cfg(test)]
#[path = "server_client_test.rs"]
mod server_client_test;
