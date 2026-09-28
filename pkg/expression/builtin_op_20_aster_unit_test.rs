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

// 标量逻辑/位运算/真值/一元负号内置函数的 Aster 单元测试。
//
// 验证 MySQL 三值逻辑真值表、短路求值边界、位运算与移位、IS TRUE/FALSE/NOT、
// 一元负号溢出，以及函数类签名选择与 Go/PB 编码一致。

use std::cell::Cell;

use crate::builtin_op::{
    BooleanValue, BuiltinLogicAndSig, BuiltinUnaryMinusDecimalSig, EvalError, EvalType,
    ExprMetadata, IntValue, IsTrueOrFalseFunctionClass, LogicAndFunctionClass, ScalarFuncSig,
    TruthOp, UnaryMinusFunctionClass, bit_and, bit_neg, bit_or, bit_xor, handle_int_overflow,
    is_false, is_null, is_true, left_shift, logical_and, logical_or, logical_xor, right_shift,
    truth_signature, unary_minus_decimal, unary_minus_int, unary_minus_signature, unary_not,
};
use rust_decimal::Decimal;
use serde_json::json;

#[test]
/// AND/OR/XOR 覆盖 NULL/0/非零组合的三值真值表。
fn logic_operators_preserve_mysql_three_valued_truth_tables() {
    let values = [None, Some(0), Some(1), Some(-1)];
    for lhs in values {
        for rhs in values {
            let expected_and = match (lhs, rhs) {
                (Some(0), _) | (_, Some(0)) => Some(0),
                (None, _) | (_, None) => None,
                _ => Some(1),
            };
            let expected_or = match (lhs, rhs) {
                (Some(v), _) if v != 0 => Some(1),
                (_, Some(v)) if v != 0 => Some(1),
                (None, _) | (_, None) => None,
                _ => Some(0),
            };
            let expected_xor = match (lhs, rhs) {
                (Some(a), Some(b)) => Some(i64::from((a != 0) ^ (b != 0))),
                _ => None,
            };

            assert_eq!(logical_and(Ok(lhs), || Ok(rhs)).unwrap(), expected_and);
            assert_eq!(logical_or(Ok(lhs), || Ok(rhs)).unwrap(), expected_or);
            assert_eq!(logical_xor(Ok(lhs), || Ok(rhs)).unwrap(), expected_xor);
        }
    }
}

#[test]
/// 左操作数已决定结果时不求右操作数（含错误与 panic 探测）。
fn binary_logic_preserves_each_go_rhs_evaluation_boundary() {
    let evaluated = Cell::new(0);
    assert_eq!(
        logical_and(Ok(Some(0)), || {
            evaluated.set(evaluated.get() + 1);
            Err(EvalError::input("rhs"))
        })
        .unwrap(),
        Some(0)
    );
    assert_eq!(
        logical_or(Ok(Some(1)), || {
            evaluated.set(evaluated.get() + 1);
            Err(EvalError::input("rhs"))
        })
        .unwrap(),
        Some(1)
    );
    assert_eq!(evaluated.get(), 0);
    assert!(logical_xor(Ok(Some(1)), || Err(EvalError::input("rhs"))).is_err());
    assert_eq!(
        logical_xor(Ok(None), || panic!("rhs must not run")).unwrap(),
        None
    );
}

