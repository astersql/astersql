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

// FastIntSet 功能单测：基础操作、随机往返、双集合运算、AddRange 与字符串表示。
//
// 使用确定性伪随机（`TestRand`）在不同规模（含 `smallCutOff` 边界）上
// 对照布尔向量/哈希表期望，覆盖 Insert/Remove/Next/ForEach/Copy 及并交差。

use super::{FastIntSet, NewFastIntSet, smallCutOff};
use std::collections::{HashMap, HashSet};

/// 测试用对照集合：标准 `HashSet<i32>`。
pub type IntSet = HashSet<i32>;

/// 构造空的对照 HashSet。
pub fn NewIntSet() -> IntSet {
    HashSet::new()
}

/// 计算两个对照集合的差集（元素在 target1 中且不在 target2 中）。
pub fn difference2(target1: &IntSet, target2: &IntSet) -> IntSet {
    target1.difference(target2).copied().collect()
}

/// 可复现的线性同余伪随机，用于确定性 fuzz 风格测试。
#[derive(Clone)]
struct TestRand(u64);

impl TestRand {
    /// 用固定种子构造生成器。
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// 推进状态并返回下一个 u64。
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// 返回 `[0, upper)` 内的随机 i32。
    fn intn(&mut self, upper: i32) -> i32 {
        (self.next_u64() % upper as u64) as i32
    }

    /// 生成 `0..size` 的随机排列（Fisher–Yates）。
    fn perm(&mut self, size: i32) -> Vec<i32> {
        let mut values: Vec<_> = (0..size).collect();
        for i in (1..values.len()).rev() {
            let j = self.next_u64() as usize % (i + 1);
            values.swap(i, j);
        }
        values
    }
}

/// 基础路径：Insert/Remove/Len/Has、Next 升序遍历、Clear、ForEach、Copy/CopyFrom。
#[test]
fn test_fast_int_set_basic() {
    let mut fis = FastIntSet::default();
    for value in [1, 2, 3] {
        fis.Insert(value);
    }
    assert_eq!(fis.Len(), 3);
    assert!(fis.Has(1) && fis.Has(2) && fis.Has(3));
    fis.Remove(2);
    assert_eq!(fis.Len(), 2);
    assert!(fis.Has(1) && fis.Has(3));
    fis.Remove(3);
    assert_eq!(fis.Len(), 1);
    assert!(fis.Has(1));
    fis.Remove(1);
    assert_eq!(fis.Len(), 0);

    // 含负数与大于 smallCutOff 的值；Next 只返回非负成员。
    for value in [6, 3, 0, -1, 77] {
        fis.Insert(value);
    }
    let (mut value, mut ok) = fis.Next(i32::MIN);
    for expected in [0, 3, 6, 77] {
        assert!(ok);
        assert_eq!(value, expected);
        (value, ok) = fis.Next(value + 1);
    }
    assert!(!ok);
    assert_eq!(value, i32::MAX);

    fis.Clear();
    assert_eq!(fis.Len(), 0);
    assert!(fis.IsEmpty());

    for value in [1, -1, 77] {
        fis.Insert(value);
    }
    let mut visited = Vec::new();
    fis.ForEach(|value| visited.push(value));
    assert_eq!(visited, fis.SortedArray());
    assert_eq!(visited.len(), 3);

    let copy = fis.Copy();
    assert_eq!(fis.SortedArray(), copy.SortedArray());
    assert!(fis.Equals(&copy));

    let mut copied_from = NewFastIntSet(vec![100]);
    copied_from.CopyFrom(&fis);
    assert_eq!(copied_from.SortedArray(), copy.SortedArray());
    assert!(copied_from.Equals(&copy));
}

/// 断言副本与原集相等；若可删一元素则验证 Equals 对修改敏感，再插回恢复。
fn assert_same(original: &FastIntSet, mut copied: FastIntSet) {
    assert!(original.Equals(&copied) && copied.Equals(original));
    let (column, ok) = copied.Next(0);
    if ok {
        copied.Remove(column);
        assert!(!original.Equals(&copied) && !copied.Equals(original));
        copied.Insert(column);
        assert!(original.Equals(&copied) && copied.Equals(original));
    }
}

