// Copyright 2026 AsterSQL.

// QUERY WATCH 执行器子 crate 入口。
//
// QUERY WATCH（查询监视 / runaway 隔离规则）用于按资源组、SQL 文本或
// 执行计划摘要识别并处置失控查询。本 crate 导出 `query_watch` 模块，
// 并在测试配置下挂载 `main_test` 与 `query_watch_test`。
#![allow(dead_code)]

/// ADD/DROP QUERY WATCH 的选项解析、校验与执行逻辑。
pub mod query_watch;

#[cfg(test)]
/// 包级 TestMain：common test 初始化与 autoid/慢查询配置。
mod main_test;
#[cfg(test)]
/// QUERY WATCH 单元测试。
mod query_watch_test;
