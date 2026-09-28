// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// COUNT 聚合函数实现。
//
// 统计非 NULL 行数；`update_partial` / `merge` 用于 HashAgg 两阶段合并部分计数；
// `slide` 支持滑动窗口：移出非空行减一、移入非空行加一。
// Go 按 EvalInt/EvalReal 等拆成多个具体类型；Rust 在进入累加器前已求值，故共享同一状态机。

use crate::aggfuncs::{AggError, PartialResult4Count};
use std::mem::size_of;

/// COUNT 部分结果（一个 i64 计数）的固定内存占用。
pub const DEF_PARTIAL_RESULT_4_COUNT_SIZE: i64 = size_of::<PartialResult4Count>() as i64;

/// COUNT 累加器：内部仅维护一个非负计数。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CountAggregator {
    count: PartialResult4Count,
}

// Go keeps one concrete type per evaluator because EvalInt/EvalReal/etc. are
// statically selected. Rust evaluates before entering the accumulator, so the
// concrete implementations intentionally share this identical state machine.
/// 原始阶段整数列 COUNT（类型别名，实现共享）。
pub type CountOriginal4Int = CountAggregator;
/// 原始阶段浮点列 COUNT。
pub type CountOriginal4Real = CountAggregator;
/// 原始阶段 Decimal 列 COUNT。
pub type CountOriginal4Decimal = CountAggregator;
/// 原始阶段时间类型 COUNT。
pub type CountOriginal4Time = CountAggregator;
/// 原始阶段 Duration 类型 COUNT。
pub type CountOriginal4Duration = CountAggregator;
/// 原始阶段 JSON 类型 COUNT。
pub type CountOriginal4Json = CountAggregator;
/// 原始阶段向量浮点类型 COUNT。
pub type CountOriginal4VectorFloat32 = CountAggregator;
/// 原始阶段字符串类型 COUNT。
pub type CountOriginal4String = CountAggregator;
/// 部分聚合阶段 COUNT。
pub type CountPartial = CountAggregator;

impl CountAggregator {
    /// 当前计数值。
    pub fn value(&self) -> i64 {
        self.count
    }
    /// 重置计数为 0。
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// Implements all original COUNT evaluator variants: int, real, decimal,
    /// time, duration, JSON, vector and string differ only in evaluation.
    /// 对每个非 NULL（`Some`）值加一；与 Go `int64` 一样按补码回绕。
    pub fn update<T>(
        &mut self,
        values: impl IntoIterator<Item = Option<T>>,
    ) -> Result<(), AggError> {
        for value in values {
            if value.is_some() {
                self.count = self.count.wrapping_add(1);
            }
        }
        Ok(())
    }

    /// 合并上游部分计数；与 Go `int64` 一样逐个回绕相加。
    pub fn update_partial(
        &mut self,
        partial_counts: impl IntoIterator<Item = Option<i64>>,
    ) -> Result<(), AggError> {
        for count in partial_counts.into_iter().flatten() {
            self.count = self.count.wrapping_add(count);
        }
        Ok(())
    }

    /// 合并另一 COUNT 累加器的部分结果。
    pub fn merge(&mut self, source: &Self) -> Result<(), AggError> {
        self.update_partial([Some(source.count)])
    }

    /// 滑动窗口：对移出的非空行减一，再对移入行做 `update`。
    pub fn slide<T, U>(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<T>>,
        incoming: impl IntoIterator<Item = Option<U>>,
    ) -> Result<(), AggError> {
        for value in outgoing {
            if value.is_some() {
                self.count = self.count.wrapping_sub(1);
            }
        }
        self.update(incoming)
    }
}
