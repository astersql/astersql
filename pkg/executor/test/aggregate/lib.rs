// Copyright 2026 AsterSQL.

// 聚合执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/aggregate` 包。聚合（Aggregate）算子负责
// GROUP BY / COUNT / SUM 等分组汇总；本 crate 挂载功能用例与包级 TestMain。

#![allow(dead_code)]

/// 聚合算子（HashAgg、StreamAgg 等）功能与回归测试。
#[cfg(test)]
mod aggregate_test;
/// 包级 TestMain：全局配置更新。
#[cfg(test)]
mod main_test;
