// Copyright 2026 AsterSQL.

// CH（ClickHouse / TPC-C·TPC-H 混合基准）规划器用例测试 crate 入口。
//
// 在 `cfg(test)` 下挂载 `ch_test`（Q2/Q5 风格多表 join reorder）与
// `main_test`（对照 Go TestMain / 建表 helper 的迁移说明）。
// 执行计划（query plan）指优化器为 SQL 选出的算子树。

#![allow(dead_code)]

/// CH 风格多表连接与 join reorder 用例。
#[cfg(test)]
mod ch_test;
/// 对照 Go TestMain 与 CH 表建表 helper 的迁移参考。
#[cfg(test)]
mod main_test;
