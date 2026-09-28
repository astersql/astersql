// Copyright 2026 AsterSQL.

// 分区表全局统计（global stats）合并 crate 入口。
//
// 将各分区的直方图（Histogram）、TopN、CMSketch、FMSketch 等统计结构
// 合并为分区表级的全局统计，供动态分区裁剪（dynamic prune）下的优化器代价估算使用。

#![allow(dead_code)]
/// 分区统计合并到全局统计的核心类型与算法。
pub mod global_stats;
/// 异步合并分区统计的任务封装。
pub mod global_stats_async;
/// TopN 合并工作器（按分区区间汇总频次）。
pub mod merge_worker;
/// 分区 TopN 合并为全局 TopN 的入口函数。
pub mod topn;
/// 再导出全局统计相关公共 API。
pub use global_stats::*;
pub use global_stats_async::*;
pub use merge_worker::*;
pub use topn::*;
#[cfg(test)]
/// 异步全局统计任务封装回归测试。
mod global_stats_async_test;
#[cfg(test)]
/// 全局统计内部 helper 与 CMS 合并单元测试。
mod global_stats_internal_test;
#[cfg(test)]
/// 全局统计展示、健康度、NDV、DDL 等场景的内存模型回归测试。
mod global_stats_test;
#[cfg(test)]
/// 对应 Go `TestMain` 的公共测试脚手架。
mod main_test;
#[cfg(test)]
/// TopN 合并性能辅助函数与并发参数校验。
mod topn_bench_test;
#[cfg(test)]
/// 分区 TopN 合并正确性单元测试。
mod topn_test;
