// Copyright 2026 AsterSQL.

// Timer 包根入口（crate root）。
//
// 对应 Go 的 `pkg/timer`：汇总定时器子系统的测试入口与集成测试挂载点。
// 业务实现分散在 `api`、`metrics`、`runtime`、`tablestore` 等子模块中。

#![allow(dead_code)]

/// 测试入口：对应 Go 的 `TestMain` 泄漏检查与 common test 初始化草稿。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 定时器存储层集成测试。
#[cfg(test)]
#[path = "store_intergartion_test.rs"]
mod store_intergartion_test;
