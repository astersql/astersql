// Copyright 2026 AsterSQL.

// DAG（有向无环图）物理计划构建用例测试 crate 入口。
//
// TiDB/AsterSQL 将执行计划组织为算子 DAG，再下推到 TiKV/TiFlash 等存储引擎。
// 本 crate 在 `cfg(test)` 下挂载 `main_test` 与 `dag_test`。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// DAG 计划构建相关：mock 表、parser、hint、并发变量等直连用例。
#[cfg(test)]
#[path = "dag_test.rs"]
mod dag_test;
