// Copyright 2026 AsterSQL.

// `handletest` crate 入口：统计 handle 集成测试总包。
//
// 聚合 handle 行为用例（`handle_test`）与包级 mock store/Domain harness（`main_test`）。
// 统计 handle 负责加载、缓存、ANALYZE、增量 delta 等表统计生命周期。

#![allow(dead_code)]

#[cfg(test)]
#[path = "handle_test.rs"]
/// 对应 Go `handle_test.go` 的统计 handle 集成用例。
mod handle_test;

#[cfg(test)]
#[path = "integration_test.rs"]
/// 对应 Go `pkg/statistics/integration_test.go` 的跨组件 SQL 回归。
mod integration_test;

#[cfg(test)]
#[path = "main_test.rs"]
/// 包级 TestMain：公共 setup、mock Domain/TestKit 工厂与 failpoint 互斥。
mod main_test;
