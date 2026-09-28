// Copyright 2026 AsterSQL.

// `tracing` crate 入口：优化器 CE 追踪与通用 Span / 类别工具。
//
// 对应 Go `pkg/util/tracing`。对外重导出 `opt_trace`（代价估计 CE 记录）
// 与 `util`（Span、TraceCategory、FlightRecorder 绑定等）；测试经 `#[path]` 挂载。

#![allow(non_snake_case)]

/// 优化器代价估计（CE）追踪记录与去重。
pub mod opt_trace;
/// 通用追踪工具：类别位图、Span、Context 与 Region 起止事件。
pub mod util;

pub use opt_trace::*;
pub use util::*;

/// 类别开关测试互斥锁：Enable/Disable/SetCategories 改全局状态，用例需串行。
#[cfg(test)]
pub(crate) static CATEGORY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 迁移对齐：CE 去重、类别、Span 与 Region 行为回归。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// Noop Span 基准测试骨架。
#[cfg(test)]
#[path = "noop_bench_test.rs"]
mod noop_bench_test;

/// util 子模块单元测试。
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
