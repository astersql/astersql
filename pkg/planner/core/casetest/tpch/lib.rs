// Copyright 2026 AsterSQL.

// TPC-H casetest crate 入口。
//
// 聚合决策支持基准 TPC-H 相关用例：schema/stats 加载与 cost-trace 生命周期顺序。
// 仅在 `cfg(test)` 下挂载子模块。
//
// TPC-H：Transaction Processing Performance Council Ad-hoc 查询基准，侧重复杂分析 SQL。

#![allow(dead_code)]

/// 对照 Go TestMain 与 cost-trace 启用顺序断言。
#[cfg(test)]
mod main_test;
/// TPC-H 查询/计划相关可执行用例。
#[cfg(test)]
mod tpch_test;
