// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// LEAD / LAG 窗口函数状态机：按 offset 向前/向后取同行分区内的值。
//
// 窗口函数（window function）在分区排序后的行序列上滑动求值。
// LEAD(expr, offset, default) 取当前位置之后 offset 行的 expr；越界则用 default。
// LAG 则取之前 offset 行。default 在当前行求值，对齐 TiDB 的求值时机。

use crate::func_rank::DEF_ROW_SIZE;

/// An input row after evaluating LEAD/LAG's value and default expressions.
/// The default belongs to the current row, matching TiDB's evaluation point.
///
/// 已求值的一行输入：`value` 为 LEAD/LAG 目标表达式，`default` 属于当前行。
#[derive(Clone, Debug, PartialEq)]
pub struct LeadLagRow<T> {
    /// 窗口值表达式在该行的求值结果（可为 SQL NULL）。
    pub value: Option<T>,
    /// 越界时回退的默认值（在当前行求值）。
    pub default: Option<T>,
}

impl<T> LeadLagRow<T> {
    /// 构造一行已求值的 LEAD/LAG 输入。
    pub fn new(value: Option<T>, default: Option<T>) -> Self {
        Self { value, default }
    }
}

/// LEAD/LAG 共享的缓冲状态：offset、行缓冲与当前游标。
#[derive(Clone, Debug, PartialEq)]
pub struct LeadLagState<T> {
    /// 向前/向后偏移的行数。
    offset: u64,
    /// 分区内已收集的行。
    rows: Vec<LeadLagRow<T>>,
    /// 下一次 next_value 将消费的行下标。
    cur_idx: usize,
}

impl<T> LeadLagState<T> {
    /// 以给定 offset 创建空缓冲状态。
    pub fn new(offset: u64) -> Self {
        Self {
            offset,
            rows: Vec::new(),
            cur_idx: 0,
        }
    }

    /// 清空行缓冲并重置游标（不改变 offset）。
    pub fn reset(&mut self) {
        self.rows.clear();
        self.cur_idx = 0;
    }

    /// 追加若干行，返回按 DEF_ROW_SIZE 估算的内存增量（字节）。
    pub fn update(&mut self, rows: impl IntoIterator<Item = LeadLagRow<T>>) -> i64 {
        let old_len = self.rows.len();
        self.rows.extend(rows);
        (self.rows.len() - old_len) as i64 * DEF_ROW_SIZE
    }

    /// 返回当前行缓冲切片。
    pub fn rows(&self) -> &[LeadLagRow<T>] {
        &self.rows
    }
}

/// LEAD 窗口函数：取当前位置 + offset 处的 value，越界用当前行 default。
#[derive(Clone, Debug, PartialEq)]
pub struct Lead<T>(LeadLagState<T>);

impl<T: Clone> Lead<T> {
    /// 构造指定 offset 的 LEAD 状态。
    pub fn new(offset: u64) -> Self {
        Self(LeadLagState::new(offset))
    }

    /// 委托内部状态 reset。
    pub fn reset(&mut self) {
        self.0.reset();
    }

    /// 委托内部状态 update，返回内存增量。
    pub fn update(&mut self, rows: impl IntoIterator<Item = LeadLagRow<T>>) -> i64 {
        self.0.update(rows)
    }

    /// 产出当前游标行的 LEAD 结果，并将游标前移一格。
    ///
    /// 外层 `None` 表示已无更多当前行；内层 `Option<T>` 为 SQL NULL 或具体值。
    pub fn next_value(&mut self) -> Option<Option<T>> {
        let current = self.0.rows.get(self.0.cur_idx)?;
        // 目标下标 = cur_idx + offset；溢出或越界则退回 current.default。
        let target = usize::try_from(self.0.offset)
            .ok()
            .and_then(|offset| self.0.cur_idx.checked_add(offset))
            .and_then(|index| self.0.rows.get(index));
        let result = target
            .map(|row| row.value.clone())
            .unwrap_or_else(|| current.default.clone());
        self.0.cur_idx += 1;
        Some(result)
    }
}

/// LAG 窗口函数：取当前位置 - offset 处的 value，越界用当前行 default。
#[derive(Clone, Debug, PartialEq)]
pub struct Lag<T>(LeadLagState<T>);

impl<T: Clone> Lag<T> {
    /// 构造指定 offset 的 LAG 状态。
    pub fn new(offset: u64) -> Self {
        Self(LeadLagState::new(offset))
    }

    /// 委托内部状态 reset。
    pub fn reset(&mut self) {
        self.0.reset();
    }

    /// 委托内部状态 update，返回内存增量。
    pub fn update(&mut self, rows: impl IntoIterator<Item = LeadLagRow<T>>) -> i64 {
        self.0.update(rows)
    }

    /// 产出当前游标行的 LAG 结果，并将游标前移一格。
    pub fn next_value(&mut self) -> Option<Option<T>> {
        let current = self.0.rows.get(self.0.cur_idx)?;
        // 目标下标 = cur_idx - offset；下溢或越界则退回 current.default。
        let target = usize::try_from(self.0.offset)
            .ok()
            .and_then(|offset| self.0.cur_idx.checked_sub(offset))
            .and_then(|index| self.0.rows.get(index));
        let result = target
            .map(|row| row.value.clone())
            .unwrap_or_else(|| current.default.clone());
        self.0.cur_idx += 1;
        Some(result)
    }
}
