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

// AVG（平均值）聚合函数实现。
//
// 在聚合执行器中，AVG 通常拆成 SUM 与 COUNT：最终结果为 `sum / count`。
// 本模块提供浮点与 Decimal（定点数）两条路径，以及 DISTINCT 去重变体；
// partial result（部分聚合结果）用于 HashAgg 两阶段合并：先本地累加，再跨分区 merge。
// `slide` 支持滑动窗口：移出旧行、移入新行而不必全量重算。

use crate::aggfuncs::AggError;
use crate::func_sum::{Decimal, DistinctDecimalSum, DistinctFloatSum};
use std::mem::size_of;

/// Decimal AVG 部分结果的固定内存占用（字节），供内存追踪。
pub const DEF_PARTIAL_RESULT_4_AVG_DECIMAL_SIZE: i64 = size_of::<DecimalAvg>() as i64;
/// Float64 AVG 部分结果的固定内存占用（字节），供内存追踪。
pub const DEF_PARTIAL_RESULT_4_AVG_FLOAT64_SIZE: i64 = size_of::<FloatAvg>() as i64;

/// 浮点 AVG 累加器：显式维护 Go partial result 的 sum 与 count。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatAvg {
    sum: f64,
    count: i64,
}

impl FloatAvg {
    /// 清空累加状态，准备下一组聚合。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// 用原始行值更新（NULL 由 Option::None 表示并跳过）。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        self.update_standard(values);
    }
    /// 标准精度路径更新，对齐 Go 的 EvalReal 累加语义。
    pub fn update_standard(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        for value in values.into_iter().flatten() {
            self.add_partial(value, 1);
        }
    }
    /// 合并另一段部分结果（HashAgg 第二阶段）。
    pub fn merge(&mut self, source: &Self) {
        let (count, sum) = source.partial_result();
        self.add_partial(sum, count);
    }
    /// 从 `(count, sum)` 部分结果对批量更新。
    ///
    /// partial result 表示已聚合的行数与总和，而非单行求值结果。
    pub fn update_partial(&mut self, partials: impl IntoIterator<Item = Option<(i64, f64)>>) {
        for (count, sum) in partials.into_iter().flatten() {
            self.add_partial(sum, count);
        }
    }
    /// 滑动窗口更新：撤销 outgoing 行并累加 incoming 行。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<f64>>,
        incoming: impl IntoIterator<Item = Option<f64>>,
    ) {
        // Go 先加入新窗口尾部，再移除旧窗口头部；浮点顺序是可观察语义。
        for value in incoming.into_iter().flatten() {
            self.add_partial(value, 1);
        }
        for value in outgoing.into_iter().flatten() {
            self.add_partial(-value, -1);
        }
    }
    /// 最终 AVG = sum / count；无有效行时返回 None。
    pub fn result(&self) -> Option<f64> {
        (self.count != 0).then_some(self.sum / self.count as f64)
    }
    /// 导出部分结果 `(count, sum)`，供序列化或跨阶段传递。
    pub fn partial_result(&self) -> (i64, f64) {
        (self.count, self.sum)
    }

    fn add_partial(&mut self, sum: f64, count: i64) {
        self.sum += sum;
        self.count += count;
    }
}

/// Decimal AVG 累加器：高精度定点数路径，对齐 MySQL `NEWDECIMAL`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecimalAvg {
    sum: Decimal,
    count: i64,
}

