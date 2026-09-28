// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// STDDEV_SAMP（样本标准差）聚合函数。
//
// 样本标准差 = √(样本方差)；分母为 N−1（无偏估计），行数不足 2 时结果为 NULL。
// 与 STDDEV_POP（分母 N）相对；内部复用方差状态，本模块只做开方收尾。
// DISTINCT 变体先去重再计算样本标准差。

use crate::func_varpop::{DistinctVariance, VarianceState};
/// 由样本方差取平方根得到 STDDEV_SAMP；行数 < 2 时返回 None。
pub fn stddev_sample(state: &VarianceState) -> Option<f64> {
    state.sample_variance().map(f64::sqrt)
}
/// DISTINCT 路径：对去重后的方差状态取样本标准差。
pub fn stddev_sample_distinct(state: &DistinctVariance) -> Option<f64> {
    state.sample_variance().map(f64::sqrt)
}
/// Float64 样本标准差的部分结果类型别名。
pub type StddevSamp4Float64 = VarianceState;
/// DISTINCT Float64 样本标准差的部分结果类型别名。
pub type StddevSampDistinctFloat64 = DistinctVariance;
