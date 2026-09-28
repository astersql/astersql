// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 整数 SUM 聚合：有符号 / 无符号 BIGINT 与 DISTINCT 变体。
//
// 对应 MySQL `SUM` 在 `BIGINT` / `BIGINT UNSIGNED` 上的累加路径；
// 使用 `checked_add` / `checked_sub` 检测溢出并返回 `AggError`。
// `slide` 先移出旧窗口值再加入新值，避免中间临时溢出。
// DISTINCT 用 HashSet 去重后再求和，并回报 capacity 增长的内存增量。

use crate::aggfuncs::{AggError, PartialResult4SumInt64, PartialResult4SumUint64};
use std::collections::HashSet;
use std::mem::size_of;

/// 有符号整数 SUM 部分结果的固定内存占用（字节）。
pub const DEF_PARTIAL_RESULT_4_SUM_INT64_SIZE: i64 = size_of::<PartialResult4SumInt64>() as i64;
/// DISTINCT 有符号整数 SUM 部分结果的固定内存占用（字节）。
pub const DEF_PARTIAL_RESULT_4_SUM_DISTINCT_INT64_SIZE: i64 = size_of::<SumDistinctInt64>() as i64;
/// 无符号整数 SUM 部分结果的固定内存占用（字节）。
pub const DEF_PARTIAL_RESULT_4_SUM_UINT64_SIZE: i64 = size_of::<PartialResult4SumUint64>() as i64;
/// DISTINCT 无符号整数 SUM 部分结果的固定内存占用（字节）。
pub const DEF_PARTIAL_RESULT_4_SUM_DISTINCT_UINT64_SIZE: i64 =
    size_of::<SumDistinctUint64>() as i64;

/// 有符号 BIGINT SUM 累加器，内部持有 `PartialResult4SumInt64`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SumInt {
    state: PartialResult4SumInt64,
}

impl SumInt {
    /// 清空累加状态。
    pub fn reset(&mut self) {
        self.state = PartialResult4SumInt64::default();
    }
    /// 返回当前和；无非空行时为 None。
    pub fn value(&self) -> Option<i64> {
        (self.state.not_null_row_count != 0).then_some(self.state.value)
    }
    /// 已累加的非 NULL 行数。
    pub fn count(&self) -> i64 {
        self.state.not_null_row_count
    }

    /// 用有符号整型行值更新；NULL 跳过，溢出返回错误。
    pub fn update(
        &mut self,
        values: impl IntoIterator<Item = Option<i64>>,
    ) -> Result<(), AggError> {
        for value in values.into_iter().flatten() {
            self.add(value)?;
        }
        Ok(())
    }

    /// 累加单个非空值；首行直接赋值。
    fn add(&mut self, value: i64) -> Result<(), AggError> {
        if self.state.not_null_row_count == 0 {
            self.state.value = value;
            self.state.not_null_row_count = 1;
            return Ok(());
        }
        self.state.value = self
            .state
            .value
            .checked_add(value)
            .ok_or_else(|| AggError("BIGINT value is out of range in SUM".to_owned()))?;
        self.state.not_null_row_count += 1;
        Ok(())
    }

    /// 合并另一段有符号 SUM 部分结果。
    pub fn merge(&mut self, source: &Self) -> Result<(), AggError> {
        if source.state.not_null_row_count == 0 {
            return Ok(());
        }
        if self.state.not_null_row_count == 0 {
            self.state = source.state.clone();
            return Ok(());
        }
        self.state.value = self
            .state
            .value
            .checked_add(source.state.value)
            .ok_or_else(|| AggError("BIGINT value is out of range in SUM merge".to_owned()))?;
        self.state.not_null_row_count += source.state.not_null_row_count;
        Ok(())
    }

    /// 滑动窗口：先 `checked_sub` 移出 outgoing，再 `update` incoming。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<i64>>,
        incoming: impl IntoIterator<Item = Option<i64>>,
    ) -> Result<(), AggError> {
        for value in outgoing.into_iter().flatten() {
            self.state.value =
                self.state.value.checked_sub(value).ok_or_else(|| {
                    AggError("BIGINT value is out of range in SUM slide".to_owned())
                })?;
            self.state.not_null_row_count -= 1;
        }
        self.update(incoming)
    }
}

/// 无符号 BIGINT SUM 累加器。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SumUint {
    state: PartialResult4SumUint64,
}

