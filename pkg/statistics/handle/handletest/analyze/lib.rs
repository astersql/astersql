// Copyright 2026 AsterSQL.

// `analyze` 测试子包入口。
//
// 聚合 ANALYZE（收集/刷新表统计信息）相关集成测试：包级 harness（`main_test`）
// 与具体 ANALYZE 行为用例（`analyze_test`）。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
/// 包级测试入口：公共 harness 一次性初始化与幂等校验。
mod main_test;

#[cfg(test)]
#[path = "analyze_test.rs"]
/// ANALYZE 语句执行与统计结果相关用例。
mod analyze_test;
