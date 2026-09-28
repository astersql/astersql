// Copyright 2026 AsterSQL.

// unistore Prometheus 指标 crate 入口。
//
// 将 `metrics.rs` 中的 Raft/锁相关 Histogram 再导出，供 unistore 写入路径
// 观测等待与更新耗时；测试配置下挂载迁移单测。

#![allow(non_snake_case, non_upper_case_globals)]

/// 指标实现模块（Histogram 定义与 RegisterMetrics）。
#[path = "metrics.rs"]
mod implementation;
pub use implementation::*;

/// Aster 迁移单测：校验 Histogram 桶边界与 Go 侧一致。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
