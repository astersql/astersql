// Copyright 2026 AsterSQL.

// 历史统计 crate 入口：对外导出 `history_stats` 中的落盘与元数据 API。

#![allow(dead_code)]
/// 历史统计快照落盘、元数据记录与相关类型定义。
pub mod history_stats;
pub use history_stats::*;

#[cfg(test)]
mod history_stats_aster_unit_test;
