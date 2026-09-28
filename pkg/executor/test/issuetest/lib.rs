// Copyright 2026 AsterSQL.

// Executor issue 回归测试 crate 入口。
//
// 对应 Go `pkg/executor/test/issuetest`：覆盖历史 issue 相关的 UNION、
// join、内存限制、failpoint、事务 schema 变更等执行器事故路径。
// 本文件仅在 `#[cfg(test)]` 下挂接测试模块。

#![allow(dead_code)]

#[cfg(test)]
/// Issue 回归用例：含 Go 原文草稿归档与可执行聚合/分区冒烟。
mod executor_issue_test;
#[cfg(test)]
/// 包级 harness：UNION 与 Region 切分边界的可执行冒烟。
mod main_test;
