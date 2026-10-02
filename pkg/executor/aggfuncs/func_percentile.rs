// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// APPROX_PERCENTILE / 百分位聚合实现。
//
// 收集非 NULL 样本后，按 TiDB 序数秩（ordinal rank）`ceil(p/100 * n)` 选取元素；
// 秩从 1 起，内部用 `select_nth_unstable_by` 做第 `(rank-1)` 小元素选择。
// 合并部分结果时按「目标在前、源在后」拼接后清空源缓冲，与 Go 语义一致。

use crate::func_max_min::{DurationValue, TimeValue};
use crate::func_sum::Decimal;
use std::cmp::Ordering;
use std::mem::size_of;

/// 空 `Vec` 的固定开销常量，供内存追踪与 Go `DefSliceSize` 对齐。
pub const DEF_SLICE_SIZE: i64 = size_of::<Vec<()>>() as i64;

/// Returns TiDB's one-based ordinal rank: ceil(percent / 100 * row_count).
/// 返回 TiDB 从 1 起的序数秩：`ceil(percent/100 * row_count)`，并截断到 `row_count`。
pub fn ordinal_rank(row_count: usize, percent: i32) -> usize {
    (((row_count as f64) * (percent as f64 / 100.0)).ceil() as usize).min(row_count)
}

/// 百分位聚合状态：保存目标百分位与样本向量。
#[derive(Clone, Debug, PartialEq)]
pub struct Percentile<T> {
    percent: i32,
    pub(crate) data: Vec<T>,
}

impl<T> Percentile<T> {
    /// 以目标百分位（通常 0..=100）构造空样本集。
    pub fn new(percent: i32) -> Self {
        Self {
            percent,
            data: Vec::new(),
        }
    }

    /// 清空样本，准备下一组聚合。
    pub fn reset(&mut self) {
        // Go assigns a fresh empty slice here, releasing the old backing array.
        self.data = Vec::new();
    }

    /// Evaluated SQL NULLs are omitted, as in every Go typed implementation.
    /// 跳过 SQL NULL（`None`），只追加有效样本；返回按元素大小估算的内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<T>>) -> i64 {
        let old_len = self.data.len();
        self.data.extend(values.into_iter().flatten());
        (self.data.len() - old_len) as i64 * size_of::<T>() as i64
    }

    /// Go constructs a fresh buffer in destination-first, source-second order
    /// and releases the source slice. Selection itself does not depend on order.
    /// 按「目标缓冲在前、源缓冲在后」合并，并清空源；选择结果与拼接顺序无关。
    pub fn merge_from(&mut self, source: &mut Self) {
        let mut merged = Vec::with_capacity(self.data.len() + source.data.len());
        merged.append(&mut self.data);
        merged.append(&mut source.data);
        self.data = merged;
    }

    /// 按自定义比较器选出序数秩对应元素；无样本或秩为 0 时返回 `None`。
    pub fn result_by(&mut self, compare: impl Fn(&T, &T) -> Ordering) -> Option<&T> {
        let rank = ordinal_rank(self.data.len(), self.percent);
        // percent=0 或空集：秩为 0，无结果。
        if rank == 0 {
            return None;
        }
        // 1-based 秩转为 0-based 下标后做 nth 选择（会重排 `data`）。
        let (_, selected, _) = self
            .data
            .select_nth_unstable_by(rank - 1, |left, right| compare(left, right));
        Some(selected)
    }

    /// 只读访问当前样本切片（合并后源侧应为空）。
    pub fn values(&self) -> &[T] {
        &self.data
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.data.capacity()
    }
}

impl<T: Ord> Percentile<T> {
    /// 使用全序比较计算百分位结果。
    pub fn result(&mut self) -> Option<&T> {
        self.result_by(Ord::cmp)
    }
}

impl Percentile<f32> {
    /// 浮点 f32：以 `partial_cmp` 比较，不可比时视为相等。
    pub fn result_float32(&mut self) -> Option<&f32> {
        self.result_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal))
    }
}

impl Percentile<f64> {
    /// 浮点 f64：以 `partial_cmp` 比较，不可比时视为相等。
    pub fn result_float64(&mut self) -> Option<&f64> {
        self.result_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal))
    }
}

/// 各 SQL 类型对应的百分位状态别名，与 Go 命名对齐。
pub type PercentileOriginal4Int = Percentile<i64>;
pub type PercentileOriginal4Real = Percentile<f64>;
pub type PercentileOriginal4Decimal = Percentile<Decimal>;
pub type PercentileOriginal4Time = Percentile<TimeValue>;
pub type PercentileOriginal4Duration = Percentile<DurationValue>;
