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

// CUME_DIST（累积分布）窗口函数实现。
//
// 窗口函数在已按 ORDER BY 排序的分区上，对当前行计算：
// `（小于等于当前行值的行数）/ 分区总行数`。
// 实现先缓存分区全部行，再按 peer group（相等值组）推进 `last_rank`，
// 同组内各行共享同一累积分布值。

use crate::func_rank::DEF_ROW_SIZE;
use std::cmp::Ordering;
use std::mem::size_of;

/// `CumeDist` 空状态的固定体积，对应 Go `DefPartialResult4CumeDistSize`。
pub const DEF_PARTIAL_RESULT_CUME_DIST_SIZE: i64 = size_of::<CumeDist<()>>() as i64;

/// CUME_DIST 窗口状态：缓存行序列并跟踪当前下标与 peer 组末秩。
#[derive(Clone, Debug, PartialEq)]
pub struct CumeDist<T> {
    cur_idx: usize,
    last_rank: usize,
    rows: Vec<T>,
}

impl<T> Default for CumeDist<T> {
    fn default() -> Self {
        Self {
            cur_idx: 0,
            last_rank: 0,
            rows: Vec::new(),
        }
    }
}

impl<T> CumeDist<T> {
    /// 清空索引与行缓存，准备下一窗口分区。
    pub fn reset(&mut self) {
        self.cur_idx = 0;
        self.last_rank = 0;
        self.rows.clear();
    }

    /// 追加分区行；返回新增行的估算内存增量（行数 × `DEF_ROW_SIZE`）。
    pub fn update(&mut self, rows: impl IntoIterator<Item = T>) -> i64 {
        let old_len = self.rows.len();
        self.rows.extend(rows);
        (self.rows.len() - old_len) as i64 * DEF_ROW_SIZE
    }

    /// 用自定义比较器推进一行，返回该行的 CUME_DIST；耗尽返回 None。
    pub fn next_by(&mut self, compare: impl Fn(&T, &T) -> Ordering) -> Option<f64> {
        let row = self.rows.get(self.cur_idx)?;
        // 扩展 last_rank 直到遇到与当前行不相等的位置，得到 peer 组末尾秩。
        while self.last_rank < self.rows.len()
            && compare(row, &self.rows[self.last_rank]) == Ordering::Equal
        {
            self.last_rank += 1;
        }
        self.cur_idx += 1;
        Some(self.last_rank as f64 / self.rows.len() as f64)
    }
}

impl<T: PartialEq> CumeDist<T> {
    /// 默认相等比较路径：相等视为同一 peer 组，不等视为 Less（仅用于分组边界）。
    pub fn next(&mut self) -> Option<f64> {
        self.next_by(|left, right| {
            if left == right {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        })
    }
}
