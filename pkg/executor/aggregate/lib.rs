// Copyright 2026 AsterSQL.

// 聚合执行器子模块入口。
//
// 提供 Hash 聚合与流式聚合两类执行路径，以及内存不足时的 spill（落盘）
// 支持。聚合（Aggregation）按分组键汇总行集，是 SQL `GROUP BY` /
// 聚合函数（如 `SUM`/`COUNT`）的执行层实现。

#![allow(dead_code)]

/// Hash 聚合（Hash Aggregation）基础 worker：在分区/桶上执行部分聚合。
pub mod agg_hash_base_worker;
/// Hash 聚合执行器：基于哈希表按分组键聚合输入行。
pub mod agg_hash_executor;
/// Hash 聚合最终阶段 worker：合并各 partial 结果得到最终聚合值。
pub mod agg_hash_final_worker;
/// Hash 聚合部分阶段 worker：对输入分片做局部聚合以降低最终合并量。
pub mod agg_hash_partial_worker;
/// 聚合溢出（spill）支持：内存不足时将中间状态落盘再回读合并。
pub mod agg_spill;
/// 流式聚合执行器：要求输入已按分组键有序，可边读边输出。
pub mod agg_stream_executor;
/// 聚合公共工具函数与辅助类型。
pub mod agg_util;

#[cfg(test)]
mod agg_hash_base_worker_test;
#[cfg(test)]
mod agg_hash_executor_test;
#[cfg(test)]
mod agg_hash_partial_worker_test;
#[cfg(test)]
/// 聚合溢出相关单元测试（仅在启用 test 配置时编译）。
mod agg_spill_test;
