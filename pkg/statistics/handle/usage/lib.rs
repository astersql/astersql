// Copyright 2026 AsterSQL.

// 统计使用率（usage）子包入口。
//
// 聚合索引使用率、谓词列使用时间、会话统计增量采集等模块，
// 并向外再导出其公共 API；测试模块覆盖集成与各子路径。

#![allow(dead_code)]
pub mod index_usage;
pub mod predicate_column;
pub mod session_stats_collect;
pub use index_usage::*;
pub use predicate_column::*;
pub use session_stats_collect::*;

#[cfg(test)]
mod export_test;
#[cfg(test)]
mod index_usage_integration_test;
#[cfg(test)]
mod predicate_column_test;
#[cfg(test)]
mod session_stats_collect_test;
