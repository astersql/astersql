// Copyright 2026 AsterSQL.

// `autoanalyze/exec` crate 入口。
//
// 聚合自动 ANALYZE 语句执行逻辑（SQL 转义、旧版本统计重写告警、进程注册），
// 并在测试配置下挂接 `exec_test`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
/// 自动 ANALYZE 执行实现（对应 Go `exec` 包）。
mod exec;
pub use exec::*;

#[cfg(test)]
#[path = "exec_test.rs"]
/// 自动 ANALYZE 执行路径的单元测试。
mod exec_test;