/// 随机 Insert/Remove 往返：对照布尔向量，覆盖 ForEach、Next、Copy、CopyFrom、Shift 复用。
#[test]
fn test_fast_int_set() {
    for (case, max_value) in [1, 8, 30, smallCutOff, 2 * smallCutOff, 4 * smallCutOff]
        .into_iter()
        .enumerate()
    {
        let mut rng = TestRand::new(0x1470_0000 + case as u64);
        let mut expected = vec![false; max_value as usize];
        let mut set = FastIntSet::default();
        for _ in 0..1000 {
            let value = rng.intn(max_value);
            if rng.intn(2) == 0 {
                expected[value as usize] = true;
                set.Insert(value);
            } else {
                expected[value as usize] = false;
                set.Remove(value);
            }
            assert_eq!(set.IsEmpty(), !expected.iter().any(|present| *present));
            for (value, present) in expected.iter().copied().enumerate() {
                assert_eq!(set.Has(value as i32), present);
            }

            // ForEach 访问集合应与期望位向量一致。
            let mut visited = vec![false; max_value as usize];
            set.ForEach(|value| visited[value as usize] = true);
            assert_eq!(visited, expected);

            // Next 升序序列应等于 SortedArray。
            let mut via_next = Vec::new();
            let (mut value, mut ok) = set.Next(0);
            while ok {
                via_next.push(value);
                (value, ok) = set.Next(value + 1);
            }
            assert_eq!(via_next, set.SortedArray());

            assert_same(&set, set.Copy());
            let mut copy = FastIntSet::default();
            copy.CopyFrom(&set);
            assert_same(&set, copy);
            // Shift 后再 CopyFrom，验证复用已偏移的存储。
            let shifted = set.Shift(100);
            let mut reused = shifted;
            reused.CopyFrom(&set);
            assert_same(&set, reused);
        }
    }
}

/// 生成带插入与删除的随机集合，并返回成员期望表。
fn generate_set(
    rng: &mut TestRand,
    num_elements: i32,
    num_removed: i32,
    value_range: i32,
) -> (FastIntSet, HashMap<i32, bool>) {
    let values = rng.perm(value_range)[..(num_elements + num_removed) as usize].to_vec();
    let mut expected: HashMap<_, _> = values.iter().map(|value| (*value, true)).collect();
    let mut set = FastIntSet::default();
    for value in expected.keys() {
        set.Insert(*value);
    }
    // 按随机顺序删除 num_removed 个元素。
    let removal_order = rng.perm(values.len() as i32);
    for index in removal_order.into_iter().take(num_removed as usize) {
        let value = values[index as usize];
        set.Remove(value);
        expected.remove(&value);
    }
    (set, expected)
}

/// 判断 left 的键是否都是 right 的键（集合意义上的子集）。
fn map_subset(left: &HashMap<i32, bool>, right: &HashMap<i32, bool>) -> bool {
    left.keys().all(|value| right.contains_key(value))
}

