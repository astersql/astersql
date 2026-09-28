// Copyright 2026 AsterSQL.

// KV store driver 的退避（backoff）crate 入口。
//
// 退避指请求失败后按策略等待再重试，用于 Region 缓存未命中、事务锁冲突等瞬时错误。
// 本 crate 聚合错误类型、KV 会话变量依赖，并导出 `backoff` 模块中的 Backoffer 实现。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 共享错误类型再导出（来自 driver_error 依赖）。
pub mod errors {
    pub use driver_error_dependency::errors::*;
}

/// TiKV / driver 错误转换与判定工具再导出。
pub mod driver_error {
    pub use driver_error_dependency::*;
}

/// KV 会话变量（如 BackoffLockFast、BackOffWeight、Killed）再导出。
pub mod kv {
    pub use kv_dependency::*;
}

/// 退避配置与 Backoffer 实现。
mod backoff;
pub use backoff::*;

/// Aster 迁移对照的退避单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod backoff_test;
