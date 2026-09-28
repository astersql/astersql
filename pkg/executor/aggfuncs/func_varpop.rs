// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 总体方差聚合函数（`VAR_POP` / `VARIANCE`）实现。
//
// 对应 Go 的 `func_varpop.go`：维护可合并的中间方差状态（count、sum、variance），
// 支持普通聚合与 `DISTINCT` 去重路径。总体方差（population variance）按
// `Σ(x-μ)² / n` 计算；样本方差（sample variance）则除以 `n-1`，供 `VAR_SAMP` 复用。
// 增量更新与分区合并采用与 Go 一致的 online 公式，避免先物化全部值再二次扫描。

use std::collections::HashMap;

/// 方差聚合的部分结果（partial result）：累计计数、求和与中间方差量。
///
/// `variance` 字段存的是可合并的中间量（接近 `Σ(x-μ)²`），最终总体/样本方差
/// 分别再除以 `count` 或 `count-1`。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VarianceState {
    count: i64,
    sum: f64,
    variance: f64,
}

impl VarianceState {
    /// 清空状态，回到初始默认值。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// 返回已参与累计的非空值个数。
    pub fn count(&self) -> i64 {
        self.count
    }
    /// 用一批可空输入增量更新方差状态；`None`（SQL NULL）被跳过。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        for input in values.into_iter().flatten() {
            self.count += 1;
            self.sum += input;
            // 首个有效值只需累计 count/sum；从第二个起用 online 公式更新中间方差。
            if self.count > 1 {
                self.variance = calculate_intermediate(self.count, self.sum, input, self.variance);
            }
        }
    }
    /// 合并另一个分区的部分结果（hash/stream 聚合的 merge 阶段）。
    pub fn merge(&mut self, source: &Self) {
        if source.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = *source;
            return;
        }
        // 两边都有数据时，用两集合均值差修正合并后的中间方差，再累加 count/sum。
        self.variance = calculate_merge(
            source.count,
            self.count,
            source.sum,
            self.sum,
            source.variance,
            self.variance,
        );
        self.count += source.count;
        self.sum += source.sum;
    }
    /// 总体方差：`中间方差 / n`；无有效行时返回 `None`。
    pub fn population_variance(&self) -> Option<f64> {
        (self.count != 0).then_some(self.variance / self.count as f64)
    }
    /// 样本方差：`中间方差 / (n-1)`；不足 2 个有效行时返回 `None`。
    pub fn sample_variance(&self) -> Option<f64> {
        (self.count > 1).then_some(self.variance / (self.count - 1) as f64)
    }
}

/// 单点增量更新中间方差（对应 Go 的 `calculateVariance` 在线公式）。
///
/// `delta = n*x - sum`，再按 `delta² / (n*(n-1))` 累加，数值上等价于维护 `Σ(x-μ)²`。
pub fn calculate_intermediate(count: i64, sum: f64, input: f64, variance: f64) -> f64 {
    let delta = count as f64 * input - sum;
    variance + delta * delta / (count * (count - 1)) as f64
}

/// 合并两个已有部分结果的中间方差（对应 Go 的 merge 公式）。
///
/// 用两边均值之差 `δ`，按 `δ² * n1 * n2 / (n1+n2)` 补偿跨分区均值偏移。
pub fn calculate_merge(
    src_count: i64,
    dst_count: i64,
    src_sum: f64,
    dst_sum: f64,
    src_variance: f64,
    dst_variance: f64,
) -> f64 {
    let total = src_count + dst_count;
    let delta = src_sum / src_count as f64 - dst_sum / dst_count as f64;
    src_variance + dst_variance + delta * delta * src_count as f64 * dst_count as f64 / total as f64
}

/// `DISTINCT` 方差的部分结果：按 Go `map[float64]` 的键语义去重。
#[derive(Clone, Debug)]
pub struct DistinctVariance {
    values: HashMap<u64, f64>,
    next_nan_payload: u64,
}
impl Default for DistinctVariance {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            next_nan_payload: 1,
        }
    }
}
impl DistinctVariance {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.values.clear();
        self.next_nan_payload = 1;
    }
    /// 插入非空输入；±0 共用一个键，NaN 每次插入都生成新键，与 Go map 一致。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        for value in values.into_iter().flatten() {
            let key = if value.is_nan() {
                const NAN_PAYLOAD_MASK: u64 = 0x000f_ffff_ffff_ffff;
                assert!(
                    self.next_nan_payload <= NAN_PAYLOAD_MASK,
                    "too many NaN keys"
                );
                let key = 0x7ff0_0000_0000_0000 | self.next_nan_payload;
                self.next_nan_payload += 1;
                key
            } else if value == 0.0 {
                0
            } else {
                value.to_bits()
            };
            self.values.insert(key, value);
        }
    }
    /// 并入另一份去重集合。
    pub fn merge(&mut self, source: &Self) {
        self.update(source.values.values().copied().map(Some));
    }
    /// 将去重后的值灌入普通 `VarianceState`，得到可求最终方差的状态。
    pub fn state(&self) -> VarianceState {
        let mut state = VarianceState::default();
        state.update(self.values.values().copied().map(Some));
        state
    }
    /// 基于去重集合计算总体方差。
    pub fn population_variance(&self) -> Option<f64> {
        self.state().population_variance()
    }
    /// 基于去重集合计算样本方差。
    pub fn sample_variance(&self) -> Option<f64> {
        self.state().sample_variance()
    }
}

/// float64 路径上 `VAR_POP` 的部分结果类型别名。
pub type VarPop4Float64 = VarianceState;
/// 原始阶段 `VAR_POP DISTINCT` 的部分结果类型别名。
pub type VarPopOriginal4DistinctFloat64 = DistinctVariance;
/// 部分聚合阶段 `VAR_POP DISTINCT` 的部分结果类型别名。
pub type VarPopPartial4DistinctFloat64 = DistinctVariance;
