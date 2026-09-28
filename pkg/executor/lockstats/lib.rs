// Copyright 2026 AsterSQL.

// 统计信息锁定（Lock Stats）子 crate。
//
// 提供 `LOCK STATS` / `UNLOCK STATS` 执行器：阻止自动 ANALYZE 覆盖指定
// 表或分区上的统计，便于人工维护的统计在变更后保持稳定。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 锁定表/分区统计的执行器与元数据解析辅助。
pub mod lock_stats_executor;
/// 解除统计锁定的执行器。
pub mod unlock_stats_executor;

#[cfg(test)]
#[path = "lock_stats_executor_test.rs"]
mod lock_stats_executor_test;
