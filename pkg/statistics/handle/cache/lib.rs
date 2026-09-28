// Copyright 2026 AsterSQL.
// 统计信息缓存（stats cache）包入口。
//
// 提供缓存错误类型、完整统计表 `StatisticsTable`，以及行数/列长度缓存、
// 统计缓存实现等子模块再导出。健康度（healthy）衡量表统计相对修改量的新鲜程度，
// 供自动 ANALYZE（分析收集统计）与监控指标使用。
#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]
use std::fmt;
/// 统计缓存相关错误，包装可读错误消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheError(pub String);
impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CacheError {}
/// 完整统计表，与 storage、LFU 和 Handle 共用同一类型。
pub use statistics::Table as StatisticsTable;

mod stats_table_row_cache;
mod statscache;
mod statscacheinner;
pub use stats_table_row_cache::*;
pub use statscache::*;
pub use statscacheinner::*;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "stats_table_row_cache_test.rs"]
mod stats_table_row_cache_test;
#[cfg(test)]
#[path = "statscache_test.rs"]
mod statscache_test;
