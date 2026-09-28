// Copyright 2026 AsterSQL.

// lockwaiter crate 入口：悲观锁等待队列管理。
//
// 悲观锁（pessimistic lock）在事务预写前先占锁；当目标键已被其他事务锁定时，
// 当前事务进入等待队列，直到锁释放、超时或检测到死锁（deadlock）。
// 本文件声明 lockwaiter 实现模块、再导出公开 API，并挂接测试入口。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

/// 锁等待管理器与 Waiter 实现。
pub mod lockwaiter;
pub use lockwaiter::*;
/// 复用 unistore 配置（含悲观事务唤醒延迟等）。
pub use unistore_config as config;

/// Go TestMain 来源说明及 Rust 资源生命周期验证。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
