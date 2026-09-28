// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 递归切片展平迭代器（`SliceRecursiveFlattenIter`）的单元测试。
//
// `RecursiveSlice` 可嵌套 Values/Slices；迭代器深度优先展平并产出全局下标。
// 用例覆盖空树、嵌套、跳过（continue）与提前终止（break）语义。

use super::{RecursiveSlice, SliceRecursiveFlattenIter};

/// 构造叶子节点：直接持有字符串值列表。
fn values(values: &[&'static str]) -> RecursiveSlice<&'static str> {
    RecursiveSlice::Values(values.to_vec())
}

/// 构造中间节点：持有子 RecursiveSlice 列表。
fn slices(values: Vec<RecursiveSlice<&'static str>>) -> RecursiveSlice<&'static str> {
    RecursiveSlice::Slices(values)
}

/// 用多组嵌套结构验证展平顺序、下标连续性，以及 continue/break 对输出的影响。
#[test]
fn test_slice_recursive_flatten_iter() {
    crate::main_test::setup_for_planner_util_test();
    /// 输入树、需跳过的值、终止值与期望 (下标, 值) 序列。
    struct Case {
        slice: Vec<RecursiveSlice<&'static str>>,
        continue_vals: Vec<&'static str>,
        break_val: &'static str,
        output: Vec<(usize, &'static str)>,
    }

    let cases = vec![
        // 空输入。
        Case {
            slice: vec![],
            continue_vals: vec![],
            break_val: "",
            output: vec![],
        },
        // 仅有空嵌套，无叶子值。
        Case {
            slice: vec![
                slices(vec![values(&[]), values(&[])]),
                slices(vec![]),
                slices(vec![]),
                slices(vec![values(&[]), values(&[])]),
                slices(vec![]),
                slices(vec![]),
                slices(vec![values(&[])]),
                slices(vec![values(&[]), values(&[])]),
            ],
            continue_vals: vec![],
            break_val: "",
            output: vec![],
        },
        // 单层 Values 按序展平。
        Case {
            slice: vec![slices(vec![values(&["111", "", "333"])])],
            continue_vals: vec![],
            break_val: "",
            output: vec![(0, "111"), (1, ""), (2, "333")],
        },
        // 中间空 Slices 不占下标；后续 Values 下标接续。
        Case {
            slice: vec![
                slices(vec![]),
                slices(vec![
                    values(&["111", "", "333"]),
                    values(&[]),
                    values(&["234"]),
                ]),
                slices(vec![]),
            ],
            continue_vals: vec![],
            break_val: "",
            output: vec![(0, "111"), (1, ""), (2, "333"), (3, "234")],
        },
        // continue 跳过 "444"：下标 3 被跳过，后续仍按全局下标递增。
        Case {
            slice: vec![slices(vec![
                values(&["111", "", "333"]),
                values(&[]),
                values(&["444", "555", "666"]),
            ])],
            continue_vals: vec!["444"],
            break_val: "",
            output: vec![(0, "111"), (1, ""), (2, "333"), (4, "555"), (5, "666")],
        },
        // continue "111" 且在 "555" 处 break。
        Case {
            slice: vec![slices(vec![
                values(&["111", "", "333"]),
                slices(vec![]),
                values(&["444", "555", "666"]),
            ])],
            continue_vals: vec!["111"],
            break_val: "555",
            output: vec![(1, ""), (2, "333"), (3, "444")],
        },
        // 多顶层切片；跳过 "321"，在 "222" 处终止。
        Case {
            slice: vec![
                slices(vec![
                    values(&[]),
                    values(&["", "", "998877"]),
                    values(&[]),
                    values(&[]),
                    values(&[]),
                    values(&[]),
                    values(&["321"]),
                ]),
                slices(vec![values(&["555", "222", "1"])]),
            ],
            continue_vals: vec!["321"],
            break_val: "222",
            output: vec![(0, ""), (1, ""), (2, "998877"), (4, "555")],
        },
    ];

    for case in cases {
        let mut output = Vec::with_capacity(case.output.len());
        // 模拟消费者：按值跳过或提前结束，验证迭代器下标仍正确。
        for (index, value) in SliceRecursiveFlattenIter(&case.slice) {
            if case.continue_vals.contains(value) {
                continue;
            }
            if !case.break_val.is_empty() && *value == case.break_val {
                break;
            }
            output.push((index, *value));
        }
        assert_eq!(output, case.output);
    }
}
