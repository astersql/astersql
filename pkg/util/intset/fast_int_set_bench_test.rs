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

// FastIntSet 与标准集合实现的基准对比测试。
//
// 对比 HashSet/BTreeSet 与 FastIntSet 在差集（Difference）与插入路径上的规模与开销；
// 使用 `black_box` 防止编译器优化掉结果，便于观察真实工作量。

use super::NewFastIntSet;
use super::fast_int_set_test::{NewIntSet, difference2};
use std::collections::BTreeSet;
use std::hint::black_box;

/// 基准：基于 HashSet（`NewIntSet`）的差集，规模约 20 万元素。
#[test]
fn benchmark_map_int_set_difference() {
    // 构造两个有重叠区间的集合，差集期望长度为 10 万。
    let set_a = (0..200_000).collect();
    let set_b = (100_000..300_000).collect();
    let difference = difference2(&set_a, &set_b);
    assert_eq!(black_box(difference).len(), 100_000);
}

/// 基准：基于 BTreeSet 的差集；空右集时差集等于左集并集后的全部元素。
#[test]
fn benchmark_int_set_difference() {
    let mut set_a: BTreeSet<_> = (0..200_000).collect();
    let set_b = BTreeSet::new();
    // 扩展左集覆盖更大区间，验证有序集合差集路径。
    set_a.extend(100_000..300_000);
    let difference: BTreeSet<_> = set_a.difference(&set_b).copied().collect();
    assert_eq!(black_box(difference).len(), 300_000);
}

/// 基准：FastIntSet 差集；右集为空时结果长度等于左集元素数。
#[test]
fn benchmark_fast_int_set_difference() {
    let mut set_a = NewFastIntSet(vec![]);
    for value in 0..200_000 {
        set_a.Insert(value);
    }
    let set_b = NewFastIntSet(vec![]);
    // 继续向左集插入，扩大到 30 万唯一整数。
    for value in 100_000..300_000 {
        set_a.Insert(value);
    }
    assert_eq!(black_box(set_a.Difference(&set_b)).Len(), 300_000);
}

/// 基准：HashSet 连续插入小规模（64）整数。
#[test]
fn benchmark_int_set_insert() {
    let mut set = NewIntSet();
    for value in 0..64 {
        set.insert(value);
    }
    assert_eq!(black_box(set).len(), 64);
}

/// 基准：BTreeSet 连续插入小规模整数（稀疏/有序表示对照）。
#[test]
fn benchmark_sparse_insert() {
    let mut set = BTreeSet::new();
    for value in 0..64 {
        set.insert(value);
    }
    assert_eq!(black_box(set).len(), 64);
}

/// 基准：FastIntSet 连续插入小规模整数（位图小集路径）。
#[test]
fn benchmark_fast_int_set_insert() {
    let mut set = NewFastIntSet(vec![]);
    for value in 0..64 {
        set.Insert(value);
    }
    assert_eq!(black_box(set).Len(), 64);
}
