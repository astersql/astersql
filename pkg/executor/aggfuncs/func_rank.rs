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

// RANK / DENSE_RANK 窗口函数及排序键比较器。
//
// `RANK`：同行组共享同一秩，下一组跳号到当前行号；`DENSE_RANK`：下一组仅 +1 不跳号。
// `RowComparer` 按多列比较器依次比较，首个不相等列决定两行是否为 peer（同行组）。
// 部分结果大小常量供内存追踪与 Go 侧 `DefPartialResult4RankSize` 对齐。

use std::cmp::Ordering;
use std::mem::size_of;

pub use crate::aggfuncs::DEF_ROW_SIZE;

/// `RankState` 空状态的固定体积，对应 Go 部分结果基线大小。
pub const DEF_PARTIAL_RESULT_RANK_SIZE: i64 = size_of::<RankState<()>>() as i64;
/// The ordering-column comparers used by RANK, DENSE_RANK, PERCENT_RANK and
/// CUME_DIST. The first non-equal column determines whether two rows are peers.
/// 供 RANK/DENSE_RANK/PERCENT_RANK/CUME_DIST 使用的多列比较器集合；
/// 第一个不相等列决定两行是否同属 peer group。
pub struct RowComparer<T> {
    comparers: Vec<fn(&T, &T) -> Ordering>,
}

impl<T> RowComparer<T> {
    /// 以列比较器列表构造；顺序即 ORDER BY 列优先级。
    pub fn new(comparers: Vec<fn(&T, &T) -> Ordering>) -> Self {
        Self { comparers }
    }

    /// 依次调用比较器，返回首个非 Equal 结果；全部相等则 Equal。
    pub fn compare(&self, left: &T, right: &T) -> Ordering {
        self.comparers
            .iter()
            .map(|compare| compare(left, right))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

/// 排名公共状态：当前输出下标、最近一次秩、分区内排序键行缓冲。
#[derive(Clone, Debug, PartialEq)]
pub struct RankState<T> {
    pub cur_idx: usize,
    pub last_rank: i64,
    pub rows: Vec<T>,
}

impl<T> Default for RankState<T> {
    fn default() -> Self {
        Self {
            cur_idx: 0,
            last_rank: 0,
            rows: Vec::new(),
        }
    }
}

/// RANK（`is_dense=false`）或 DENSE_RANK（`is_dense=true`）的求值状态机。
#[derive(Clone, Debug, PartialEq)]
pub struct Rank<T> {
    is_dense: bool,
    state: RankState<T>,
}

impl<T> Rank<T> {
    /// `is_dense=true` 为 DENSE_RANK，否则为带跳号的 RANK。
    pub fn new(is_dense: bool) -> Self {
        Self {
            is_dense,
            state: RankState::default(),
        }
    }

    /// 清空行缓冲与游标，准备下一分区。
    pub fn reset(&mut self) {
        self.state.cur_idx = 0;
        self.state.last_rank = 0;
        self.state.rows.clear();
    }

    /// 追加排序键行，返回估算内存增量。
    pub fn update(&mut self, rows: impl IntoIterator<Item = T>) -> i64 {
        let old_len = self.state.rows.len();
        self.state.rows.extend(rows);
        (self.state.rows.len() - old_len) as i64 * DEF_ROW_SIZE
    }

    /// 按自定义比较器输出下一行之秩；耗尽返回 `None`。
    pub fn next_by(&mut self, compare: impl Fn(&T, &T) -> Ordering) -> Option<i64> {
        if self.state.cur_idx >= self.state.rows.len() {
            return None;
        }
        self.state.cur_idx += 1;
        // 首行秩恒为 1。
        if self.state.cur_idx == 1 {
            self.state.last_rank = 1;
        } else if compare(
            &self.state.rows[self.state.cur_idx - 2],
            &self.state.rows[self.state.cur_idx - 1],
        ) != Ordering::Equal
        {
            // 新 peer group：DENSE_RANK 递增 1；RANK 跳到当前 1-based 行号。
            if self.is_dense {
                self.state.last_rank += 1;
            } else {
                self.state.last_rank = self.state.cur_idx as i64;
            }
        }
        Some(self.state.last_rank)
    }

    /// 只读访问内部 `RankState`（供 PERCENT_RANK 等复用或调试）。
    pub fn state(&self) -> &RankState<T> {
        &self.state
    }
}

impl<T: PartialEq> Rank<T> {
    /// 以 `PartialEq` 判定 peer：相等同组，否则开启新组。
    pub fn next(&mut self) -> Option<i64> {
        self.next_by(|left, right| {
            if left == right {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        })
    }
}
