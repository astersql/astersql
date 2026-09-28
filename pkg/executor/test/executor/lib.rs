// Copyright 2026 AsterSQL.

// 执行器综合测试 crate 入口。
//
// 对应 Go `pkg/executor/test/executor`：覆盖时区下推、Point Get、
// 历史读（AS OF TIMESTAMP / 事务内快照）、Admin DDL、Union、
// 内存控制等执行器路径。聚合 `executor_test` 与包级 `main_test`。

#![allow(dead_code)]

/// 执行器功能与回归用例（含 Go 原文草稿与可执行 testkit 冒烟）。
#[cfg(test)]
mod executor_test;
/// 包级 TestMain：自增步长与慢日志阈值语义。
#[cfg(test)]
mod main_test;
