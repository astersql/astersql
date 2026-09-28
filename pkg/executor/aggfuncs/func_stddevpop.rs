// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// STDDEV_POP（总体标准差）聚合函数。
//
// 总体标准差 = √(总体方差)；分母为 N（全体样本数），与 STDDEV_SAMP 的 N−1 不同。
// 内部复用 `VarianceState` / `DistinctVariance` 的方差累加，本模块只做开方收尾。
// DISTINCT 变体先对输入去重再计算总体标准差。

use crate::func_varpop::{DistinctVariance, VarianceState};
/// 由总体方差取平方根得到 STDDEV_POP；无有效行时返回 None。
pub fn stddev_population(state: &VarianceState) -> Option<f64> {
    state.population_variance().map(f64::sqrt)
}
/// DISTINCT 路径：对去重后的方差状态取总体标准差。
pub fn stddev_population_distinct(state: &DistinctVariance) -> Option<f64> {
    state.population_variance().map(f64::sqrt)
}
/// Float64 总体标准差的部分结果类型别名（与方差状态同结构）。
pub type StddevPop4Float64 = VarianceState;
/// DISTINCT Float64 总体标准差的部分结果类型别名。
pub type StddevPopDistinctFloat64 = DistinctVariance;
