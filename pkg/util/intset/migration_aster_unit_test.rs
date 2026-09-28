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

// FastIntSet 迁移对照单测：核心行为与 Go `util/intset` 对齐。
//
// 覆盖小位图、溢出到大表示、集合运算、Copy、区间、Shift、字符串表示与非法区间 panic。

use super::fast_int_set::{FastIntSet, NewFastIntSet};

/// 按升序收集集合中全部元素，便于断言与 Go 结果对比。
fn values(set: &FastIntSet) -> Vec<i32> {
    let mut result = Vec::new();
    set.ForEach(|value| result.push(value));
    result
}

/// 小位图插入/Next、越界后切换大表示，以及删除越界值后仍保持大表示（与 Go 一致）。
#[test]
fn migration_small_bitmap_and_large_transition_match_go() {
    let mut set = NewFastIntSet(vec![0, 1, 3, 63]);
    assert_eq!(set.Len(), 4);
    assert_eq!(
        set.GetSmallUInt64(),
        Ok((1 << 0) | (1 << 1) | (1 << 3) | (1 << 63))
    );
    assert_eq!(set.Next(-100), (0, true));
    assert_eq!(set.Next(2), (3, true));

    // 插入负数与 >=64 的值，迫使从 small bitmap 切到 large 表示。
    set.Insert(-5);
    set.Insert(64);
    assert_eq!(values(&set), vec![-5, 0, 1, 3, 63, 64]);
    assert!(set.GetSmallUInt64().is_err());

    // Go keeps the allocated large representation after its out-of-range
    // values are removed, and GetSmallUInt64 still reports an error.
    // 删除越界元素后仍保留 large 分配，GetSmallUInt64 继续返回错误。
    set.Remove(-5);
    set.Remove(64);
    assert_eq!(values(&set), vec![0, 1, 3, 63]);
    assert!(set.GetSmallUInt64().is_err());
    assert!(set.Equals(&NewFastIntSet(vec![0, 1, 3, 63])));
}

/// 并集/交集/差集、Intersects、SubsetOf，以及 Copy / CopyFrom 独立性。
#[test]
fn migration_copy_and_set_operators_match_go() {
    let left = NewFastIntSet(vec![-2, 0, 2, 64, 100]);
    let right = NewFastIntSet(vec![0, 3, 64, 80]);

    assert_eq!(values(&left.Union(&right)), vec![-2, 0, 2, 3, 64, 80, 100]);
    assert_eq!(values(&left.Intersection(&right)), vec![0, 64]);
    assert_eq!(values(&left.Difference(&right)), vec![-2, 2, 100]);
    assert!(left.Intersects(&right));
    assert!(NewFastIntSet(vec![0, 64]).SubsetOf(&left));

    // Copy 后修改副本不应影响原集合。
    let mut copy = left.Copy();
    copy.Remove(64);
    assert!(left.Has(64));
    assert!(!copy.Has(64));

    // CopyFrom 复用目标集合的存储并覆盖内容。
    let mut reused = NewFastIntSet(vec![1000]);
    reused.CopyFrom(&right);
    assert!(reused.Equals(&right));
}

/// AddRange、Shift、String 压缩表示，以及空集字符串。
#[test]
fn migration_range_shift_and_string_match_go() {
    let mut all_small = FastIntSet::default();
    all_small.AddRange(0, 63);
    assert_eq!(all_small.Len(), 64);
    assert_eq!(all_small.GetSmallUInt64(), Ok(u64::MAX));

    let shifted = NewFastIntSet(vec![0, 1, 63]).Shift(-1);
    assert_eq!(values(&shifted), vec![-1, 0, 62]);

    // 跨 smallCutOff 的大区间，验证 Len / Next / String。
    let ranged = {
        let mut set = FastIntSet::default();
        set.AddRange(-2, 66);
        set
    };
    assert_eq!(ranged.Len(), 69);
    assert_eq!(ranged.Next(-10), (0, true));
    assert_eq!(ranged.String(), "(-2,-1,0-66)");

    assert_eq!(NewFastIntSet(vec![]).String(), "()");
    assert_eq!(
        NewFastIntSet(vec![-5, -3, -2, -1, 0, 1, 2, 3, 4, 5]).String(),
        "(-5,-3,-2,-1,0-5)"
    );
}

/// 非法区间（from > to）应 panic，与 Go 一致。
#[test]
#[should_panic(expected = "invalid range when adding range to FastIntSet")]
fn migration_invalid_range_panics_like_go() {
    FastIntSet::default().AddRange(2, 1);
}
