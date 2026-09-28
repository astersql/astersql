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

// 向量化算术内核的 Go 对等单元测试。
//
// 对应 `builtin_arithmetic_vec.go` 的批处理路径：REAL/DECIMAL/INT 列上的
// 加减乘除、取模、整除，以及 NULL 合并、溢出与 `vectorized()` 声明。

use super::*;

/// 从字符串解析 `MyDecimal`（TiDB 定点数类型）。
fn dec(value: &str) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal.FromString(value.as_bytes()).unwrap();
    decimal
}

/// 由可选字符串列表构造 DECIMAL 向量列。
fn decimals(values: &[Option<&str>]) -> DecimalVector {
    DecimalVector::from_options(
        values
            .iter()
            .map(|value| value.map(dec))
            .collect::<Vec<_>>(),
    )
}

/// 将 DECIMAL 向量列格式化为可选十进制字符串，便于断言。
fn decimal_strings(column: &DecimalVector) -> Vec<Option<String>> {
    column
        .options()
        .into_iter()
        .map(|value| value.map(|value| value.String()))
        .collect()
}

#[test]
/// 向量化 REAL 加减乘除模：NULL、除零与溢出路径与 Go 一致。
fn vectorized_real_arithmetic_matches_go_null_zero_and_overflow_paths() {
    let ctx = EvalContext::default();
    let lhs = RealVector::from_options(vec![Some(6.0), None, Some(f64::MAX), Some(5.5)]);
    let rhs = RealVector::from_options(vec![Some(3.0), Some(2.0), Some(2.0), Some(0.0)]);

    assert_eq!(
        BuiltinArithmeticPlusRealSig
            .vec_eval_real(&ctx, &lhs, &rhs)
            .unwrap()
            .options()[..2],
        [Some(9.0), None]
    );
    assert_eq!(
        BuiltinArithmeticMinusRealSig
            .vec_eval_real(&ctx, &lhs, &rhs)
            .unwrap()
            .options()[..2],
        [Some(3.0), None]
    );
    assert!(matches!(
        BuiltinArithmeticMultiplyRealSig.vec_eval_real(&ctx, &lhs, &rhs),
        Err(ArithmeticError::Overflow {
            target_type: "DOUBLE",
            ..
        })
    ));

    let finite_lhs = RealVector::from_options(vec![Some(6.0), None, Some(5.5)]);
    let finite_rhs = RealVector::from_options(vec![Some(3.0), Some(2.0), Some(0.0)]);
    assert_eq!(
        BuiltinArithmeticDivideRealSig
            .vec_eval_real(&ctx, &finite_lhs, &finite_rhs)
            .unwrap()
            .options(),
        vec![Some(2.0), None, None]
    );
    assert_eq!(
        BuiltinArithmeticModRealSig
            .vec_eval_real(&ctx, &finite_lhs, &finite_rhs)
            .unwrap()
            .options(),
        vec![Some(0.0), None, None]
    );

    let strict = EvalContext {
        division_by_zero_as_error: true,
        ..EvalContext::default()
    };
    assert_eq!(
        BuiltinArithmeticDivideRealSig
            .vec_eval_real(&strict, &finite_lhs, &finite_rhs)
            .unwrap_err(),
        ArithmeticError::DivisionByZero
    );
}

#[test]
/// 向量化 DECIMAL 运算：舍入、NULL 与错误路径与 Go 一致。
fn vectorized_decimal_arithmetic_matches_go_rounding_nulls_and_errors() {
    let ctx = EvalContext {
        result_decimal: 2,
        div_precision_increment: 4,
        ..EvalContext::default()
    };
    let lhs = decimals(&[Some("7.50"), None, Some("5")]);
    let rhs = decimals(&[Some("2.00"), Some("3"), Some("0")]);

    assert_eq!(
        decimal_strings(
            &BuiltinArithmeticPlusDecimalSig
                .vec_eval_decimal(&ctx, &lhs, &rhs)
                .unwrap()
        ),
        vec![Some("9.50".into()), None, Some("5".into())]
    );
    assert_eq!(
        decimal_strings(
            &BuiltinArithmeticMinusDecimalSig
                .vec_eval_decimal(&ctx, &lhs, &rhs)
                .unwrap()
        ),
        vec![Some("5.50".into()), None, Some("5".into())]
    );
    assert_eq!(
        decimal_strings(
            &BuiltinArithmeticMultiplyDecimalSig
                .vec_eval_decimal(&ctx, &lhs, &rhs)
                .unwrap()
        ),
        vec![Some("15.0000".into()), None, Some("0".into())]
    );
    assert_eq!(
        decimal_strings(
            &BuiltinArithmeticDivideDecimalSig
                .vec_eval_decimal(&ctx, &lhs, &rhs)
                .unwrap()
        ),
        vec![Some("3.750000".into()), None, None]
    );
    assert_eq!(
        decimal_strings(
            &BuiltinArithmeticModDecimalSig
                .vec_eval_decimal(&ctx, &lhs, &rhs)
                .unwrap()
        ),
        vec![Some("1.50".into()), None, None]
    );

    let maximum = NewMaxOrMinDec(false, 81, 0);
    let overflow_lhs = DecimalVector::from_options(vec![Some(maximum.clone())]);
    let overflow_rhs = DecimalVector::from_options(vec![Some(maximum)]);
    assert!(matches!(
        BuiltinArithmeticPlusDecimalSig.vec_eval_decimal(&ctx, &overflow_lhs, &overflow_rhs),
        Err(ArithmeticError::Overflow {
            target_type: "DECIMAL",
            ..
        })
    ));
}

