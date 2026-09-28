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

// PERCENT_RANK 窗口函数实现。
//
// 对应 SQL `PERCENT_RANK()`：`(rank - 1) / (partition_rows - 1)`，结果落在 \[0, 1\]。
// 同行组（peer group，ORDER BY 键相等）共享同一 rank，与 `RANK()` 跳号规则一致；
// 分区仅一行时分母为 0，实现用 `saturating_sub` 避免除零（此时首行已返回 0.0）。

use crate::func_rank::{DEF_ROW_SIZE, RankState};
use std::cmp::Ordering;

/// PERCENT_RANK 状态：复用 `RankState` 保存行缓冲、当前下标与最近一次 rank。
#[derive(Clone, Debug, PartialEq)]
pub struct PercentRank<T> {
    state: RankState<T>,
}

impl<T> Default for PercentRank<T> {
    fn default() -> Self {
        Self {
            state: RankState::default(),
        }
    }
}

impl<T> PercentRank<T> {
    /// 清空分区行缓冲与游标，准备下一分区。
    pub fn reset(&mut self) {
        self.state.cur_idx = 0;
        self.state.last_rank = 0;
        self.state.rows.clear();
    }

    /// 追加分区内排序键行，返回估算内存增量（行数 × `DEF_ROW_SIZE`）。
    pub fn update(&mut self, rows: impl IntoIterator<Item = T>) -> i64 {
        let old_len = self.state.rows.len();
        self.state.rows.extend(rows);
        (self.state.rows.len() - old_len) as i64 * DEF_ROW_SIZE
    }

    /// 按自定义比较器输出下一行的百分位秩；耗尽后返回 `None`。
    pub fn next_by(&mut self, compare: impl Fn(&T, &T) -> Ordering) -> Option<f64> {
        if self.state.cur_idx >= self.state.rows.len() {
            return None;
        }
        self.state.cur_idx += 1;
        // 首行 rank=1，PERCENT_RANK 恒为 0。
        if self.state.cur_idx == 1 {
            self.state.last_rank = 1;
            return Some(0.0);
        }
        // 与前一行不相等则开启新 peer group，rank 取当前 1-based 行号（同 RANK 跳号）。
        if compare(
            &self.state.rows[self.state.cur_idx - 2],
            &self.state.rows[self.state.cur_idx - 1],
        ) != Ordering::Equal
        {
            self.state.last_rank = self.state.cur_idx as i64;
        }
        // 公式：(rank-1)/(n-1)；n=1 时分母按 saturating_sub 变为 0，但首行已提前返回。
        Some((self.state.last_rank - 1) as f64 / (self.state.rows.len().saturating_sub(1)) as f64)
    }
}

impl<T: PartialEq> PercentRank<T> {
    /// 以 `PartialEq` 判定 peer：相等则同组，否则视为新组（不区分大小关系）。
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
