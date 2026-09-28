// Copyright 2026 AsterSQL.

// `initstats` crate 入口：启动时并发加载统计信息。
//
// 聚合并发度计算（`load_stats`）与按表 ID 区间的 RangeWorker
// （`load_stats_page`），并转发 config / 日志依赖。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as astersql_statistics_handle_initstats;
/// 全局配置依赖（含 `Performance.ForceInitStats` 等）。
pub mod config {
    pub use config_dependency::*;
}
/// 工具与日志桥接（后台采样日志等）。
pub mod util {
    pub mod logutil {
        pub use logutil_dependency::log::*;
    }
}
/// 加载统计时的并发度计算。
mod load_stats;
pub use load_stats::*;
/// 按表 ID 区间并发加载的 RangeWorker 与进度百分比。
mod load_stats_page;
pub use load_stats_page::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移相关单元测试。
mod migration_aster_unit_test;
