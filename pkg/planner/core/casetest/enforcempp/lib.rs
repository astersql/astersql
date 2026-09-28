// Copyright 2026 AsterSQL.

// Enforce MPP（强制大规模并行处理）规划器用例测试 crate 入口。
//
// MPP（Massively Parallel Processing）将查询拆到多个计算节点并行执行，
// 常配合 TiFlash 列存副本。Enforce MPP 在优化阶段强制选择可下推的 MPP 计划。
// 本 crate 仅在 `cfg(test)` 下挂载 `main_test` 与 `enforce_mpp_test`。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// Enforce MPP 相关：session 变量、DDL、explain 等用例。
#[cfg(test)]
#[path = "enforce_mpp_test.rs"]
mod enforce_mpp_test;
