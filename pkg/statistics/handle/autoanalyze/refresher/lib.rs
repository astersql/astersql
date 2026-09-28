// Copyright 2026 AsterSQL.

// 自动 ANALYZE 刷新器（refresher）crate 入口。
//
// 聚合优先级队列调度（`refresher`）与并发执行 worker，
// 并在测试配置下挂接对应单元测试模块。

#![allow(dead_code)]
/// 优先级队列刷新与调度逻辑（对应 Go refresher）。
pub mod refresher;
/// 自动 ANALYZE 作业并发执行 worker。
pub mod worker;
pub use refresher::*;
pub use worker::*;
#[cfg(test)]
/// Refresher 行为相关单元测试。
mod refresher_test;
#[cfg(test)]
/// Worker 并发与生命周期相关单元测试。
mod worker_test;