/// 双集合运算：Shift、SubsetOf/Equals、Union/Intersection/Difference 与期望表对照。
#[test]
fn test_fast_int_set_two_set_ops() {
    let mut rng = TestRand::new(0x1472_5e70);
    for min_value in [-10, -1, 0, smallCutOff, 2 * smallCutOff] {
        for value_range in [0, 20, 200] {
            for num1 in [0, 1, 5, 10, 20] {
                for removed1 in [0, 1, 3, 8] {
                    let (set1, map1) =
                        generate_set(&mut rng, num1, removed1, num1 + removed1 + value_range);
                    // 各方向 Shift 后成员应一一对应偏移。
                    for shift in [-100, -10, -1, 1, 2, 10, 100] {
                        let shifted = set1.Shift(shift);
                        set1.ForEach(|value| assert!(shifted.Has(value + shift)));
                        shifted.ForEach(|value| assert!(set1.Has(value - shift)));
                    }
                    for num2 in [0, 1, 5, 10, 20] {
                        for removed2 in [0, 1, 4, 10] {
                            let (set2, map2) = generate_set(
                                &mut rng,
                                num2,
                                removed2,
                                num2 + removed2 + value_range,
                            );
                            let subset1 = map_subset(&map1, &map2);
                            let subset2 = map_subset(&map2, &map1);
                            assert_eq!(set1.SubsetOf(&set2), subset1);
                            assert_eq!(set2.SubsetOf(&set1), subset2);
                            assert_eq!(set1.Equals(&set2), subset1 && subset2);
                            assert_eq!(set2.Equals(&set1), subset1 && subset2);

                            // UnionWith 就地合并应等于非破坏性 Union。
                            let mut union = set1.Copy();
                            union.UnionWith(&set2);
                            assert!(union.Equals(&set1.Union(&set2)));
                            for value in map1.keys().chain(map2.keys()) {
                                assert!(union.Has(*value));
                            }
                            union.ForEach(|value| {
                                assert!(map1.contains_key(&value) || map2.contains_key(&value))
                            });

                            // 交集非空 ⇔ Intersects；就地 IntersectionWith 对齐 Intersection。
                            let mut intersection = set1.Copy();
                            intersection.IntersectionWith(&set2);
                            assert_eq!(set1.Intersects(&set2), !intersection.IsEmpty());
                            assert_eq!(set2.Intersects(&set1), !intersection.IsEmpty());
                            assert!(intersection.Equals(&set1.Intersection(&set2)));
                            for value in map1.keys() {
                                if map2.contains_key(value) {
                                    assert!(intersection.Has(*value));
                                }
                            }
                            intersection.ForEach(|value| {
                                assert!(map1.contains_key(&value) && map2.contains_key(&value))
                            });

                            // 差集成员均来自 map1 且不在 map2。
                            let mut difference = set1.Copy();
                            difference.DifferenceWith(&set2);
                            assert!(difference.Equals(&set1.Difference(&set2)));
                            for value in map1.keys() {
                                if !map2.contains_key(value) {
                                    assert!(difference.Has(*value));
                                }
                            }
                            let (mut value, mut ok) = difference.Next(min_value);
                            while ok {
                                assert!(map1.contains_key(&value));
                                (value, ok) = difference.Next(value + 1);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// AddRange：闭区间 [from, to] 内元素连续且不超过 to。
#[test]
fn test_fast_int_set_add_range() {
    let max_value = smallCutOff + 20;
    for from in -5..=max_value {
        for to in from..=max_value {
            let mut set = FastIntSet::default();
            set.AddRange(from, to);
            let mut expected = from;
            set.ForEach(|actual| {
                assert!(actual <= to);
                assert_eq!(actual, expected);
                expected += 1;
            });
        }
    }
}

/// GetSmallUInt64：仅当全部成员落在 0..63 时返回位掩码，否则报错。
#[test]
fn test_get_small_uint64() {
    assert_eq!(FastIntSet::default().GetSmallUInt64().unwrap(), 0);
    assert_eq!(
        NewFastIntSet(vec![0, 1, 3]).GetSmallUInt64().unwrap(),
        0b1011
    );
    assert_eq!(
        NewFastIntSet(vec![0, 1, 2, 3, 4, 5])
            .GetSmallUInt64()
            .unwrap(),
        63
    );
    assert!(NewFastIntSet(vec![64]).GetSmallUInt64().is_err());
    assert!(NewFastIntSet(vec![1, 64]).GetSmallUInt64().is_err());
}

/// String：空集、连续区间压缩与间断元素的文本表示。
#[test]
fn test_fast_int_set_string() {
    for (values, expected) in [
        (vec![], "()"),
        (vec![-5, -3, -2, -1, 0, 1, 2, 3, 4, 5], "(-5,-3,-2,-1,0-5)"),
        (vec![0, 1, 3, 4, 5], "(0,1,3-5)"),
    ] {
        assert_eq!(NewFastIntSet(values).String(), expected);
    }
}
