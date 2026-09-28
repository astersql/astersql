// Copyright 2026 AsterSQL.

// ANALYZE 执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/analyzetest` 包。ANALYZE 收集表/索引的
// 直方图、TopN、CMSketch 等统计信息，供优化器做基数估计与计划选择。
// 本 crate 通过 `#[path]` 挂载基准、功能冒烟与包级 TestMain。

#![allow(dead_code)]

/// ANALYZE 采样构建 CMSketch/TopN 的基准冒烟。
#[cfg(test)]
#[path = "analyze_bench_test.rs"]
mod analyze_bench_test;
/// ANALYZE 版本判定与倾斜度估计冒烟。
#[cfg(test)]
#[path = "analyze_test.rs"]
mod analyze_test;
/// 包级 TestMain：统计加载状态相关冒烟入口。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
