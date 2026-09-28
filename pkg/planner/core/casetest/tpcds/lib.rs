// Copyright 2026 AsterSQL.

// TPC-DS casetest crate 入口。
//
// 聚合零售决策支持基准 TPC-DS 相关用例：官方 schema 建表、ANALYZE 统计与 Q64 子集。
// 仅在 `cfg(test)` 下挂载子模块。
//
// TPC-DS：Transaction Processing Performance Council Decision Support 基准，含多事实/维表。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
/// TPC-DS 建表/主键/ANALYZE 可执行回归（对应 Go tpcds_test.go 建表子集）。
#[cfg(test)]
mod tpcds_test;
