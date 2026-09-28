// Copyright 2026 AsterSQL.

// `util/channel` crate 入口：channel 排空等辅助工具。
//
// 对应 Go `util/channel`，提供与 Go `for range ch {}` 等价的清空语义，
// 供执行器等并发路径在关闭 channel 前排空残留消息。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// channel 清空与可接收抽象。
pub mod channel;
pub use channel::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充：验证 Clear 排空与等待关闭语义。
mod migration_aster_unit_test;
