// Copyright 2026 AsterSQL.

// 死锁历史（Deadlock History）crate 入口。
//
// 对应 INFORMATION_SCHEMA.DEADLOCKS / CLUSTER_DEADLOCKS：以环形缓冲
// 保存最近死锁事件，供诊断查询。死锁指事务互相等待对方持有锁而无法推进。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as deadlockhistory;

pub mod deadlock_history;
pub use deadlock_history::*;

#[cfg(test)]
mod deadlock_history_test;
#[cfg(test)]
mod main_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
