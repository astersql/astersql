// Copyright 2026 AsterSQL.

// 连接顺序（join order）重排 crate 的库入口。
//
// 连接重排在代价优化器中枚举多表连接树形态：先由冲突检测器构图，
// 再按顶点数选择动态规划（DP）或贪心；`ordered_leading` 处理 LEADING hint。

#![allow(dead_code)]

/// 冲突规则与边/顶点构图，校验候选连接是否合法。
pub mod conflict_detector;
/// DP / 贪心连接顺序枚举与优化入口。
pub mod join_order;
/// LEADING hint 有序前缀约束。
pub mod ordered_leading;
/// 逻辑计划节点、连接类型与 hint 等共享工具。
pub mod util;

/// 大规模 Rule/Edge 收敛的规模矩阵测试（由 Go bitset bench 迁移）。
#[cfg(test)]
mod bitset_bench_test;
/// ConflictDetector 与 Go 版本的方向、边消费及剩余边语义回归。
#[cfg(test)]
mod conflict_detector_test;
/// 贪心起点选择与 Node clone 隔离等语义测试。
#[cfg(test)]
mod join_order_test;
/// Ordered Leading 的输入校验、候选选择与内部 hint 冲突语义回归。
#[cfg(test)]
mod ordered_leading_test;
/// Join-order shared helpers' Go parity regressions.
#[cfg(test)]
mod util_test;
