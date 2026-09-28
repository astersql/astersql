// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// selection 迁移单元测试。
//
// 验证 Select 的 1-based 排名、空切片哨兵 -1、重复值与大规模输入
// 相对排序结果的一致性，对齐 Go 迁移基线。

use super::{Interface, Select};

/// 实现 Interface 的整数切片，供 Select 就地重排。
#[derive(Debug)]
struct TestSlice(Vec<i32>);

impl Interface for TestSlice {
    fn Len(&self) -> isize {
        self.0.len() as isize
    }

    fn Less(&self, i: isize, j: isize) -> bool {
        self.0[i as usize] < self.0[j as usize]
    }

    fn Swap(&mut self, i: isize, j: isize) {
        self.0.swap(i as usize, j as usize);
    }
}

/// 对副本执行 Select 并返回第 rank 小元素的值（rank 为 1-based）。
fn selected_value(values: Vec<i32>, rank: isize) -> i32 {
    let mut data = TestSlice(values);
    let index = Select(&mut data, rank);
    data.0[index as usize]
}

/// 校验空切片返回 -1，以及正序/逆序下 1-based 排名取值。
#[test]
fn selection_matches_go_one_based_rank_and_empty_sentinel() {
    let mut empty = TestSlice(Vec::new());
    assert_eq!(Select(&mut empty, 1), -1);

    assert_eq!(selected_value(vec![1, 2, 3, 4, 5], 3), 3);
    assert_eq!(selected_value(vec![5, 4, 3, 2, 1], 1), 1);
    assert_eq!(selected_value(vec![5, 4, 3, 2, 1], 5), 5);
}

/// 校验含重复值时 Select 仍返回对应排名的元素。
#[test]
fn selection_matches_go_duplicate_behavior() {
    assert_eq!(selected_value(vec![1, 2, 3, 3, 5], 3), 3);
    assert_eq!(selected_value(vec![1, 2, 3, 3, 5], 5), 5);
    assert_eq!(selected_value(vec![7; 101], 51), 7);
}

/// 对较大输入将 Select 结果与全排序后的对应位置比对。
#[test]
fn selection_matches_sorted_result_for_large_inputs() {
    let cases = [
        (0..257).rev().collect::<Vec<_>>(),
        (0..1000).map(|i| (i * 37 + 11) % 97).collect::<Vec<_>>(),
    ];

    for values in cases {
        for rank in [1, values.len() / 2, values.len()] {
            let mut sorted = values.clone();
            sorted.sort();
            assert_eq!(
                selected_value(values.clone(), rank as isize),
                sorted[rank - 1]
            );
        }
    }
}
