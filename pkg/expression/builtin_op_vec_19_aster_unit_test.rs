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

// 运算符向量化内核的 Aster 单元测试。
//
// 在真实 chunk `Column` 上验证三值逻辑、右参数告警回退标量路径、位运算/移位、
// 真值与 IS NULL、一元负号溢出，以及列长不匹配错误。

use std::cell::Cell;

use crate::builtin_op_vec::*;
use crate::types_test_support::decimal::mydecimal::MyDecimal;
use crate::util_chunk::Column;

/// 由可选 i64 序列构造 Int64 列（含 NULL）。
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

/// 由可选 u64 序列构造 Uint64 列。
fn uint_column(values: &[Option<u64>]) -> Column {
    let mut column = Column::default();
    column.ResizeUint64(0, false);
    for value in values {
        match value {
            Some(value) => column.AppendUint64(*value),
            None => column.AppendNull(),
        }
    }
    column
}

/// 由可选 f64 序列构造 Float64 列。
fn real_column(values: &[Option<f64>]) -> Column {
    let mut column = Column::default();
    column.ResizeFloat64(0, false);
    for value in values {
        match value {
            Some(value) => column.AppendFloat64(*value),
            None => column.AppendNull(),
        }
    }
    column
}

/// 从文本解析 MyDecimal。
fn decimal(text: &str) -> MyDecimal {
    let mut value = MyDecimal::default();
    value.FromString(text.as_bytes()).expect("valid decimal");
    value
}

/// 由可选十进制文本构造 Decimal 列。
fn decimal_column(values: &[Option<&str>]) -> Column {
    let mut column = Column::default();
    column.ResizeDecimal(0, false);
    for value in values {
        match value {
            Some(value) => column.AppendMyDecimal(&decimal(value)),
            None => column.AppendNull(),
        }
    }
    column
}

/// 将 Int64 列还原为 Option 向量便于断言。
fn int_values(column: &Column) -> Vec<Option<i64>> {
    (0..column.Rows())
        .map(|row| (!column.IsNull(row)).then(|| column.GetInt64(row)))
        .collect()
}

#[test]
/// 向量化 AND/OR/XOR 保持 Go 三值真值表与行序。
fn go_three_valued_logic_truth_table_is_preserved() {
    let lhs = int_column(&[
        None,
        Some(0),
        None,
        Some(1),
        None,
        Some(0),
        Some(0),
        Some(1),
        Some(1),
        Some(-1),
    ]);
    let rhs = int_column(&[
        None,
        None,
        Some(0),
        None,
        Some(1),
        Some(0),
        Some(1),
        Some(0),
        Some(1),
        Some(1),
    ]);

    assert_eq!(
        int_values(&vec_logic_or(&lhs, &rhs).expect("logic OR")),
        vec![
            None,
            None,
            None,
            Some(1),
            Some(1),
            Some(0),
            Some(1),
            Some(1),
            Some(1),
            Some(1)
        ]
    );
    assert_eq!(
        int_values(&vec_logic_and(&lhs, &rhs).expect("logic AND")),
        vec![
            None,
            Some(0),
            Some(0),
            None,
            None,
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1)
        ]
    );
    assert_eq!(
        int_values(&vec_logic_xor(&lhs, &rhs).expect("logic XOR")),
        vec![
            None,
            None,
            None,
            None,
            None,
            Some(0),
            Some(1),
            Some(1),
            Some(0),
            Some(0)
        ]
    );
}

#[test]
/// 右操作数转换产生 warning/错误时丢弃向量告警并走标量回退。
fn warning_or_error_in_second_logic_argument_uses_scalar_fallback() {
    let mut ctx = WarningContext::default();
    ctx.append_warning("existing");
    let fallback_called = Cell::new(false);

    let result = vec_logic_or_with_fallback(
        &mut ctx,
        |ctx| {
            ctx.append_warning("lhs conversion");
            Ok(int_column(&[Some(0), None]))
        },
        |ctx| {
            ctx.append_warning("rhs conversion");
            Ok(int_column(&[Some(1), Some(0)]))
        },
        |_ctx| {
            fallback_called.set(true);
            Ok(int_column(&[Some(1), None]))
        },
    )
    .expect("fallback evaluation");

    assert!(fallback_called.get());
    assert_eq!(ctx.warning_count(), 1);
    assert_eq!(int_values(&result), vec![Some(1), None]);

    let and_result = vec_logic_and_with_fallback(
        &mut ctx,
        |_ctx| Ok(int_column(&[Some(1)])),
        |_ctx| Err(EvalError::Evaluation("rhs failed".into())),
        |_ctx| Ok(int_column(&[Some(0)])),
    )
    .expect("error also falls back");
    assert_eq!(int_values(&and_result), vec![Some(0)]);
}

