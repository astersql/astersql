// Copyright 2026 AsterSQL.

// 会话上下文（sessionctx）crate 根模块。
//
// 导出会话执行上下文相关的核心类型与 trait（见 `context` 子模块），
// 供上层会话、执行器与会话迁移等组件共享；测试模块仅在 `cfg(test)` 下编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_sessionctx;

/// 会话上下文核心定义（执行上下文、快照读校验、基础 Ctx 类型等）。
mod context;
pub use context::*;

/// 迁移期单元测试：对照 Go 的 BasicCtxType 与快照读时间戳校验行为。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 会话上下文单元测试（对应 Go `context_test.go`）。
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;

/// 测试入口辅助：通用 Setup（对应 Go `TestMain`）。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
