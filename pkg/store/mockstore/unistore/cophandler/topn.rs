// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// Coprocessor TopN（取前 N 行）排序与有界堆实现。
//
// TopN 在下推执行计划中按 ORDER BY 键比较行，只保留最优的 `limit` 行。
// `TopNSorter` 负责多键升/降序比较与最终排序；`TopNHeap` 是有界最大堆
// （堆顶为当前最差行），对应 Go `topNHeap` 的反向 `Less` 语义。

use crate::cop_handler::{ByItem, CopError, Datum, Row};
use std::cmp::Ordering;

/// 参与排序的一行：排序键与原始数据行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortRow {
    /// 按 ORDER BY 表达式求值得到的排序键序列。
    pub key: Vec<Datum>,
    /// 原始数据行（投影后的 Datum 列表）。
    pub data: Row,
}

/// 按 `ByItem`（排序项，含升降序）对 `SortRow` 进行比较与排序。
#[derive(Clone, Debug)]
pub struct TopNSorter {
    /// ORDER BY 项列表。
    pub order_by_items: Vec<ByItem>,
    /// 当前收集的排序行。
    pub rows: Vec<SortRow>,
    /// 堆/排序过程中累积的错误（若有）。
    pub error: Option<CopError>,
}

impl TopNSorter {
    /// 用给定 ORDER BY 项构造空排序器。
    pub fn new(order_by_items: Vec<ByItem>) -> Self {
        Self {
            order_by_items,
            rows: Vec::new(),
            error: None,
        }
    }

    /// 当前行数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// 交换两行位置（供堆调整使用）。
    pub fn swap(&mut self, left: usize, right: usize) {
        self.rows.swap(left, right);
    }

    /// Ordering used by Go's `sort.Interface.Less` after heap collection.
    /// 多键比较：按 `order_by_items` 依次比较，降序项对结果取反。
    pub fn compare(&self, left: &SortRow, right: &SortRow) -> Ordering {
        compare_with(&self.order_by_items, left, right)
    }

    /// 按 ORDER BY 对 `rows` 做最终稳定排序。
    pub fn sort(&mut self) {
        let order_by = self.order_by_items.clone();
        self.rows
            .sort_by(|left, right| compare_with(&order_by, left, right));
    }
}

/// 独立比较函数，供 `sort_by` 闭包使用（避免借用 `self`）。
fn compare_with(order_by: &[ByItem], left: &SortRow, right: &SortRow) -> Ordering {
    for (position, by) in order_by.iter().enumerate() {
        let mut ordering = match (left.key.get(position), right.key.get(position)) {
            (Some(left), Some(right)) if by.enum_unsigned => {
                enum_unsigned_value(left).cmp(&enum_unsigned_value(right))
            }
            (left, right) => left.cmp(&right),
        };
        if by.descending {
            ordering = ordering.reverse();
        }
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Mirror Go's enum-only `Datum.GetUint64` comparison. Enum expressions produce
/// unsigned datums; accepting signed numeric values preserves their underlying
/// two's-complement value when callers construct simplified rows directly.
fn enum_unsigned_value(value: &Datum) -> u64 {
    match value {
        Datum::Uint(value) => *value,
        Datum::Int(value) => *value as u64,
        Datum::Real(value) => value.to_bits(),
        Datum::Null | Datum::Bytes(_) => 0,
    }
}

/// A bounded max-heap: index zero is the worst row currently admitted, exactly
/// matching the reversed `Less` in the Go topNHeap.
/// 有界最大堆：下标 0 为当前最差行，与 Go `topNHeap` 的反向 `Less` 一致。
#[derive(Clone, Debug)]
pub struct TopNHeap {
    /// 内嵌排序器，持有 ORDER BY 与行缓冲。
    pub sorter: TopNSorter,
    /// 目标保留行数（TopN 的 N）。
    pub total_count: usize,
    /// 当前堆中有效元素个数。
    pub heap_size: usize,
}

impl TopNHeap {
    /// 构造容量为 `total_count` 的 TopN 堆。
    pub fn new(total_count: usize, order_by_items: Vec<ByItem>) -> Self {
        Self {
            sorter: TopNSorter::new(order_by_items),
            total_count,
            heap_size: 0,
        }
    }

    /// 当前堆大小。
    pub fn len(&self) -> usize {
        self.heap_size
    }
    /// 堆是否为空。
    pub fn is_empty(&self) -> bool {
        self.heap_size == 0
    }

    /// 尝试将行加入堆：未满则上滤；已满则仅当新行优于堆顶时替换并下滤。
    pub fn try_to_add_row(&mut self, row: SortRow) -> bool {
        if self.total_count == 0 {
            return false;
        }
        if self.heap_size < self.total_count {
            self.sorter.rows.push(row);
            self.heap_size += 1;
            self.sift_up(self.heap_size - 1);
            return true;
        }
        // 堆已满：仅接纳比堆顶（最差行）更优的候选。
        if self.sorter.compare(&row, &self.sorter.rows[0]) == Ordering::Less {
            self.sorter.rows[0] = row;
            self.sift_down(0);
            true
        } else {
            false
        }
    }

    /// 对数据行求 ORDER BY 键后尝试入堆。
    pub fn add_data_row(&mut self, row: Row) -> Result<bool, CopError> {
        let key = self
            .sorter
            .order_by_items
            .iter()
            .map(|by| by.expr.eval(&row))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.try_to_add_row(SortRow { key, data: row }))
    }

    /// 消费堆：若有错误则返回；否则排序后抽出原始数据行。
    pub fn into_sorted_rows(mut self) -> Result<Vec<Row>, CopError> {
        if let Some(error) = self.sorter.error.take() {
            return Err(error);
        }
        self.sorter.sort();
        Ok(self.sorter.rows.into_iter().map(|row| row.data).collect())
    }

    /// 判断 left 是否比 right「更差」（在最大堆语义下应更靠近堆顶）。
    fn worse(&self, left: usize, right: usize) -> bool {
        self.sorter
            .compare(&self.sorter.rows[left], &self.sorter.rows[right])
            == Ordering::Greater
    }

    /// 上滤：将子节点沿父链上浮，直到不再比父更差。
    fn sift_up(&mut self, mut child: usize) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.worse(child, parent) {
                break;
            }
            self.sorter.rows.swap(child, parent);
            child = parent;
        }
    }

    /// 下滤：将父节点与更差的子节点交换，直到堆性质恢复。
    fn sift_down(&mut self, mut parent: usize) {
        loop {
            let left = parent * 2 + 1;
            if left >= self.heap_size {
                break;
            }
            let right = left + 1;
            // 在左右子中选更差者作为交换候选。
            let child = if right < self.heap_size && self.worse(right, left) {
                right
            } else {
                left
            };
            if !self.worse(child, parent) {
                break;
            }
            self.sorter.rows.swap(child, parent);
            parent = child;
        }
    }
}
