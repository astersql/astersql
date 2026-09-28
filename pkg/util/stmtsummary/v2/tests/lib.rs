// Copyright 2026 AsterSQL.

// stmtsummary v2 集成测试 crate 根模块。
//
// 导出共享 `harness`，并在测试配置下通过 `#[path]` 挂入 `main_test` / `table_test`。

#![allow(dead_code)]

/// 集成测试共享夹具与语句摘要记账模拟。
pub mod harness;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "table_test.rs"]
/// 对应 Go `table_test.go` 的语句摘要表行为测试。
mod table_test;
