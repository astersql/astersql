// Copyright 2026 AsterSQL.

// 子查询（subquery）相关集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `subquery_test`（collation 下 IN 子查询计划形状）与
// `main_test`（对应 Go TestMain）。子查询是嵌套在另一查询中的 SELECT；
// IN 子查询常改写为半连接（semi-join）或 IndexHashJoin 等物理计划。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// collation 与 IN 子查询计划形状对齐用例。
#[cfg(test)]
#[path = "subquery_test.rs"]
mod subquery_test;
