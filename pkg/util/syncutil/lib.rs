// Copyright 2026 AsterSQL.

// `syncutil` crate 入口：互斥锁 / 读写锁封装，可选死锁检测构建变体。
//
// 对应 Go `pkg/util/syncutil`。`deadlock` feature 开启时导出 `mutex_deadlock`，
// 否则导出普通 `mutex_sync`；测试通过 `#[path]` 挂载迁移对齐用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 启用死锁检测包装的 Mutex/RWMutex（对应 Go `deadlock` 构建标签）。
pub mod mutex_deadlock;
/// 普通 parking_lot 互斥锁封装（对应 Go 默认 sync 实现）。
pub mod mutex_sync;

/// deadlock feature 开启时，对外使用带死锁检测的锁类型与常量。
#[cfg(feature = "deadlock")]
pub use mutex_deadlock::*;

/// 默认构建：对外使用无死锁检测的锁类型与常量。
#[cfg(not(feature = "deadlock"))]
pub use mutex_sync::*;

/// 迁移对齐单测：验证两种构建变体的锁语义与并发行为。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod mutex_deadlock_test;
