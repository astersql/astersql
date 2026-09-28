// Copyright 2026 AsterSQL.

// 系统进程跟踪（sysproctrack）子 crate 入口。
//
// 导出可被会话管理器注册/注销的系统进程跟踪接口（`Tracker` / `TrackProc`），
// 用于 SHOW PROCESSLIST 与杀进程等管理路径；测试仅在 `cfg(test)` 下编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 进程跟踪核心 trait 与类型（对应 Go `sysproctrack`）。
mod track;
pub use track::*;

/// 迁移期单元测试：Track/UnTrack、进程列表与 Kill 语义对照 Go。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