#[test]
/// 整型取模覆盖 Go 的四种有/无符号组合。
fn integer_mod_covers_all_go_signedness_combinations() {
    let ctx = EvalContext::default();
    let unsigned_lhs = IntVector::unsigned(vec![Some(u64::MAX), Some(9), None]);
    let unsigned_rhs = IntVector::unsigned(vec![Some(10), Some(0), Some(3)]);
    assert_eq!(
        BuiltinArithmeticModIntUnsignedUnsignedSig
            .vec_eval_int(&ctx, &unsigned_lhs, &unsigned_rhs)
            .unwrap()
            .options_u64(),
        vec![Some(5), None, None]
    );

    let signed_rhs = IntVector::signed(vec![Some(-10), Some(4)]);
    assert_eq!(
        BuiltinArithmeticModIntUnsignedSignedSig
            .vec_eval_int(
                &ctx,
                &IntVector::unsigned(vec![Some(u64::MAX), Some(9)]),
                &signed_rhs,
            )
            .unwrap()
            .options_u64(),
        vec![Some(5), Some(1)]
    );

    let signed_lhs = IntVector::signed(vec![Some(-9), Some(9)]);
    assert_eq!(
        BuiltinArithmeticModIntSignedUnsignedSig
            .vec_eval_int(
                &ctx,
                &signed_lhs,
                &IntVector::unsigned(vec![Some(4), Some(4)]),
            )
            .unwrap()
            .options_i64(),
        vec![Some(-1), Some(1)]
    );
    assert_eq!(
        BuiltinArithmeticModIntSignedSignedSig
            .vec_eval_int(
                &ctx,
                &IntVector::signed(vec![Some(i64::MIN), Some(-9)]),
                &IntVector::signed(vec![Some(-1), Some(4)]),
            )
            .unwrap()
            .options_i64(),
        vec![Some(0), Some(-1)]
    );
}

