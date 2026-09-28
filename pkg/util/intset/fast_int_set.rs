// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 快速整数集合：小值用 `u64` 位图，大值/越界值落入 `BTreeSet`。
//
// 对应 Go `intsets.Fast`/`Sparse` 思路：`[0, 64)` 走 bitmap 快路径；一旦插入
// 范围外整数，`large` 持有全集，`small` 仍缓存前 64 个非负位，便于集合运算。

#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

/// 位图快路径上界：值落在 `[0, smallCutOff)` 时用 `u64` 位表示。
pub const smallCutOff: i32 = 64;

/// An integer set with a bitmap fast path for values in `[0, 64)`.
///
/// Once an out-of-range value is inserted, `large` contains every value while
/// `small` remains a cache for the first 64 non-negative values. This mirrors
/// the representation and state transitions of Go's `intsets.Sparse` version.
/// 带位图快路径的整数集合；越界插入后 `large` 存全集，`small` 仍缓存 `[0,64)`。
#[derive(Clone, Debug, Default)]
pub struct FastIntSet {
    /// `[0, 64)` 的位图缓存（每位对应一个整数）。
    small: u64,
    /// 含越界值时的有序全集；`None` 表示仍纯位图模式。
    large: Option<BTreeSet<i32>>,
}

/// 集合已含大值时，无法仅返回 `small` 位图的错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmallValueError;

impl fmt::Display for SmallValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("set contains large values, cannot get small uint64")
    }
}

impl Error for SmallValueError {}

/// 由一组初值构造 `FastIntSet`。
pub fn NewFastIntSet(values: Vec<i32>) -> FastIntSet {
    let mut result = FastIntSet::default();
    for value in values {
        result.Insert(value);
    }
    result
}

impl FastIntSet {
    /// 返回集合元素个数。
    pub fn Len(&self) -> i32 {
        self.large.as_ref().map_or_else(
            || self.small.count_ones() as i32,
            |large| large.len() as i32,
        )
    }

    /// 是否仅含单个元素且为 `0`。
    pub fn Only1Zero(&self) -> bool {
        self.Len() == 1 && self.Has(0)
    }

    /// 插入整数：小值写位图，越界则提升到 `large` 并同步全集。
    pub fn Insert(&mut self, value: i32) {
        let is_small = (0..smallCutOff).contains(&value);
        if is_small {
            self.small |= 1_u64 << value;
        } else if self.large.is_none() {
            // 首次越界：把现有位图展开为 BTreeSet 再插入。
            self.large = Some(self.toLarge());
        }
        if let Some(large) = &mut self.large {
            large.insert(value);
        }
    }

    /// 将当前集合展开为 `BTreeSet`（已有 `large` 则克隆）。
    fn toLarge(&self) -> BTreeSet<i32> {
        if let Some(large) = &self.large {
            return large.clone();
        }
        let mut result = BTreeSet::new();
        let mut bits = self.small;
        // 逐个取出位图中置位的下标。
        while bits != 0 {
            let value = bits.trailing_zeros() as i32;
            result.insert(value);
            bits &= !(1_u64 << value);
        }
        result
    }

    /// Returns the first member greater than or equal to `startVal`.
    /// Negative starts are clamped to zero, matching the Go implementation.
    /// 返回 `>= startVal` 的最小成员；负起点钳到 0；无则 `(i32::MAX, false)`。
    pub fn Next(&self, mut startVal: i32) -> (i32, bool) {
        if startVal < smallCutOff {
            startVal = startVal.max(0);
            // 在位图右移后找第一个置位，得到相对 gap。
            let gap = (self.small >> startVal).trailing_zeros() as i32;
            if gap < smallCutOff {
                return (startVal + gap, true);
            }
        }
        if let Some(value) = self
            .large
            .as_ref()
            .and_then(|large| large.range(startVal..).next())
        {
            return (*value, true);
        }
        (i32::MAX, false)
    }

    /// 删除元素：同步更新位图与 `large`。
    pub fn Remove(&mut self, value: i32) {
        if (0..smallCutOff).contains(&value) {
            self.small &= !(1_u64 << value);
        }
        if let Some(large) = &mut self.large {
            large.remove(&value);
        }
    }

