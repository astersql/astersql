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

// Key Range（键范围）异常诊断工具。
//
// Coprocessor / MPP 在按 Region（键空间分片）拆分任务前，需检查输入 ranges 是否
// 存在重复、重叠、包含、乱序、非法边界或无限尾等异常，便于日志诊断与 fallback。
// 空 end 键表示正无穷上界（+inf）。

use crate::key_ranges::{KeyRange, KeyRanges};

/// 一组 Key Range 上各类异常的计数统计。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RangeIssueStats {
    /// 完全相同的相邻 range 次数。
    pub duplicate: usize,
    /// 部分相交但不互相包含的次数。
    pub overlap: usize,
    /// 一方完全包含另一方的次数。
    pub contain: usize,
    /// 结束键小于下一起点但按扫描顺序“乱序”的次数。
    pub out_of_order: usize,
    /// start > end（且 end 非空）的非法边界次数。
    pub invalid_bound: usize,
    /// 非末尾位置出现无限 end（空 end）的次数。
    pub infinite_tail: usize,
}

impl RangeIssueStats {
    /// 是否没有任何异常计数。
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// 比较两个 range 的 end 边界；空切片视为 +inf，大于任何有限键。
pub fn compare_range_end(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => left.cmp(right),
    }
}

/// 判断 `outer` 是否在键空间上完全包含 `inner`（半开区间语义）。
pub fn range_contains(outer: &KeyRange, inner: &KeyRange) -> bool {
    outer.start <= inner.start
        && compare_range_end(&outer.end, &inner.end) != std::cmp::Ordering::Less
}

/// 判断两个半开区间是否相交；空 end 表示延伸到 +inf。
pub fn ranges_overlap(left: &KeyRange, right: &KeyRange) -> bool {
    (left.end.is_empty() || left.end > right.start)
        && (right.end.is_empty() || right.end > left.start)
}

/// 对相邻一对有问题的 range 分类并累加到统计。
fn classify_pair(stats: &mut RangeIssueStats, previous: &KeyRange, current: &KeyRange) {
    if previous == current {
        stats.duplicate += 1;
    } else if range_contains(previous, current) || range_contains(current, previous) {
        stats.contain += 1;
    } else if ranges_overlap(previous, current) {
        stats.overlap += 1;
    } else {
        stats.out_of_order += 1;
    }
}

/// 扫描整组 KeyRanges，汇总重复/重叠/包含/乱序/非法边界/无限尾等问题。
pub fn range_issues_for_key_ranges(ranges: &KeyRanges) -> RangeIssueStats {
    let mut stats = RangeIssueStats::default();
    let Some(mut previous) = ranges.ref_at(0) else {
        return stats;
    };
    if !previous.end.is_empty() && previous.start > previous.end {
        stats.invalid_bound += 1;
    }
    for current in ranges.iter().skip(1) {
        if !current.end.is_empty() && current.start > current.end {
            stats.invalid_bound += 1;
        }
        // 前一段已是无限尾，后续任何 range 都无法再合法接续。
        if previous.end.is_empty() {
            stats.infinite_tail += 1;
        } else if previous.end > current.start {
            classify_pair(&mut stats, previous, current);
        }
        previous = current;
    }
    stats
}

/// 返回一组 ranges 中最小 start 与最大 end（空 end 按 +inf 参与比较）。
pub fn min_start_and_max_end_key_of_key_ranges(
    ranges: &KeyRanges,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let Some(first) = ranges.ref_at(0) else {
        return (None, None);
    };
    let mut minimum = first.start.clone();
    let mut maximum = first.end.clone();
    for range in ranges.iter().skip(1) {
        if range.start < minimum {
            minimum.clone_from(&range.start);
        }
        if compare_range_end(&range.end, &maximum).is_gt() {
            maximum.clone_from(&range.end);
        }
    }
    (Some(minimum), Some(maximum))
}

/// 查找第一个超出给定 location（Region 边界）的 KeyRange，并返回索引、副本与原因标签。
pub fn first_out_of_bound_key_range_in_location(
    ranges: &KeyRanges,
    location_start: &[u8],
    location_end: &[u8],
) -> Option<(usize, KeyRange, &'static str)> {
    for (index, range) in ranges.iter().enumerate() {
        let reason = if range.start.as_slice() < location_start {
            Some("start_before_location_start")
        } else if !location_end.is_empty() && range.start.as_slice() >= location_end {
            Some("start_after_or_eq_location_end")
        } else if range.end.is_empty() && !location_end.is_empty() {
            Some("end_infinite_but_location_finite")
        } else if !range.end.is_empty()
            && !location_end.is_empty()
            && range.end.as_slice() > location_end
        {
            Some("end_after_location_end")
        } else if !range.end.is_empty() && range.start > range.end {
            Some("invalid_start_greater_than_end")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Some((index, range.clone(), reason));
        }
    }
    None
}
