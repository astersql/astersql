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

// 运算符向量化模块的冒烟测试。
//
// 覆盖逻辑三值结果、位运算/移位边界、一元负号溢出、真值与列长错误。

use crate::builtin_op_vec::*;
use crate::util_chunk::Column;

/// 测试辅助：构造 Int64 列。
fn int_column(values: &[Option<i64>]) -> Column {
    let mut column = Column::default();
    column.ResizeInt64(0, false);
    for value in values {
        match value {
            Some(value) => column.AppendInt64(*value),
            None => column.AppendNull(),
        }
    }
    column
}

/// 测试辅助：读出 Int64 列内容。
fn int_values(column: &Column) -> Vec<Option<i64>> {
    (0..column.Rows())
        .map(|row| (!column.IsNull(row)).then(|| column.GetInt64(row)))
        .collect()
}

#[test]
/// 向量逻辑运算保持行序与三值结果。
fn vectorized_logic_preserves_row_order_and_three_valued_results() {
    let left = int_column(&[Some(0), Some(1), None, None]);
    let right = int_column(&[None, None, Some(0), Some(1)]);
    assert_eq!(
        int_values(&vec_logic_and(&left, &right).unwrap()),
        vec![Some(0), None, Some(0), None]
    );
    assert_eq!(
        int_values(&vec_logic_or(&left, &right).unwrap()),
        vec![None, Some(1), None, Some(1)]
    );
    assert_eq!(
        int_values(&vec_logic_xor(&left, &right).unwrap()),
        vec![None, None, None, None]
    );
}

#[test]
/// 位运算、移位与按位取反边界。
fn vectorized_bit_shift_and_unary_paths_cover_boundaries() {
    let left = int_column(&[Some(6), Some(-1), None, Some(1)]);
    let right = int_column(&[Some(3), Some(1), Some(2), Some(64)]);
    assert_eq!(
        int_values(&vec_bit_and(&left, &right).unwrap()),
        vec![Some(2), Some(1), None, Some(0)]
    );
    assert_eq!(
        int_values(&vec_left_shift(&left, &right).unwrap()),
        vec![Some(48), Some(-2), None, Some(0)]
    );
    assert_eq!(
        int_values(&vec_right_shift(&left, &right).unwrap()),
        vec![Some(0), Some(i64::MAX), None, Some(0)]
    );
    assert_eq!(
        int_values(&vec_bit_neg(&left)),
        vec![Some(-7), Some(0), None, Some(-2)]
    );
}

#[test]
/// 一元负号溢出、IS TRUE/IS NULL 与列长不匹配。
fn vectorized_unary_minus_truth_and_length_error_match_go() {
    let values = int_column(&[Some(7), Some(0), None, Some(i64::MIN)]);
    assert!(vec_unary_minus_int(&values, false).is_err());
    assert_eq!(
        int_values(&vec_int_is_true(&values, true)),
        vec![Some(1), Some(0), None, Some(1)]
    );
    assert_eq!(
        int_values(&vec_is_null(&values)),
        vec![Some(0), Some(0), Some(1), Some(0)]
    );
    assert!(vec_bit_or(&values, &int_column(&[Some(1)])).is_err());
    assert!(vectorized());
}