#[test]
/// 按位与/或/异或/取反与按无符号移位；NULL 短路。
fn bit_operators_match_go_uint64_shift_and_null_semantics() {
    assert_eq!(bit_and(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(2));
    assert_eq!(bit_or(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(7));
    assert_eq!(bit_xor(Ok(Some(6)), || Ok(Some(3))).unwrap(), Some(5));
    assert_eq!(bit_neg(Ok(Some(-1))).unwrap(), Some(0));
    assert_eq!(left_shift(Ok(Some(-1)), || Ok(Some(1))).unwrap(), Some(-2));
    assert_eq!(
        right_shift(Ok(Some(-1)), || Ok(Some(1))).unwrap(),
        Some(i64::MAX)
    );
    assert_eq!(left_shift(Ok(Some(1)), || Ok(Some(64))).unwrap(), Some(0));
    assert_eq!(right_shift(Ok(Some(1)), || Ok(Some(-1))).unwrap(), Some(0));
    assert_eq!(bit_and(Ok(None), || Ok(Some(3))).unwrap(), None);
    assert_eq!(
        bit_or(Ok(None), || panic!("rhs must not run")).unwrap(),
        None
    );
}

#[test]
/// 各值族零值判定与 keep_null 对 NULL 的不同处理。
fn truth_false_and_not_cover_all_go_value_families() {
    let zeroes = [
        BooleanValue::Int(0),
        BooleanValue::Real(0.0),
        BooleanValue::Decimal(Decimal::ZERO),
        BooleanValue::VectorFloat32(vec![0.0, -0.0]),
        BooleanValue::Json(json!(0)),
    ];
    for zero in zeroes {
        assert_eq!(is_true(Ok(Some(zero.clone())), false).unwrap(), Some(0));
        assert_eq!(is_false(Ok(Some(zero.clone())), false).unwrap(), Some(1));
        assert_eq!(unary_not(Ok(Some(zero))).unwrap(), Some(1));
    }
    assert_eq!(is_true(Ok(None), false).unwrap(), Some(0));
    assert_eq!(is_true(Ok(None), true).unwrap(), None);
    assert_eq!(is_false(Ok(None), false).unwrap(), Some(0));
    assert_eq!(unary_not(Ok(None)).unwrap(), None);
    assert_eq!(
        unary_not(Ok(Some(BooleanValue::Json(json!(false))))).unwrap(),
        Some(0)
    );
}

#[test]
/// 一元负号：有符号/无符号边界与 DECIMAL 取负。
fn unary_minus_preserves_signed_unsigned_and_decimal_boundaries() {
    assert!(!handle_int_overflow(IntValue::Signed(7)));
    assert!(handle_int_overflow(IntValue::Signed(i64::MIN)));
    assert!(!handle_int_overflow(IntValue::Unsigned(1_u64 << 63)));
    assert!(handle_int_overflow(IntValue::Unsigned((1_u64 << 63) + 1)));

    assert_eq!(
        unary_minus_int(Ok(Some(IntValue::Signed(7)))).unwrap(),
        Some(-7)
    );
    assert_eq!(
        unary_minus_int(Ok(Some(IntValue::Unsigned(1_u64 << 63)))).unwrap(),
        Some(i64::MIN)
    );
    assert!(matches!(
        unary_minus_int(Ok(Some(IntValue::Signed(i64::MIN)))),
        Err(EvalError::Overflow {
            type_name: "BIGINT",
            ..
        })
    ));
    assert_eq!(
        unary_minus_decimal(Ok(Some(Decimal::ONE))).unwrap(),
        Some(-Decimal::ONE)
    );
    assert_eq!(unary_minus_decimal(Ok(None)).unwrap(), None);
}

#[test]
/// 真值与一元负号签名：类型提升、PB 码与 flen。
fn signature_selection_matches_go_coercion_pb_codes_and_widths() {
    let truth = truth_signature(TruthOp::IsTruth, EvalType::String, true).unwrap();
    assert_eq!(truth.argument_type, EvalType::Real);
    assert_eq!(truth.pb_code, Some(ScalarFuncSig::RealIsTrueWithNull));
    assert_eq!(truth.flen, 1);

    let vector = truth_signature(TruthOp::IsFalsity, EvalType::VectorFloat32, false).unwrap();
    assert_eq!(vector.argument_type, EvalType::VectorFloat32);
    assert_eq!(vector.pb_code, None);

    let signed_column = ExprMetadata::column(EvalType::Int, 20, 0, false);
    let signed = unary_minus_signature(&signed_column, None).unwrap();
    assert_eq!(signed.return_type, EvalType::Int);
    assert_eq!(signed.flen, 20);

    let unsigned_constant = ExprMetadata::constant(EvalType::Int, 20, 0, true);
    let decimal = unary_minus_signature(
        &unsigned_constant,
        Some(IntValue::Unsigned((1_u64 << 63) + 1)),
    )
    .unwrap();
    assert_eq!(decimal.return_type, EvalType::Decimal);
    assert_eq!(decimal.pb_code, Some(ScalarFuncSig::UnaryMinusDecimal));
    assert_eq!(decimal.flen, 21);
}

#[test]
/// IS NULL 结果非 NULL；参数求值错误向上传播。
fn is_null_returns_a_non_null_boolean_and_propagates_errors() {
    assert_eq!(is_null::<i64>(Ok(None)).unwrap(), Some(1));
    assert_eq!(is_null(Ok(Some(9))).unwrap(), Some(0));
    assert!(is_null::<i64>(Err(EvalError::input("value"))).is_err());
}

#[test]
/// 函数类/签名门面与 Go 一一对应，且可 Clone。
fn go_function_class_and_signature_facades_remain_one_to_one_and_cloneable() {
    let class = LogicAndFunctionClass;
    assert_eq!(
        class.get_function(2).unwrap().pb_code,
        Some(ScalarFuncSig::LogicalAnd)
    );
    assert!(class.get_function(1).is_err());
    assert_eq!(
        BuiltinLogicAndSig
            .eval_int(Ok(Some(1)), || Ok(Some(1)))
            .unwrap(),
        Some(1)
    );

    let truth_class = IsTrueOrFalseFunctionClass {
        operation: TruthOp::IsTruth,
        keep_null: true,
    };
    assert_eq!(truth_class.get_display_name(), "IS TRUE");
    assert_eq!(
        truth_class
            .get_function(1, EvalType::Decimal)
            .unwrap()
            .pb_code,
        Some(ScalarFuncSig::DecimalIsTrueWithNull)
    );

    let minus_class = UnaryMinusFunctionClass;
    let metadata = ExprMetadata::constant(EvalType::Int, 20, 0, false);
    assert_eq!(
        minus_class.type_infer(&metadata, Some(IntValue::Signed(i64::MIN))),
        (EvalType::Decimal, true)
    );
    let sig = BuiltinUnaryMinusDecimalSig {
        constant_arg_overflow: true,
    };
    assert!(sig.clone().constant_arg_overflow);
}