#[test]
/// 整型加法覆盖符号组合与溢出检查。
fn integer_plus_covers_all_go_signedness_and_overflow_checks() {
    let ctx = EvalContext::default();
    assert_eq!(
        BuiltinArithmeticPlusIntSig
            .vec_eval_int(
                &ctx,
                &IntVector::signed(vec![Some(-3), Some(4)]),
                &IntVector::signed(vec![Some(2), Some(5)]),
            )
            .unwrap()
            .options_i64(),
        vec![Some(-1), Some(9)]
    );
    assert_eq!(
        BuiltinArithmeticPlusIntSig
            .vec_eval_int(
                &ctx,
                &IntVector::unsigned(vec![Some(7)]),
                &IntVector::signed(vec![Some(-3)]),
            )
            .unwrap()
            .options_u64(),
        vec![Some(4)]
    );
    assert_eq!(
        BuiltinArithmeticPlusIntSig
            .vec_eval_int(
                &ctx,
                &IntVector::signed(vec![Some(-3)]),
                &IntVector::unsigned(vec![Some(7)]),
            )
            .unwrap()
            .options_u64(),
        vec![Some(4)]
    );
    assert!(matches!(
        BuiltinArithmeticPlusIntSig.vec_eval_int(
            &ctx,
            &IntVector::unsigned(vec![Some(u64::MAX)]),
            &IntVector::unsigned(vec![Some(1)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        BuiltinArithmeticPlusIntSig.vec_eval_int(
            &ctx,
            &IntVector::signed(vec![Some(i64::MAX)]),
            &IntVector::signed(vec![Some(1)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT",
            ..
        })
    ));
}

#[test]
/// 整型减法遵循 `no_unsigned_subtraction` SQL 模式。
fn integer_minus_honors_no_unsigned_subtraction_mode() {
    let unsigned = IntVector::unsigned(vec![Some(2), Some(8)]);
    let rhs = IntVector::unsigned(vec![Some(3), Some(3)]);
    assert!(matches!(
        BuiltinArithmeticMinusIntSig.vec_eval_int(&EvalContext::default(), &unsigned, &rhs),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT UNSIGNED",
            ..
        })
    ));

    let signed_mode = EvalContext {
        no_unsigned_subtraction: true,
        ..EvalContext::default()
    };
    assert_eq!(
        BuiltinArithmeticMinusIntSig
            .vec_eval_int(&signed_mode, &unsigned, &rhs)
            .unwrap()
            .options_i64(),
        vec![Some(-1), Some(5)]
    );
}

#[test]
/// 整型乘除边界（溢出、除零、最小负数）与 Go 一致。
fn integer_multiply_and_divide_match_go_boundaries() {
    let ctx = EvalContext::default();
    assert_eq!(
        BuiltinArithmeticMultiplyIntSig
            .vec_eval_int(
                &ctx,
                &IntVector::signed(vec![Some(-4), None]),
                &IntVector::signed(vec![Some(3), Some(i64::MAX)]),
            )
            .unwrap()
            .options_i64(),
        vec![Some(-12), None]
    );
    assert!(matches!(
        BuiltinArithmeticMultiplyIntSig.vec_eval_int(
            &ctx,
            &IntVector::signed(vec![Some(i64::MAX)]),
            &IntVector::signed(vec![Some(2)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT",
            ..
        })
    ));
    assert!(matches!(
        BuiltinArithmeticMultiplyIntUnsignedSig.vec_eval_int(
            &ctx,
            &IntVector::unsigned(vec![Some(u64::MAX)]),
            &IntVector::unsigned(vec![Some(2)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT UNSIGNED",
            ..
        })
    ));

    assert_eq!(
        BuiltinArithmeticIntDivideIntSig
            .vec_eval_int(
                &ctx,
                &IntVector::signed(vec![Some(9), Some(9), None]),
                &IntVector::signed(vec![Some(2), Some(0), Some(1)]),
            )
            .unwrap()
            .options_i64(),
        vec![Some(4), None, None]
    );
    assert!(matches!(
        BuiltinArithmeticIntDivideIntSig.vec_eval_int(
            &ctx,
            &IntVector::signed(vec![Some(i64::MIN)]),
            &IntVector::signed(vec![Some(-1)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT",
            ..
        })
    ));
    assert!(matches!(
        BuiltinArithmeticIntDivideIntSig.vec_eval_int(
            &ctx,
            &IntVector::unsigned(vec![Some(9)]),
            &IntVector::signed(vec![Some(-2)]),
        ),
        Err(ArithmeticError::Overflow {
            target_type: "BIGINT UNSIGNED",
            ..
        })
    ));
}

#[test]
/// DECIMAL 整除保留 Go 对无符号小数部分的处理规则。
fn decimal_int_divide_preserves_go_unsigned_fraction_rule() {
    let ctx = EvalContext::default();
    assert_eq!(
        BuiltinArithmeticIntDivideDecimalSig
            .vec_eval_int(
                &ctx,
                &decimals(&[Some("7.9"), Some("-0.5"), Some("5")]),
                &decimals(&[Some("2"), Some("10"), Some("0")]),
                false,
                true,
            )
            .unwrap()
            .options_u64(),
        vec![Some(3), Some(0), None]
    );
}

#[test]
/// 确认每个 Go 算术签名均声明 `vectorized() == true`。
fn every_go_signature_reports_vectorized() {
    let signatures: [&dyn Vectorized; 18] = [
        &BuiltinArithmeticMultiplyRealSig,
        &BuiltinArithmeticDivideDecimalSig,
        &BuiltinArithmeticModIntUnsignedUnsignedSig,
        &BuiltinArithmeticModIntUnsignedSignedSig,
        &BuiltinArithmeticModIntSignedUnsignedSig,
        &BuiltinArithmeticModIntSignedSignedSig,
        &BuiltinArithmeticMinusRealSig,
        &BuiltinArithmeticMinusDecimalSig,
        &BuiltinArithmeticMinusIntSig,
        &BuiltinArithmeticModRealSig,
        &BuiltinArithmeticModDecimalSig,
        &BuiltinArithmeticPlusRealSig,
        &BuiltinArithmeticMultiplyDecimalSig,
        &BuiltinArithmeticIntDivideDecimalSig,
        &BuiltinArithmeticMultiplyIntSig,
        &BuiltinArithmeticDivideRealSig,
        &BuiltinArithmeticIntDivideIntSig,
        &BuiltinArithmeticPlusIntSig,
    ];
    assert!(signatures.into_iter().all(Vectorized::vectorized));
    assert!(BuiltinArithmeticPlusDecimalSig.vectorized());
    assert!(BuiltinArithmeticMultiplyIntUnsignedSig.vectorized());
}