impl SumUint {
    /// 清空累加状态。
    pub fn reset(&mut self) {
        self.state = PartialResult4SumUint64::default();
    }
    /// 返回当前无符号和；无非空行时为 None。
    pub fn value(&self) -> Option<u64> {
        (self.state.not_null_row_count != 0).then_some(self.state.value)
    }
    /// 已累加的非 NULL 行数。
    pub fn count(&self) -> i64 {
        self.state.not_null_row_count
    }
    /// 用无符号整型行值更新。
    pub fn update(
        &mut self,
        values: impl IntoIterator<Item = Option<u64>>,
    ) -> Result<(), AggError> {
        for value in values.into_iter().flatten() {
            if self.state.not_null_row_count == 0 {
                self.state.value = value;
            } else {
                self.state.value = self.state.value.checked_add(value).ok_or_else(|| {
                    AggError("BIGINT UNSIGNED value is out of range in SUM".to_owned())
                })?;
            }
            self.state.not_null_row_count += 1;
        }
        Ok(())
    }
    /// 合并另一段无符号 SUM 部分结果。
    pub fn merge(&mut self, source: &Self) -> Result<(), AggError> {
        if source.state.not_null_row_count == 0 {
            return Ok(());
        }
        if self.state.not_null_row_count == 0 {
            self.state = source.state.clone();
            return Ok(());
        }
        self.state.value = self
            .state
            .value
            .checked_add(source.state.value)
            .ok_or_else(|| {
                AggError("BIGINT UNSIGNED value is out of range in SUM merge".to_owned())
            })?;
        self.state.not_null_row_count += source.state.not_null_row_count;
        Ok(())
    }
    /// 滑动窗口：先减后加，避免 `maxUint64-1 + 2` 式中间溢出。
    pub fn slide(
        &mut self,
        outgoing: impl IntoIterator<Item = Option<u64>>,
        incoming: impl IntoIterator<Item = Option<u64>>,
    ) -> Result<(), AggError> {
        for value in outgoing.into_iter().flatten() {
            self.state.value = self.state.value.checked_sub(value).ok_or_else(|| {
                AggError("BIGINT UNSIGNED value is out of range in SUM slide".to_owned())
            })?;
            self.state.not_null_row_count -= 1;
        }
        self.update(incoming)
    }
}

/// DISTINCT 有符号整数 SUM：HashSet 去重后再求和。
#[derive(Clone, Debug, Default)]
pub struct SumDistinctInt64 {
    values: HashSet<i64>,
}

impl SumDistinctInt64 {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.values = HashSet::new();
    }
    /// 插入去重值；返回 capacity 增长的内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<i64>>) -> i64 {
        let old = self.values.capacity();
        self.values.extend(values.into_iter().flatten());
        ((self.values.capacity() - old) * size_of::<i64>()) as i64
    }
    /// 合并另一段 DISTINCT 集合。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.update(source.values.iter().copied().map(Some))
    }
    /// 对去重后的 i64 做检查求和；空集返回 Ok(None)。
    pub fn value(&self) -> Result<Option<i64>, AggError> {
        if self.values.is_empty() {
            return Ok(None);
        }
        self.values
            .iter()
            .try_fold(0_i64, |sum, value| {
                sum.checked_add(*value).ok_or_else(|| {
                    AggError("BIGINT value is out of range in DISTINCT SUM".to_owned())
                })
            })
            .map(Some)
    }
}

/// DISTINCT 无符号整数 SUM：以 i64 bit pattern 存入 HashSet 去重。
#[derive(Clone, Debug, Default)]
pub struct SumDistinctUint64 {
    bit_patterns: HashSet<i64>,
}

/// 原始阶段有符号 SUM 类型别名。
pub type SumIntOriginal = SumInt;
/// 原始阶段无符号 SUM 类型别名。
pub type SumUintOriginal = SumUint;
/// 原始阶段 DISTINCT 有符号 SUM 类型别名。
pub type SumDistinctInt64Original = SumDistinctInt64;
/// 原始阶段 DISTINCT 无符号 SUM 类型别名。
pub type SumDistinctUint64Original = SumDistinctUint64;

impl SumDistinctUint64 {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.bit_patterns = HashSet::new();
    }
    /// 将 u64 按 bit 转为 i64 存入集合；返回内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<u64>>) -> i64 {
        let old = self.bit_patterns.capacity();
        self.bit_patterns
            .extend(values.into_iter().flatten().map(|value| value as i64));
        ((self.bit_patterns.capacity() - old) * size_of::<i64>()) as i64
    }
    /// 合并另一段 DISTINCT 无符号集合。
    pub fn merge(&mut self, source: &Self) -> i64 {
        self.update(source.bit_patterns.iter().map(|value| Some(*value as u64)))
    }
    /// 对去重后的 bit pattern 按 u64 求和。
    pub fn value(&self) -> Result<Option<u64>, AggError> {
        if self.bit_patterns.is_empty() {
            return Ok(None);
        }
        self.bit_patterns
            .iter()
            .try_fold(0_u64, |sum, value| {
                sum.checked_add(*value as u64).ok_or_else(|| {
                    AggError("BIGINT UNSIGNED value is out of range in DISTINCT SUM".to_owned())
                })
            })
            .map(Some)
    }
}