    /// 清空集合；保留已分配的 `large` 容器但清空内容。
    pub fn Clear(&mut self) {
        self.small = 0;
        if let Some(large) = &mut self.large {
            large.clear();
        }
    }

    /// 判断是否包含 `value`。
    pub fn Has(&self, value: i32) -> bool {
        if (0..smallCutOff).contains(&value) {
            return self.small & (1_u64 << value) != 0;
        }
        self.large
            .as_ref()
            .is_some_and(|large| large.contains(&value))
    }

    /// 集合是否为空。
    pub fn IsEmpty(&self) -> bool {
        self.small == 0 && self.large.as_ref().is_none_or(BTreeSet::is_empty)
    }

    /// 返回升序元素向量。
    pub fn SortedArray(&self) -> Vec<i32> {
        if let Some(large) = &self.large {
            return large.iter().copied().collect();
        }
        let mut result = Vec::with_capacity(self.Len() as usize);
        self.ForEach(|value| result.push(value));
        result
    }

    /// 按升序对每个元素调用 `f`。
    pub fn ForEach<F>(&self, mut f: F)
    where
        F: FnMut(i32),
    {
        if let Some(large) = &self.large {
            for value in large {
                f(*value);
            }
            return;
        }
        let mut bits = self.small;
        while bits != 0 {
            let value = bits.trailing_zeros() as i32;
            f(value);
            bits &= !(1_u64 << value);
        }
    }

    /// 深拷贝当前集合。
    pub fn Copy(&self) -> FastIntSet {
        self.clone()
    }

    /// 从 `target` 拷贝内容到 `self`（尽量复用已有 `large` 缓冲）。
    pub fn CopyFrom(&mut self, target: &FastIntSet) {
        self.small = target.small;
        match (&mut self.large, &target.large) {
            (Some(large), Some(target_large)) => large.clone_from(target_large),
            (None, Some(target_large)) => self.large = Some(target_large.clone()),
            (Some(large), None) => large.clear(),
            (None, None) => {}
        }
    }

    /// 判断两集合是否相等（含一侧仍为纯位图、另一侧 `large` 仅含小值的情形）。
    pub fn Equals(&self, rhs: &FastIntSet) -> bool {
        match (&self.large, &rhs.large) {
            (None, None) => self.small == rhs.small,
            (Some(left), Some(right)) => left == right,
            (Some(_), None) => !self.largeToSmall().1 && self.small == rhs.small,
            (None, Some(_)) => !rhs.largeToSmall().1 && self.small == rhs.small,
        }
    }

    /// 若 `large` 仅含 `[0,64)` 值则返回 `(small, false)`，否则第二分量 `true`。
    fn largeToSmall(&self) -> (u64, bool) {
        let large = self.large.as_ref().expect("set contains no large");
        let has_other_values = large.first().is_some_and(|value| *value < 0)
            || large.last().is_some_and(|value| *value >= smallCutOff);
        (self.small, has_other_values)
    }

    /// 仅在纯位图模式返回 `small`；已有大值则报 `SmallValueError`。
    pub fn GetSmallUInt64(&self) -> Result<u64, SmallValueError> {
        if self.large.is_some() {
            Err(SmallValueError)
        } else {
            Ok(self.small)
        }
    }

    /// 差集：`self \ rhs` 的新集合。
    pub fn Difference(&self, rhs: &FastIntSet) -> FastIntSet {
        let mut result = self.Copy();
        result.DifferenceWith(rhs);
        result
    }

    /// 原地差集：从 `self` 去掉 `rhs` 中的元素。
    pub fn DifferenceWith(&mut self, rhs: &FastIntSet) {
        self.small &= !rhs.small;
        if let Some(large) = &mut self.large {
            let rhs_large = rhs.toLarge();
            large.retain(|value| !rhs_large.contains(value));
        }
    }

    /// 并集：`self ∪ rhs` 的新集合。
    pub fn Union(&self, rhs: &FastIntSet) -> FastIntSet {
        let mut result = self.Copy();
        result.UnionWith(rhs);
        result
    }

    /// 原地并集。
    pub fn UnionWith(&mut self, rhs: &FastIntSet) {
        self.small |= rhs.small;
        if self.large.is_none() && rhs.large.is_none() {
            return;
        }
        if self.large.is_none() {
            self.large = Some(self.toLarge());
        }
        let rhs_large = rhs.toLarge();
        self.large.as_mut().unwrap().extend(rhs_large);
    }

