// Copyright 2026 AsterSQL.

//! BR OpenTracing 辅助包入口：导出 `tracing`（启停 span / MemoryStore）。
//! 对齐 Go `br/pkg/trace`；CLI 在 EnableOpenTracing 时包裹任务执行。
//! 测试含 parity、main_test 与串行 tracing 用例，经 path 挂载。
//! 具体 span 生命周期在 `tracing` 模块，此处只做 crate 装配。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "tracing.rs"]
pub mod tracing;

pub use tracing::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "tracing_serial_test.rs"]
mod tracing_serial_test;

#[cfg(test)]
#[path = "tracing_test.rs"]
mod tracing_test;
