// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 标量运算符内置函数的冒烟测试。
//
// 覆盖三值逻辑短路、位运算与移位边界、真值/NOT/IS NULL 契约，以及一元负号溢出。

use crate::builtin_op::*;
use rust_decimal::Decimal;

#[test]
/// 逻辑运算：短路与三值逻辑。
fn logical_operators_cover_mysql_three_valued_logic_and_short_circuit() {
    assert_eq!(
        logical_and(Ok(Some(0)), || panic!("short circuit")).unwrap(),
        Some(0)
    );
    assert_eq!(logical_and(Ok(None), || Ok(Some(1))).unwrap(), None);
    assert_eq!(logical_and(Ok(None), || Ok(Some(0))).unwrap(), Some(0));
    assert_eq!(
        logical_or(Ok(Some(1)), || panic!("short circuit")).unwrap(),
        Some(1)
    );
    assert_eq!(logical_or(Ok(None), || Ok(Some(0))).unwrap(), None);
    assert_eq!(logical_or(Ok(None), || Ok(Some(2))).unwrap(), Some(1));
    assert_eq!(logical_xor(Ok(Some(1)), || Ok(Some(0))).unwrap(), Some(1));
    assert_eq!(
        logical_xor(Ok(None), || panic!("NULL short circuit")).unwrap(),
        None
    );
    assert!(logical_and(Err(EvalError::input("lhs")), || Ok(Some(1))).is_err());
}

#[test]
/// 位运算与移位：NULL 与字宽边界。
fn bitwise_and_shift_operators_cover_null_and_word_boundaries() {
    assert_eq!(bit_and(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(2));
    assert_eq!(bit_or(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(7));
    assert_eq!(bit_xor(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(5));
    assert_eq!(bit_neg(Ok(Some(-1))).unwrap(), Some(0));
    assert_eq!(
        left_shift(Ok(Some(1)), || Ok(Some(63))).unwrap(),
        Some(i64::MIN)
    );
    assert_eq!(left_shift(Ok(Some(1)), || Ok(Some(64))).unwrap(), Some(0));
    assert_eq!(
        right_shift(Ok(Some(-1)), || Ok(Some(1))).unwrap(),
        Some(i64::MAX)
    );
    assert_eq!(bit_and(Ok(None), || Ok(Some(3))).unwrap(), None);
}

#[test]
/// IS TRUE/FALSE、NOT、IS NULL 的 NULL 契约。
fn truth_not_and_null_functions_keep_null_contracts() {
    assert_eq!(
        is_true(Ok(Some(BooleanValue::Int(0))), false).unwrap(),
        Some(0)
    );
    assert_eq!(
        is_true(Ok(Some(BooleanValue::Int(-1))), false).unwrap(),
        Some(1)
    );
    assert_eq!(is_true(Ok(None), true).unwrap(), None);
    assert_eq!(is_true(Ok(None), false).unwrap(), Some(0));
    assert_eq!(
        is_false(Ok(Some(BooleanValue::Real(0.0))), false).unwrap(),
        Some(1)
    );
    assert_eq!(
        unary_not(Ok(Some(BooleanValue::Decimal(Decimal::ZERO)))).unwrap(),
        Some(1)
    );
    assert_eq!(unary_not(Ok(None)).unwrap(), None);
    assert_eq!(is_null::<i64>(Ok(None)).unwrap(), Some(1));
    assert_eq!(is_null(Ok(Some(9))).unwrap(), Some(0));
}

#[test]
/// 一元负号：有符号/无符号/DECIMAL 与溢出。
fn unary_minus_covers_signed_unsigned_decimal_and_overflow() {
    assert_eq!(
        unary_minus_int(Ok(Some(IntValue::Signed(7)))).unwrap(),
        Some(-7)
    );
    assert_eq!(
        unary_minus_int(Ok(Some(IntValue::Unsigned(1_u64 << 63)))).unwrap(),
        Some(i64::MIN)
    );
    assert!(unary_minus_int(Ok(Some(IntValue::Signed(i64::MIN)))).is_err());
    assert!(unary_minus_int(Ok(Some(IntValue::Unsigned((1_u64 << 63) + 1)))).is_err());
    assert_eq!(
        unary_minus_decimal(Ok(Some(Decimal::ONE))).unwrap(),
        Some(-Decimal::ONE)
    );
    assert_eq!(unary_minus_decimal(Ok(None)).unwrap(), None);
}

#[test]
/// 函数类派发元数据与 Go 一致。
fn function_class_metadata_matches_go_dispatch() {
    let logic = LogicAndFunctionClass.get_function(2).unwrap();
    assert_eq!(logic.pb_code, Some(ScalarFuncSig::LogicalAnd));
    assert!(LogicAndFunctionClass.get_function(1).is_err());

    let metadata = ExprMetadata::constant(EvalType::Int, 20, 0, true);
    let signature =
        unary_minus_signature(&metadata, Some(IntValue::Unsigned((1_u64 << 63) + 1))).unwrap();
    assert_eq!(signature.return_type, EvalType::Decimal);
    assert!(signature.constant_arg_overflow);
}

#[test]
/// DECIMAL 返回宽度通过 Go 的 SetFlenUnderLimit 截断到最大精度 65。
fn unary_minus_decimal_flen_is_clamped_to_mysql_limit() {
    let decimal_constant = ExprMetadata::constant(EvalType::Decimal, 65, 2, false);
    assert_eq!(
        unary_minus_signature(&decimal_constant, None).unwrap().flen,
        65
    );

    let overflowing_integer = ExprMetadata::constant(EvalType::Int, 65, 0, true);
    assert_eq!(
        unary_minus_signature(
            &overflowing_integer,
            Some(IntValue::Unsigned((1_u64 << 63) + 1)),
        )
        .unwrap()
        .flen,
        65
    );
}