#[test]
/// 位运算、移位与一元 NOT 的逐行结果对齐 Go。
fn bit_operators_shifts_and_unary_not_match_go_rows() {
    let lhs = int_column(&[Some(-1), Some(6), None, Some(1), Some(-8)]);
    let rhs = int_column(&[Some(1), Some(3), Some(2), Some(64), Some(1)]);

    assert_eq!(
        int_values(&vec_bit_or(&lhs, &rhs).unwrap()),
        vec![Some(-1), Some(7), None, Some(65), Some(-7)]
    );
    assert_eq!(
        int_values(&vec_bit_xor(&lhs, &rhs).unwrap()),
        vec![Some(-2), Some(5), None, Some(65), Some(-7)]
    );
    assert_eq!(
        int_values(&vec_bit_and(&lhs, &rhs).unwrap()),
        vec![Some(1), Some(2), None, Some(0), Some(0)]
    );
    assert_eq!(
        int_values(&vec_left_shift(&lhs, &rhs).unwrap()),
        vec![Some(-2), Some(48), None, Some(0), Some(-16)]
    );
    assert_eq!(
        int_values(&vec_right_shift(&lhs, &rhs).unwrap()),
        vec![
            Some(i64::MAX),
            Some(0),
            None,
            Some(0),
            Some(9_223_372_036_854_775_804)
        ]
    );
    assert_eq!(
        int_values(&vec_bit_neg(&lhs)),
        vec![Some(0), Some(-7), None, Some(-2), Some(7)]
    );
    assert_eq!(
        int_values(&vec_unary_not_int(&lhs)),
        vec![Some(0), Some(0), None, Some(0), Some(0)]
    );
}

#[test]
/// 各类型 IS TRUE/FALSE 与 IS NULL 的 keep_null 规则。
fn truth_falsity_and_is_null_keep_go_null_rules_for_all_kinds() {
    let ints = int_column(&[None, Some(0), Some(-2)]);
    let reals = real_column(&[None, Some(0.0), Some(-0.5)]);
    let decimals = decimal_column(&[None, Some("0.000"), Some("-2.5")]);

    assert!(vectorized());
    assert_eq!(
        int_values(&vec_int_is_true(&ints, false)),
        vec![Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        int_values(&vec_int_is_true(&ints, true)),
        vec![None, Some(0), Some(1)]
    );
    assert_eq!(
        int_values(&vec_int_is_false(&ints, false)),
        vec![Some(0), Some(1), Some(0)]
    );
    assert_eq!(
        int_values(&vec_real_is_true(&reals, false)),
        vec![Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        int_values(&vec_real_is_false(&reals, true)),
        vec![None, Some(1), Some(0)]
    );
    assert_eq!(
        int_values(&vec_decimal_is_true(&decimals, false)),
        vec![Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        int_values(&vec_decimal_is_false(&decimals, true)),
        vec![None, Some(1), Some(0)]
    );
    assert_eq!(
        int_values(&vec_unary_not_real(&reals)),
        vec![None, Some(1), Some(0)]
    );
    assert_eq!(
        int_values(&vec_unary_not_decimal(&decimals)),
        vec![None, Some(1), Some(0)]
    );
    assert_eq!(
        int_values(&vec_time_is_null(&ints)),
        vec![Some(1), Some(0), Some(0)]
    );
    assert_eq!(
        int_values(&vec_int_is_null(&ints)),
        vec![Some(1), Some(0), Some(0)]
    );
    assert_eq!(
        int_values(&vec_real_is_null(&reals)),
        vec![Some(1), Some(0), Some(0)]
    );
    assert_eq!(
        int_values(&vec_decimal_is_null(&decimals)),
        vec![Some(1), Some(0), Some(0)]
    );
    assert_eq!(
        int_values(&vec_duration_is_null(&ints)),
        vec![Some(1), Some(0), Some(0)]
    );
}

#[test]
/// 一元负号：NULL 保留与有符号/无符号溢出报错。
fn unary_minus_preserves_nulls_and_reports_signed_and_unsigned_overflow() {
    let signed = int_column(&[Some(233_333), None, Some(i64::MIN)]);
    let error = vec_unary_minus_int(&signed, false)
        .err()
        .expect("signed MIN must overflow");
    assert_eq!(
        error.to_string(),
        "BIGINT value -(-9223372036854775808) is out of range"
    );

    let signed_ok = int_column(&[Some(233_333), None, Some(-7)]);
    assert_eq!(
        int_values(&vec_unary_minus_int(&signed_ok, false).unwrap()),
        vec![Some(-233_333), None, Some(7)]
    );

    let unsigned_ok = uint_column(&[Some(233_333), None, Some(1u64 << 63)]);
    assert_eq!(
        int_values(&vec_unary_minus_int(&unsigned_ok, true).unwrap()),
        vec![Some(-233_333), None, Some(i64::MIN)]
    );
    let unsigned_overflow = uint_column(&[Some((1u64 << 63) + 1)]);
    assert!(vec_unary_minus_int(&unsigned_overflow, true).is_err());

    let reals = real_column(&[Some(3.5), None, Some(-0.0)]);
    let real_result = vec_unary_minus_real(&reals);
    assert_eq!(real_result.GetFloat64(0), -3.5);
    assert!(real_result.IsNull(1));
    assert_eq!(real_result.GetFloat64(2).to_bits(), 0.0f64.to_bits());

    let decimals = decimal_column(&[Some("12.50"), None, Some("-3")]);
    let decimal_result = vec_unary_minus_decimal(&decimals);
    assert_eq!(decimal_result.GetDecimal(0).String(), "-12.50");
    assert!(decimal_result.IsNull(1));
    assert_eq!(decimal_result.GetDecimal(2).String(), "3");
}

#[test]
/// 双列长度不一致时报错，绝不截断行。
fn mismatched_binary_columns_fail_instead_of_truncating_rows() {
    let error = vec_bit_or(&int_column(&[Some(1)]), &int_column(&[Some(1), Some(2)]))
        .err()
        .expect("length mismatch");
    assert_eq!(error, EvalError::ColumnLengthMismatch { left: 1, right: 2 });
}