impl DecimalAvg {
    /// 清空 Decimal 累加状态。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// 用 Decimal 行值更新；溢出等错误经 `AggError` 返回。
    pub fn update(
        &mut self,
        values: impl IntoIterator<Item = Option<Decimal>>,
    ) -> Result<(), AggError> {
        for value in values.into_iter().flatten() {
            self.add_partial(value, 1)?;
        }
        Ok(())
    }
    /// 合并另一段 Decimal 部分结果。
    pub fn merge(&mut self, source: &Self) -> Result<(), AggError> {
        let (count, sum) = source.partial_result();
        self.add_partial(sum, count)
    }
    /// 从 `(count, Decimal sum)` 部分结果对更新。
    pub fn update_partial(
        &mut self,
        partials: impl IntoIterator<Item = Option<(i64, Decimal)>>,
    ) -> Result<(), AggError> {
        for (count, sum) in partials.into_iter().flatten() {
            self.add_partial(sum, count)?;
        }
        Ok(())
    }
    /// Decimal 滑动窗口更新。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<Decimal>>,
        incoming: impl IntoIterator<Item = Option<Decimal>>,
    ) -> Result<(), AggError> {
        for value in incoming.into_iter().flatten() {
            self.add_partial(value, 1)?;
        }
        for value in outgoing.into_iter().flatten() {
            self.sum = self.sum.checked_sub(value)?;
            self.count -= 1;
        }
        Ok(())
    }
    /// 按 `result_scale`（结果小数位）做 checked 除法得到 AVG。
    pub fn result(&self, result_scale: u32) -> Result<Option<Decimal>, AggError> {
        (self.count != 0)
            .then(|| self.sum.checked_div_i64(self.count, result_scale))
            .transpose()
    }
    /// 导出 `(count, Decimal sum)` 部分结果。
    pub fn partial_result(&self) -> (i64, Decimal) {
        (self.count, self.sum)
    }

    fn add_partial(&mut self, sum: Decimal, count: i64) -> Result<(), AggError> {
        self.sum = self.sum.checked_add(sum)?;
        self.count += count;
        Ok(())
    }
}

/// DISTINCT 浮点 AVG：先对值去重再求平均。
#[derive(Clone, Debug, Default)]
pub struct DistinctFloatAvg {
    pub(crate) sum: DistinctFloatSum,
}

impl DistinctFloatAvg {
    /// 清空去重集合与累加状态。
    pub fn reset(&mut self) {
        self.sum.reset();
    }
    /// 插入新值并返回内存增量（字节），供 MemTracker 记账。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) -> i64 {
        self.sum.update(values)
    }
    /// 合并另一段 DISTINCT 部分结果，返回内存增量。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.sum.merge(&source.sum)
    }
    /// DISTINCT AVG = 去重后 sum / 去重个数。
    pub fn result(&self) -> Option<f64> {
        self.sum.value().map(|sum| sum / self.sum.len() as f64)
    }
}

/// DISTINCT Decimal AVG 累加器。
#[derive(Clone, Debug, Default)]
pub struct DistinctDecimalAvg {
    pub(crate) sum: DistinctDecimalSum,
}

/// 原始阶段 Decimal AVG（对应 Go Original 求值器）。
pub type AvgOriginal4Decimal = DecimalAvg;
/// 部分聚合阶段 Decimal AVG。
pub type AvgPartial4Decimal = DecimalAvg;
/// 原始阶段 Float64 AVG。
pub type AvgOriginal4Float64 = FloatAvg;
/// 原始阶段高精度 Float64 AVG（实现与普通 Float 共享）。
pub type AvgOriginal4Float64HighPrecision = FloatAvg;
/// 部分聚合阶段 Float64 AVG。
pub type AvgPartial4Float64 = FloatAvg;
/// 原始阶段 DISTINCT Decimal AVG。
pub type AvgOriginal4DistinctDecimal = DistinctDecimalAvg;
/// 部分聚合阶段 DISTINCT Decimal AVG。
pub type AvgPartial4DistinctDecimal = DistinctDecimalAvg;
/// 原始阶段 DISTINCT Float64 AVG。
pub type AvgOriginal4DistinctFloat64 = DistinctFloatAvg;
/// 部分聚合阶段 DISTINCT Float64 AVG。
pub type AvgPartial4DistinctFloat64 = DistinctFloatAvg;

impl DistinctDecimalAvg {
    /// 清空 DISTINCT Decimal 状态。
    pub fn reset(&mut self) {
        self.sum.reset();
    }
    /// 插入 Decimal 并返回内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<Decimal>>) -> i64 {
        self.sum.update(values)
    }
    /// 合并 DISTINCT Decimal 部分结果。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.sum.merge(&source.sum)
    }
    /// 按 `result_scale` 计算 DISTINCT Decimal AVG。
    pub fn result(&self, result_scale: u32) -> Result<Option<Decimal>, AggError> {
        self.sum
            .value()?
            .map(|sum| sum.checked_div_i64(self.sum.len() as i64, result_scale))
            .transpose()
    }
}
