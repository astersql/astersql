// Copyright 2026 AsterSQL.

// 相关子查询（correlated subquery）规划器用例测试 crate 入口。
//
// 相关子查询指内层查询引用外层列的子查询；优化器常通过 Apply（侧向连接）
// 或解相关（decorrelate）改写处理。本 crate 在测试配置下挂载
// `correlated_test` 与 `main_test`。

#![allow(dead_code)]

/// 相关子查询 / NATURAL JOIN / NO_DECORRELATE 等用例。
#[cfg(test)]
#[path = "correlated_test.rs"]
mod correlated_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
