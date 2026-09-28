// Copyright 2026 AsterSQL.

// 语句上下文（stmtctx）子 crate 入口。
//
// 导出单条 SQL 语句执行期状态（`StatementContext` 等），供会话、优化器与执行器共享；
// 测试模块仅在 `cfg(test)` 下编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 自引用别名：测试与模块路径中沿用 Go 迁移期 crate 命名。
extern crate self as astersql_sessionctx_stmtctx;

/// 语句上下文核心实现（对应 Go `stmtctx` 包）。
mod stmtctx;
pub use stmtctx::*;

/// 规划器相关状态的迁移期单元测试（hint 交换、统计加载等）。
#[cfg(test)]
#[path = "planner_state_aster_unit_test.rs"]
mod planner_state_aster_unit_test;
/// 语句上下文综合迁移期单元测试（计数器、缓存、Reset 等对照 Go）。
#[cfg(test)]
#[path = "stmtctx_1_aster_unit_test.rs"]
mod stmtctx_1_aster_unit_test;
/// 语句上下文单元测试原文结构（对应 Go `stmtctx_test.go`）。
#[cfg(test)]
#[path = "stmtctx_test.rs"]
mod stmtctx_test;
