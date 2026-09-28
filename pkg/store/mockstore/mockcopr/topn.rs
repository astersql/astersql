// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// TopN 堆与排序辅助：在协处理器侧维护「按 ORDER BY 最优的 N 行」。
//
// TopN 等价于 ORDER BY + LIMIT：用大小受限的堆只保留当前最优的 `totalCount` 行，
// 最后再按排序键输出有序结果，避免全量排序。

use std::cmp::Ordering;

use crate::copr_handler::{ByItem, CopError, Datum, Row};

/// 带排序键的行：`key` 为 ORDER BY 表达式求值结果，`data` 为原始行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct sortRow {
    pub key: Vec<Datum>,
    pub data: Row,
}

/// 按 `orderByItems` 比较多行的排序器，并缓存可能的比较错误。
#[derive(Clone, Debug)]
pub struct topNSorter {
    pub orderByItems: Vec<ByItem>,
    pub rows: Vec<sortRow>,
    pub err: Option<CopError>,
}

impl topNSorter {
    /// 用 ORDER BY 项列表构造空排序器。
    pub fn new(order_by: Vec<ByItem>) -> Self {
        Self {
            orderByItems: order_by,
            rows: Vec::new(),
            err: None,
        }
    }
    /// 当前缓冲行数。
    pub fn Len(&self) -> usize {
        self.rows.len()
    }
    /// 交换两行（堆/排序接口）。
    pub fn Swap(&mut self, left: usize, right: usize) {
        self.rows.swap(left, right);
    }
    /// 是否 `rows[left] < rows[right]`（升序语义）。
    pub fn Less(&self, left: usize, right: usize) -> bool {
        self.compare(&self.rows[left], &self.rows[right]) == Ordering::Less
    }

    /// 按 ORDER BY 项逐列比较两行。
    fn compare(&self, left: &sortRow, right: &sortRow) -> Ordering {
        compare_rows(&self.orderByItems, left, right)
    }

    /// 对缓冲行做稳定排序，准备输出。
    fn sort(&mut self) {
        let order_by = self.orderByItems.clone();
        self.rows
            .sort_by(|left, right| compare_rows(&order_by, left, right));
    }
}

/// 按多列 ORDER BY（支持降序）比较两行的排序键。
fn compare_rows(order_by: &[ByItem], left: &sortRow, right: &sortRow) -> Ordering {
    for (index, item) in order_by.iter().enumerate() {
        let mut ordering = left.key.get(index).cmp(&right.key.get(index));
        // 降序时反转比较结果。
        if item.descending {
            ordering = ordering.reverse();
        }
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// TopN 最大堆（根为当前最差行）：容量为 `totalCount`。
pub struct topNHeap {
    pub topNSorter: topNSorter,
    pub totalCount: usize,
    pub heapSize: usize,
}

impl topNHeap {
    /// 创建容量为 `total_count`、排序键为 `order_by` 的空堆。
    pub fn new(total_count: usize, order_by: Vec<ByItem>) -> Self {
        Self {
            topNSorter: topNSorter::new(order_by),
            totalCount: total_count,
            heapSize: 0,
        }
    }

    /// 当前堆中元素个数。
    pub fn Len(&self) -> usize {
        self.heapSize
    }
    /// 将行压入堆并上浮维护堆序。
    pub fn Push(&mut self, row: sortRow) {
        self.topNSorter.rows.push(row);
        self.heapSize += 1;
        self.sift_up(self.heapSize - 1);
    }
    /// 弹出接口占位（当前实现返回 None，与迁移基线一致）。
    pub fn Pop(&mut self) -> Option<sortRow> {
        None
    }

    /// Reversed comparison makes index zero the worst currently admitted row.
    /// 反向比较：下标 0 为当前堆中最差（即将被替换）的行。
    pub fn Less(&self, left: usize, right: usize) -> bool {
        self.topNSorter
            .compare(&self.topNSorter.rows[left], &self.topNSorter.rows[right])
            == Ordering::Greater
    }

    /// 尝试接纳一行：堆未满则 Push；已满则仅当新行优于根时替换并下沉。
    pub fn tryToAddRow(&mut self, row: sortRow) -> bool {
        if self.totalCount == 0 {
            return false;
        }
        if self.heapSize < self.totalCount {
            self.Push(row);
            return true;
        }
        // 新行优于当前最差行时替换堆顶。
        if self.topNSorter.compare(&row, &self.topNSorter.rows[0]) == Ordering::Less {
            self.topNSorter.rows[0] = row;
            self.sift_down(0);
            true
        } else {
            false
        }
    }

    /// 取出错误（若有），否则排序后返回行数据向量。
    pub fn intoSortedRows(mut self) -> Result<Vec<Row>, CopError> {
        if let Some(error) = self.topNSorter.err.take() {
            return Err(error);
        }
        self.topNSorter.sort();
        Ok(self
            .topNSorter
            .rows
            .into_iter()
            .map(|row| row.data)
            .collect())
    }

    /// 向上调整堆，使子节点不优于父节点。
    fn sift_up(&mut self, mut child: usize) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.Less(child, parent) {
                break;
            }
            self.topNSorter.rows.swap(child, parent);
            child = parent;
        }
    }

    /// 向下调整堆，使父节点不劣于子节点。
    fn sift_down(&mut self, mut parent: usize) {
        loop {
            let left = parent * 2 + 1;
            if left >= self.heapSize {
                break;
            }
            let right = left + 1;
            let child = if right < self.heapSize && self.Less(right, left) {
                right
            } else {
                left
            };
            if !self.Less(child, parent) {
                break;
            }
            self.topNSorter.rows.swap(child, parent);
            parent = child;
        }
    }
}
