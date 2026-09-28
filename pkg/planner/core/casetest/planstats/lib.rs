// Copyright 2026 AsterSQL.

// `planstats` casetest crate 入口。
//
// 计划侧统计信息（plan stats）加载与依赖收集相关用例：验证 analyze 后直方图/TopN、
// stats lease 往返、`LoadNeededHistograms` 幂等性、DDL 推动 stats_meta 版本，以及
// 虚拟列依赖列集合扩展。统计信息（statistics）供优化器估算代价与选择率。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
/// 计划统计加载、lease、虚拟列依赖等真实 API 回归。
#[cfg(test)]
mod plan_stats_test;
