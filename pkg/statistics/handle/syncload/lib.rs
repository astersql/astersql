// Copyright 2026 AsterSQL.

// 同步加载（syncload）crate 入口。
//
// 在查询执行路径上按需同步拉取缺失的列/索引直方图，
// 通过队列与 worker 并发加载，并支持超时与去重（singleflight）。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 同步加载核心实现（队列、worker、缓存更新）。
mod stats_syncload;

/// 对外再导出 syncload 公共 API。
pub use stats_syncload::*;

#[cfg(test)]
/// 同步加载行为单元测试。
#[path = "stats_syncload_test.rs"]
mod stats_syncload_test;
