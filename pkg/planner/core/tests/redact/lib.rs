// Copyright 2026 AsterSQL.

// 执行计划日志脱敏（redact）相关集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `main_test`（对应 Go TestMain）与 `redact_test`
//（`tidb_redact_log` 的 MARKER/ON 模式下常量脱敏）。脱敏用于在 EXPLAIN /
// 慢查询等输出中隐藏字面量，避免敏感值进入日志。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// MARKER/ON 脱敏、分区表 / 生成列 fixture 与 TiFlash 相关用例。
#[cfg(test)]
#[path = "redact_test.rs"]
mod redact_test;
