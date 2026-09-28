// Copyright 2026 AsterSQL.

// 统计信息句柄（statistics handle）crate 根。
//
// 聚合 bootstrap、句柄实现、受限 SQL、运行时统计，并再导出锁统计与存储子 crate，
// 以及自动 ANALYZE 最小行数相关配置入口。

#![allow(dead_code)]

pub mod bootstrap;
pub mod handle;
pub mod restricted_sql;
pub mod runtime_stats;

pub use astersql_statistics::{
    EffectiveAutoAnalyzeMinCnt, ResetAutoAnalyzeMinCnt, SetAutoAnalyzeMinCnt,
};
pub use astersql_statistics_handle_lockstats as lockstats;
pub use astersql_statistics_handle_storage as storage;
pub use bootstrap::*;
pub use handle::*;
pub use restricted_sql::*;
pub use runtime_stats::*;

#[cfg(test)]
#[path = "analyze_runtime_aster_unit_test.rs"]
mod analyze_runtime_aster_unit_test;

#[cfg(test)]
#[path = "runtime_stats_test.rs"]
mod runtime_stats_test;

#[cfg(test)]
#[path = "bootstrap_test.rs"]
mod bootstrap_test;

#[cfg(test)]
#[path = "handle_test.rs"]
mod handle_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
