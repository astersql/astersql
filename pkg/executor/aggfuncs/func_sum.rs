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

// SUM（求和）聚合函数：浮点、Decimal 与 DISTINCT 变体。
//
// 聚合执行器中 SUM 对各分组非 NULL 值累加；partial result（部分聚合结果）
// 用于 HashAgg 两阶段：本地累加后再 merge。`slide` 支持滑动窗口增量更新。
// 浮点路径保持 Go 的顺序累加语义；Decimal 为定点数精确路径。

use crate::aggfuncs::AggError;
use std::mem::size_of;

/// 简化版 Decimal：系数 `coefficient` 与小数位 `scale`（定点数表示）。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Decimal {
    coefficient: i128,
    scale: u32,
}

impl Decimal {
    /// 由系数与小数位构造 Decimal。
    pub const fn new(coefficient: i128, scale: u32) -> Self {
        Self { coefficient, scale }
    }
    /// 返回定点系数。
    pub const fn coefficient(self) -> i128 {
        self.coefficient
    }
    /// 返回小数位数。
    pub const fn scale(self) -> u32 {
        self.scale
    }

    /// 对齐 Go `MyDecimal::ToHashKey`：去掉小数部分尾随零后比较数值键。
    fn normalized_key(mut self) -> (i128, u32) {
        if self.coefficient == 0 {
            return (0, 0);
        }
        while self.scale > 0 && self.coefficient % 10 == 0 {
            self.coefficient /= 10;
            self.scale -= 1;
        }
        (self.coefficient, self.scale)
    }

    /// 将系数对齐到目标 scale；缩小 scale 不允许（会损失精度）。
    fn checked_rescale(self, scale: u32) -> Result<i128, AggError> {
        if scale < self.scale {
            return Err(AggError(
                "decimal scale cannot be reduced exactly".to_owned(),
            ));
        }
        let factor = 10_i128
            .checked_pow(scale - self.scale)
            .ok_or_else(|| AggError("DECIMAL scale overflow".to_owned()))?;
        self.coefficient
            .checked_mul(factor)
            .ok_or_else(|| AggError("DECIMAL value is out of range".to_owned()))
    }

    /// 检查加法：对齐 scale 后相加，溢出返回 `AggError`。
    pub fn checked_add(self, other: Self) -> Result<Self, AggError> {
        let scale = self.scale.max(other.scale);
        let coefficient = self
            .checked_rescale(scale)?
            .checked_add(other.checked_rescale(scale)?)
            .ok_or_else(|| AggError("DECIMAL value is out of range in SUM".to_owned()))?;
        Ok(Self { coefficient, scale })
    }

    /// 检查减法：用于滑动窗口移出旧值。
    pub fn checked_sub(self, other: Self) -> Result<Self, AggError> {
        let scale = self.scale.max(other.scale);
        let coefficient = self
            .checked_rescale(scale)?
            .checked_sub(other.checked_rescale(scale)?)
            .ok_or_else(|| AggError("DECIMAL value is out of range in SUM slide".to_owned()))?;
        Ok(Self { coefficient, scale })
    }

    /// 除以 i64，结果 scale 取 `result_scale` 与自身的较大者。
    pub fn checked_div_i64(self, divisor: i64, result_scale: u32) -> Result<Self, AggError> {
        if divisor == 0 {
            return Err(AggError("division by zero".to_owned()));
        }
        let scale = result_scale.max(self.scale);
        Ok(Self {
            coefficient: self.checked_rescale(scale)? / divisor as i128,
            scale,
        })
    }
}

/// 浮点 SUM 累加器：维护 sum 与非空行数。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatSum {
    sum: f64,
    count: i64,
}

impl FloatSum {
    /// 清空累加状态。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// 返回最终和；无有效行返回 None。
    pub fn value(&self) -> Option<f64> {
        (self.count != 0).then_some(self.sum)
    }
    /// 已累加的非 NULL 行数。
    pub fn count(&self) -> i64 {
        self.count
    }

    /// 跳过 NULL，按 Go `sum += value` 的输入顺序累加。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        self.update_standard(values);
    }

    /// 标准精度路径：直接 `sum += value`，对齐 Go EvalReal 简单累加。
    pub fn update_standard(&mut self, values: impl IntoIterator<Item = Option<f64>>) {
        for value in values.into_iter().flatten() {
            self.sum += value;
            self.count = self.count.wrapping_add(1);
        }
    }

    /// 合并另一段部分结果（HashAgg 第二阶段）。
    pub fn merge(&mut self, source: &Self) {
        if source.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = *source;
            return;
        }
        self.sum += source.sum;
        self.count = self.count.wrapping_add(source.count);
    }

    /// 滑动窗口：按 Go 顺序先累加 incoming，再减去 outgoing。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<f64>>,
        incoming: impl IntoIterator<Item = Option<f64>>,
    ) {
        self.update(incoming);
        for value in outgoing.into_iter().flatten() {
            self.sum -= value;
            self.count = self.count.wrapping_sub(1);
        }
    }
}

/// Decimal SUM 累加器：精确定点求和。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecimalSum {
    sum: Decimal,
    count: i64,
}

