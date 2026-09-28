// Copyright 2026 AsterSQL.

// ANALYZE 相关集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `analyze_test`（端到端 ANALYZE 行为）与
// `main_test`（对应 Go TestMain 的参考入口）。ANALYZE 用于收集表/列/索引
// 统计信息，供优化器估算代价。

#![allow(dead_code)]

/// 端到端 ANALYZE / 自动 ANALYZE 用例。
#[cfg(test)]
#[path = "analyze_test.rs"]
mod analyze_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
