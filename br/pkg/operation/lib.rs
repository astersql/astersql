// Copyright 2026 AsterSQL.

//! BR 操作上下文包入口：导出 `context`（OperationID/Hint/LockMeta）。
//! 对应 Go `br/pkg/operation`，为备份等长任务提供可观测与锁元数据。
//! 测试分文件挂载；本文件仅装配边界。
//! 业务实现见 `context` 模块。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
#[path = "context.rs"]
pub mod context;
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
pub use context::*;
