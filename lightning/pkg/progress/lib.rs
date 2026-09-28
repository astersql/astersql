// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/progress`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/progress`).
//! crate 根只负责装配最小依赖桩、核心进度逻辑和对齐测试。
//! 调用方通过这里拿到统一导出面，不需要关心实现拆分在哪个文件。
//! 测试模块仅在 `cfg(test)` 下挂载，避免污染生产导出。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 先引入本地边界桩，隔离 common/mydump/errors 的真实依赖。
pub use stubs::*;

#[path = "progress.rs"]
mod progress;
// 任务级与表级进度状态机都从这里对外暴露。
pub use progress::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