impl DecimalSum {
    /// 清空 Decimal 累加状态。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// 返回当前和；无有效行返回 None。
    pub fn value(&self) -> Option<Decimal> {
        (self.count != 0).then_some(self.sum)
    }
    /// 已累加的非 NULL 行数。
    pub fn count(&self) -> i64 {
        self.count
    }
    /// 用 Decimal 行值更新；溢出经 `AggError` 返回。
    pub fn update(
        &mut self,
        values: impl IntoIterator<Item = Option<Decimal>>,
    ) -> Result<(), AggError> {
        for value in values.into_iter().flatten() {
            // 首个非空值直接赋值，避免与零系数默认值相加。
            self.sum = if self.count == 0 {
                value
            } else {
                self.sum.checked_add(value)?
            };
            self.count = self.count.wrapping_add(1);
        }
        Ok(())
    }
    /// 合并另一段 Decimal 部分结果。
    pub fn merge(&mut self, source: &Self) -> Result<(), AggError> {
        if source.count == 0 {
            return Ok(());
        }
        if self.count == 0 {
            *self = *source;
            return Ok(());
        }
        self.sum = self.sum.checked_add(source.sum)?;
        self.count = self.count.wrapping_add(source.count);
        Ok(())
    }
    /// 以 `(sum, count)` 部分结果对增量更新（供 AVG 等复用）。
    pub(crate) fn add_partial(&mut self, sum: Decimal, count: i64) -> Result<(), AggError> {
        if count == 0 {
            return Ok(());
        }
        if self.count == 0 {
            self.sum = sum;
            self.count = count;
            return Ok(());
        }
        self.sum = self.sum.checked_add(sum)?;
        self.count = self.count.wrapping_add(count);
        Ok(())
    }
    /// 滑动窗口：按 Go 顺序先加入 incoming，再移出 outgoing。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<Decimal>>,
        incoming: impl IntoIterator<Item = Option<Decimal>>,
    ) -> Result<(), AggError> {
        self.update(incoming)?;
        for value in outgoing.into_iter().flatten() {
            self.sum = self.sum.checked_sub(value)?;
            self.count = self.count.wrapping_sub(1);
        }
        Ok(())
    }
}

/// DISTINCT 浮点 SUM：按 Go `map[float64]` 的相等语义去重后求和。
#[derive(Clone, Debug, Default)]
pub struct DistinctFloatSum {
    values: Vec<f64>,
}

impl DistinctFloatSum {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.values.clear();
    }
    /// 插入去重值；返回因 capacity 增长产生的内存增量（字节）。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<f64>>) -> i64 {
        let old = self.values.capacity();
        for value in values.into_iter().flatten() {
            // Go map 合并 +0/-0；NaN 因不等于自身，每次插入都成为独立键。
            if !self.values.iter().any(|existing| *existing == value) {
                self.values.push(value);
            }
        }
        ((self.values.capacity() - old) * size_of::<u64>()) as i64
    }
    /// 合并另一段 DISTINCT 集合，同样返回内存增量。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.update(source.values.iter().copied().map(Some))
    }
    /// 对去重后的浮点值求和；空集返回 None。
    pub fn value(&self) -> Option<f64> {
        if self.values.is_empty() {
            None
        } else {
            Some(self.values.iter().sum())
        }
    }
    /// 去重后的元素个数。
    pub fn len(&self) -> usize {
        self.values.len()
    }
}

/// DISTINCT Decimal SUM：按 Go Decimal 规范化哈希语义去重后再精确求和。
#[derive(Clone, Debug, Default)]
pub struct DistinctDecimalSum {
    values: Vec<Decimal>,
}

/// Float64 SUM 类型别名。
pub type Sum4Float64 = FloatSum;
/// 高精度 Float64 SUM 类型别名（与 `FloatSum` 同结构）。
pub type Sum4Float64HighPrecision = FloatSum;
/// Decimal SUM 类型别名。
pub type Sum4Decimal = DecimalSum;
/// 部分聚合阶段的 DISTINCT Float64 SUM。
pub type Sum4PartialDistinctFloat64 = DistinctFloatSum;
/// 原始阶段的 DISTINCT Float64 SUM。
pub type Sum4OriginalDistinctFloat64 = DistinctFloatSum;
/// 部分聚合阶段的 DISTINCT Decimal SUM。
pub type Sum4PartialDistinctDecimal = DistinctDecimalSum;
/// 原始阶段的 DISTINCT Decimal SUM。
pub type Sum4OriginalDistinctDecimal = DistinctDecimalSum;

impl DistinctDecimalSum {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.values.clear();
    }
    /// 插入去重 Decimal；返回 capacity 增长带来的内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<Decimal>>) -> i64 {
        let old = self.values.capacity();
        for value in values.into_iter().flatten() {
            let key = value.normalized_key();
            if !self
                .values
                .iter()
                .any(|existing| existing.normalized_key() == key)
            {
                self.values.push(value);
            }
        }
        ((self.values.capacity() - old) * size_of::<Decimal>()) as i64
    }
    /// 合并另一段 DISTINCT Decimal 集合。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.update(source.values.iter().copied().map(Some))
    }
    /// 对去重后的 Decimal 做 `checked_add` 求和；空集返回 Ok(None)。
    pub fn value(&self) -> Result<Option<Decimal>, AggError> {
        let mut values = self.values.iter().copied();
        let Some(first) = values.next() else {
            return Ok(None);
        };
        values.try_fold(first, Decimal::checked_add).map(Some)
    }
    /// 去重后的元素个数。
    pub fn len(&self) -> usize {
        self.values.len()
    }
}