    /// 交集：`self ∩ rhs` 的新集合。
    pub fn Intersection(&self, rhs: &FastIntSet) -> FastIntSet {
        let mut result = self.Copy();
        result.IntersectionWith(rhs);
        result
    }

    /// 原地交集；若 `rhs` 无 `large` 则丢弃本侧 `large`。
    pub fn IntersectionWith(&mut self, rhs: &FastIntSet) {
        self.small &= rhs.small;
        if rhs.large.is_none() {
            self.large = None;
            return;
        }
        if let Some(large) = &mut self.large {
            let rhs_large = rhs.toLarge();
            large.retain(|value| rhs_large.contains(value));
        }
    }

    /// 是否与 `rhs` 有公共元素。
    pub fn Intersects(&self, rhs: &FastIntSet) -> bool {
        if self.small & rhs.small != 0 {
            return true;
        }
        match (&self.large, &rhs.large) {
            (Some(left), Some(right)) => left.iter().any(|value| right.contains(value)),
            _ => false,
        }
    }

    /// 是否为 `rhs` 的子集。
    pub fn SubsetOf(&self, rhs: &FastIntSet) -> bool {
        if self.large.is_none() {
            return self.small & rhs.small == self.small;
        }
        if let (Some(left), Some(right)) = (&self.large, &rhs.large) {
            return left.is_subset(right);
        }
        !self.largeToSmall().1 && self.small & rhs.small == self.small
    }

    /// 将所有元素平移 `delta`；小集合且不越界时走位图移位快路径。
    pub fn Shift(&self, delta: i32) -> FastIntSet {
        if self.IsEmpty() {
            return FastIntSet::default();
        }
        if self.large.is_none() {
            // 右移/左移后仍落在 `u64` 内则直接移位，避免展开。
            if delta > 0 && delta < smallCutOff && self.small.leading_zeros() as i32 >= delta {
                return FastIntSet {
                    small: self.small << delta,
                    large: None,
                };
            }
            if delta <= 0 && delta > -smallCutOff && self.small.trailing_zeros() as i32 >= -delta {
                return FastIntSet {
                    small: self.small >> -delta,
                    large: None,
                };
            }
        }
        let mut result = FastIntSet::default();
        self.ForEach(|value| result.Insert(value + delta));
        result
    }

    /// 向集合加入闭区间 `[from, to]` 内全部整数。
    pub fn AddRange(&mut self, from: i32, to: i32) {
        assert!(to >= from, "invalid range when adding range to FastIntSet");
        if from >= 0 && to < smallCutOff && self.large.is_none() {
            // 纯位图模式下用掩码一次 OR 写入整段。
            let count = (to - from + 1) as u32;
            let mask = if count == 64 {
                u64::MAX
            } else {
                (1_u64 << count) - 1
            };
            self.small |= mask << from;
            return;
        }
        for value in from..=to {
            self.Insert(value);
        }
    }

    /// 格式化为 `(a,b-c,...)`：负值逐个列出，非负连续段压缩为区间。
    pub fn String(&self) -> String {
        fn append_range(buffer: &mut String, start: i32, end: i32) {
            if buffer.len() > 1 {
                buffer.push(',');
            }
            if start == end {
                buffer.push_str(&start.to_string());
            } else if start + 1 == end {
                buffer.push_str(&format!("{start},{end}"));
            } else {
                buffer.push_str(&format!("{start}-{end}"));
            }
        }

        let mut buffer = String::from("(");
        let mut range: Option<(i32, i32)> = None;
        self.ForEach(|value| {
            if value < 0 {
                append_range(&mut buffer, value, value);
            } else if let Some((start, end)) = range {
                if end == value - 1 {
                    range = Some((start, value));
                } else {
                    append_range(&mut buffer, start, end);
                    range = Some((value, value));
                }
            } else {
                range = Some((value, value));
            }
        });
        if let Some((start, end)) = range {
            append_range(&mut buffer, start, end);
        }
        buffer.push(')');
        buffer
    }
}

impl fmt::Display for FastIntSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}
