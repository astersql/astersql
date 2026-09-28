// Copyright 2026 AsterSQL.

// `calculatoranalysis` 测试 crate 入口。
//
// 挂接优先级计算器与 Go 同路径 golden CSV 对照测试，以及公共测试 harness 初始化。

#![allow(dead_code)]

#[cfg(test)]
#[path = "calculator_analysis_test.rs"]
/// 用生成数据集驱动真实 `PriorityCalculator`，并与 golden CSV 逐字节比对。
mod calculator_analysis_test;
#[cfg(test)]
#[path = "main_test.rs"]
/// 对应 Go `TestMain` 的公共测试环境准备。
mod main_test;
