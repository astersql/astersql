// Copyright 2026 AsterSQL.

// 慢日志（slow log）规则测试 crate 入口。
//
// 在测试配置下挂载 `main_test` 与 `slow_log_test`，对应 Go
// `sessionctx/variable/tests/slowlog` 包。

#![allow(dead_code, non_snake_case)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod slow_log_test;
