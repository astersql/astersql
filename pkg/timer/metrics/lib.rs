// Copyright 2026 AsterSQL.

// Timer 指标（metrics）子模块入口。
//
// 导出 Prometheus 计数器相关类型与初始化函数，用于统计定时器事件
// （如触发、刷新、Hook 回调）的发生次数，按 scope/type 标签区分来源。

#![allow(non_snake_case, non_upper_case_globals)]

/// 指标定义与初始化实现。
pub mod metrics;
pub use metrics::*;

/// 与 Go 指标描述符/标签约定对齐的单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
