// Copyright 2026 AsterSQL.

// sqlexec mock crate 根：为受限 SQL 执行器提供 GoMock 风格替身。
//
// 再导出父 crate 的 AST/Chunk/Context，并暴露 mock 键与 `MockRestrictedSQLExecutor`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 父 sqlexec crate 的再导出别名。
pub use sqlexec_crate as sqlexec;
/// 常用依赖类型的便捷再导出。
pub use sqlexec_crate::{ast, chunk, context, resolve};

// Context 键：标识 mock 执行器在 ctx 中的槽位。
mod mock;
pub use mock::*;
// GoMock 生成逻辑的 Rust 移植：期望队列与 FIFO 消费。
mod restricted_sql_executor_mock;
pub use restricted_sql_executor_mock::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
