// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 指数退避（exponential backoff）组合多列统计值。
//
// 用于在缺少精确 GroupNDV 时，将多列 NDV 或选择率按
// `v0 * v1^(1/2) * v2^(1/4) * v3^(1/8)` 组合，并裁剪到调用方给定边界。
// 最多考虑 `MaxExponentialBackoffCols` 列，避免尾部权重过小却仍参与乘积。

// MaxExponentialBackoffCols 是指数退避最多考虑的列数。
// 超过四列后权重 1/2^i 已很小，继续相乘对结果影响有限。
/// 指数退避最多参与组合的列数上限。
pub const MaxExponentialBackoffCols: usize = 4;

/// 对预先排序的统计值应用指数退避，并将结果限制在给定上下界内。
// ApplyExponentialBackoff 对预先排序的值应用指数退避，并把最终结果限制在调用方给定的上下界内。
// 公式保持为 val[0] * val[1]^(1/2) * val[2]^(1/4) * val[3]^(1/8)。
pub fn ApplyExponentialBackoff(sortedValues: &[f64], lowerBound: f64, upperBound: f64) -> f64 {
    if sortedValues.is_empty() {
        // 空输入没有可组合的统计值，直接采用保守下界。
        return lowerBound;
    }
    if sortedValues.len() == 1 {
        // 单值无需退避，只执行与多值路径相同的边界裁剪。
        return sortedValues[0].min(upperBound).max(lowerBound);
    }

    let mut result = sortedValues[0];
    let maxCols = MaxExponentialBackoffCols.min(sortedValues.len());
    for (index, value) in sortedValues.iter().enumerate().take(maxCols).skip(1) {
        let mut weighted = *value;
        // 连续开平方 index 次，等价于把当前列提升到 1/2^index 次幂。
        for _ in 0..index {
            weighted = weighted.sqrt();
        }
        result *= weighted;
    }

    // 同时适用于选择率与 NDV：调用方负责传入各自语义下的合法边界。
    result.min(upperBound).max(lowerBound)
}
