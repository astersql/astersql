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

// 标量算术内核的 Go 对等单元测试。
//
// 覆盖 `builtin_arithmetic.rs` 中类型推导、flen/decimal 规则、签名分派，
// 以及加减乘除、整除、取模在有/无符号、REAL、DECIMAL、向量等路径上的溢出与 NULL 语义。

use std::str::FromStr;

use super::*;
use bigdecimal::BigDecimal;

/// 构造有符号整型常量表达式。
fn int(value: i64) -> Expression {
    Expression::signed(value)
}

/// 构造无符号整型常量表达式。
fn uint(value: u64) -> Expression {
    Expression::unsigned(value)
}

/// 由字符串解析 DECIMAL 常量，并带上显示宽度 flen 与小数位 scale。
fn decimal(value: &str, flen: i32, scale: i32) -> Expression {
    Expression::decimal(BigDecimal::from_str(value).unwrap(), flen, scale)
}

/// 构建算术表达式并在默认上下文中求值。
fn eval(op: ArithmeticOp, lhs: Expression, rhs: Expression) -> Result<EvalValue, ArithmeticError> {
    ArithmeticExpr::build(op, lhs, rhs, &EvalContext::default())?.eval(&mut EvalContext::default())
}

#[test]
/// 校验数值上下文结果类型：时间/二进制字面量/BIT/混合类型等规则与 Go 一致。
fn numeric_context_type_matches_temporal_binary_bit_and_hybrid_rules() {
    assert_eq!(
        numeric_context_result_type(&Expression::temporal(0)),
        EvalType::Int
    );
    assert_eq!(
        numeric_context_result_type(&Expression::temporal(3)),
        EvalType::Decimal
    );
    assert_eq!(
        numeric_context_result_type(&Expression::binary_literal(0x1234)),
        EvalType::Int
    );
    assert_eq!(
        numeric_context_result_type(&Expression::bit_literal(3)),
        EvalType::Int
    );
    assert_eq!(
        numeric_context_result_type(&Expression::binary_string(vec![1, 2])),
        EvalType::Real
    );
    assert_eq!(
        numeric_context_result_type(&Expression::hybrid_string("12")),
        EvalType::Real
    );
}

#[test]
/// 校验加减乘结果字段的 flen/decimal 上限与 Go 规则一致。
fn flen_and_decimal_follow_go_add_subtract_and_multiply_limits() {
    let a = FieldType::decimal(3, 1);
    let b = FieldType::decimal(2, 0);
    assert_eq!(
        set_flen_decimal_for_real_or_decimal(&a, &b, true, false),
        FieldType::real(4, 1)
    );
    assert_eq!(
        set_flen_decimal_for_real_or_decimal(&a, &b, true, true),
        FieldType::real(5, 1)
    );

    let wide = FieldType::decimal(65, 0);
    assert_eq!(
        set_flen_decimal_for_real_or_decimal(&a, &wide, true, false).flen,
        MAX_REAL_WIDTH
    );
    assert_eq!(
        set_flen_decimal_for_real_or_decimal(&a, &wide, false, true).flen,
        MAX_DECIMAL_WIDTH
    );

    let unspecified = FieldType::decimal(UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH);
    let result = set_flen_decimal_for_real_or_decimal(&a, &unspecified, true, false);
    assert_eq!(
        (result.flen, result.decimal),
        (UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH)
    );
}

#[test]
/// 校验签名分派优先级与无符号等标志保留方式与 Go 一致。
fn signature_dispatch_preserves_go_precedence_and_flags() {
    let ctx = EvalContext::default();
    let plus =
        ArithmeticExpr::build(ArithmeticOp::Plus, int(1), decimal("2.0", 2, 1), &ctx).unwrap();
    assert_eq!(plus.signature(), Signature::PlusDecimal);

    let multiply =
        ArithmeticExpr::build(ArithmeticOp::Multiply, Expression::real(1.0), int(2), &ctx).unwrap();
    assert_eq!(multiply.signature(), Signature::MultiplyReal);

    let vector = ArithmeticExpr::build(
        ArithmeticOp::Plus,
        Expression::vector(vec![1.0]),
        Expression::vector(vec![2.0]),
        &ctx,
    )
    .unwrap();
    assert_eq!(vector.signature(), Signature::PlusVectorFloat32);

    let unsigned_minus = ArithmeticExpr::build(ArithmeticOp::Minus, uint(2), int(1), &ctx).unwrap();
    assert!(unsigned_minus.result_type().unsigned);
    let signed_ctx = EvalContext {
        no_unsigned_subtraction: true,
        ..EvalContext::default()
    };
    let signed_minus =
        ArithmeticExpr::build(ArithmeticOp::Minus, uint(2), int(1), &signed_ctx).unwrap();
    assert!(!signed_minus.result_type().unsigned);
}

#[test]
/// 整型加法：NULL 传播及四种有/无符号组合的溢出检查。
fn integer_plus_propagates_null_and_checks_all_signedness_combinations() {
    assert_eq!(
        eval(ArithmeticOp::Plus, int(12), int(1)).unwrap(),
        EvalValue::Int(13)
    );
    assert_eq!(
        eval(
            ArithmeticOp::Plus,
            Expression::null(FieldType::int(false)),
            int(1)
        )
        .unwrap(),
        EvalValue::Null
    );

    assert!(matches!(
        eval(ArithmeticOp::Plus, int(i64::MAX), int(1)),
        Err(ArithmeticError::Overflow { ty: "BIGINT", .. })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Plus, uint(u64::MAX), uint(1)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Plus, uint(0), int(-1)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Plus, int(-2), uint(1)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert_eq!(
        eval(ArithmeticOp::Plus, uint(u64::MAX - 1), int(1)).unwrap(),
        EvalValue::UInt(u64::MAX)
    );
}

#[test]
/// REAL/DECIMAL 加减及表达式 clone 结果与 Go 对齐。
fn real_and_decimal_plus_minus_and_clone_keep_go_results() {
    assert_eq!(
        eval(
            ArithmeticOp::Plus,
            Expression::real(1.01001),
            Expression::real(-0.01)
        )
        .unwrap(),
        EvalValue::Real(1.00001)
    );
    assert_eq!(
        eval(
            ArithmeticOp::Minus,
            Expression::real(1.01001),
            Expression::real(-0.01)
        )
        .unwrap(),
        EvalValue::Real(1.02001)
    );
    assert_eq!(
        eval(
            ArithmeticOp::Plus,
            decimal("1.20", 3, 2),
            decimal("2.3", 2, 1)
        )
        .unwrap(),
        EvalValue::Decimal(BigDecimal::from_str("3.50").unwrap())
    );
    let expression = ArithmeticExpr::build(
        ArithmeticOp::Minus,
        decimal("5.25", 3, 2),
        decimal("2.00", 3, 2),
        &EvalContext::default(),
    )
    .unwrap();
    assert_eq!(
        expression
            .clone()
            .eval(&mut EvalContext::default())
            .unwrap(),
        EvalValue::Decimal(BigDecimal::from_str("3.25").unwrap())
    );
}

#[test]
/// 整型减法：`no_unsigned_subtraction` 模式与溢出矩阵。
fn integer_minus_matches_no_unsigned_subtraction_and_go_overflow_matrix() {
    assert_eq!(
        eval(ArithmeticOp::Minus, int(12), int(1)).unwrap(),
        EvalValue::Int(11)
    );
    assert!(matches!(
        eval(ArithmeticOp::Minus, uint(0), uint(1)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Minus, uint(u64::MAX), int(-1)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Minus, int(i64::MIN), uint(1)),
        Err(ArithmeticError::Overflow { .. })
    ));

    let mut signed_ctx = EvalContext {
        no_unsigned_subtraction: true,
        ..EvalContext::default()
    };
    let expr = ArithmeticExpr::build(ArithmeticOp::Minus, uint(2), uint(3), &signed_ctx).unwrap();
    assert_eq!(expr.eval(&mut signed_ctx).unwrap(), EvalValue::Int(-1));
    assert!(
        ArithmeticExpr::build(ArithmeticOp::Minus, uint(u64::MAX), int(0), &signed_ctx)
            .unwrap()
            .eval(&mut signed_ctx)
            .is_err()
    );
}

#[test]
/// 乘法覆盖有/无符号、REAL、DECIMAL 与向量路径。
fn multiplication_covers_signed_unsigned_real_decimal_and_vector_paths() {
    assert_eq!(
        eval(ArithmeticOp::Multiply, int(-7), int(6)).unwrap(),
        EvalValue::Int(-42)
    );
    assert!(matches!(
        eval(ArithmeticOp::Multiply, int(i64::MIN), int(-1)),
        Err(ArithmeticError::Overflow { .. })
    ));
    assert!(matches!(
        eval(ArithmeticOp::Multiply, uint(u64::MAX), uint(2)),
        Err(ArithmeticError::Overflow { .. })
    ));
    assert!(matches!(
        eval(
            ArithmeticOp::Multiply,
            Expression::real(f64::MAX),
            Expression::real(2.0)
        ),
        Err(ArithmeticError::Overflow { ty: "DOUBLE", .. })
    ));

    assert_eq!(
        eval(
            ArithmeticOp::Multiply,
            decimal("1.25", 3, 2),
            decimal("2.4", 2, 1)
        )
        .unwrap(),
        EvalValue::Decimal(BigDecimal::from_str("3.000").unwrap())
    );
    assert_eq!(
        eval(
            ArithmeticOp::Multiply,
            Expression::vector(vec![1.0, -2.0]),
            Expression::vector(vec![3.0, 4.0])
        )
        .unwrap(),
        EvalValue::Vector(vec![3.0, -8.0])
    );
}

#[test]
/// 除法：除零策略、溢出与 DECIMAL 小数位精度增量。
fn divide_preserves_division_by_zero_policy_overflow_and_decimal_scale() {
    let mut warning_ctx = EvalContext::default();
    let divide_zero = ArithmeticExpr::build(
        ArithmeticOp::Divide,
        Expression::real(1.0),
        Expression::real(0.0),
        &warning_ctx,
    )
    .unwrap();
    assert_eq!(divide_zero.eval(&mut warning_ctx).unwrap(), EvalValue::Null);
    assert_eq!(
        warning_ctx.warnings,
        vec![ArithmeticWarning::DivisionByZero]
    );

    let mut error_ctx = EvalContext {
        division_by_zero_as_error: true,
        ..EvalContext::default()
    };
    assert!(matches!(
        divide_zero.eval(&mut error_ctx),
        Err(ArithmeticError::DivisionByZero)
    ));
    assert!(matches!(
        eval(
            ArithmeticOp::Divide,
            Expression::real(f64::MAX),
            Expression::real(0.5)
        ),
        Err(ArithmeticError::Overflow { ty: "DOUBLE", .. })
    ));

    let mut decimal_ctx = EvalContext {
        div_precision_increment: 4,
        ..EvalContext::default()
    };
    let quotient = ArithmeticExpr::build(
        ArithmeticOp::Divide,
        decimal("1", 1, 0),
        decimal("8", 1, 0),
        &decimal_ctx,
    )
    .unwrap();
    assert_eq!(quotient.result_type().decimal, 4);
    assert_eq!(
        quotient.eval(&mut decimal_ctx).unwrap(),
        EvalValue::Decimal(BigDecimal::from_str("0.1250").unwrap())
    );
}

#[test]
/// 整除（DIV）：四种符号组合与最小值溢出边界。
fn integer_division_handles_four_signedness_pairs_and_minimum_overflow() {
    assert_eq!(
        eval(ArithmeticOp::IntDivide, uint(7), uint(2)).unwrap(),
        EvalValue::UInt(3)
    );
    assert_eq!(
        eval(ArithmeticOp::IntDivide, uint(1), int(-2)).unwrap(),
        EvalValue::UInt(0)
    );
    assert_eq!(
        eval(ArithmeticOp::IntDivide, int(-1), uint(2)).unwrap(),
        EvalValue::UInt(0)
    );
    assert!(matches!(
        eval(ArithmeticOp::IntDivide, uint(7), int(-2)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert!(matches!(
        eval(ArithmeticOp::IntDivide, int(-7), uint(2)),
        Err(ArithmeticError::Overflow {
            ty: "BIGINT UNSIGNED",
            ..
        })
    ));
    assert_eq!(
        eval(ArithmeticOp::IntDivide, int(-7), int(2)).unwrap(),
        EvalValue::Int(-3)
    );
    assert!(matches!(
        eval(ArithmeticOp::IntDivide, int(i64::MIN), int(-1)),
        Err(ArithmeticError::Overflow { ty: "BIGINT", .. })
    ));
}

#[test]
/// DECIMAL 整除向零截断，并保留无符号边界行为。
fn decimal_integer_division_truncates_toward_zero_and_preserves_unsigned_edge() {
    assert_eq!(
        eval(
            ArithmeticOp::IntDivide,
            decimal("7.9", 2, 1),
            decimal("2", 1, 0)
        )
        .unwrap(),
        EvalValue::Int(3)
    );
    assert_eq!(
        eval(
            ArithmeticOp::IntDivide,
            Expression::unsigned_decimal("-0.5", 2, 1),
            decimal("1", 1, 0)
        )
        .unwrap(),
        EvalValue::UInt(0)
    );
}

#[test]
/// 取模保留符号规则与除零处理。
fn modulo_preserves_sign_rules_and_division_by_zero_handling() {
    assert_eq!(
        eval(ArithmeticOp::Mod, uint(7), uint(3)).unwrap(),
        EvalValue::UInt(1)
    );
    assert_eq!(
        eval(ArithmeticOp::Mod, uint(7), int(-3)).unwrap(),
        EvalValue::UInt(1)
    );
    assert_eq!(
        eval(ArithmeticOp::Mod, int(-7), uint(3)).unwrap(),
        EvalValue::Int(-1)
    );
    assert_eq!(
        eval(ArithmeticOp::Mod, int(-7), int(3)).unwrap(),
        EvalValue::Int(-1)
    );
    assert_eq!(
        eval(ArithmeticOp::Mod, decimal("7.5", 2, 1), decimal("2", 1, 0)).unwrap(),
        EvalValue::Decimal(BigDecimal::from_str("1.5").unwrap())
    );

    let mut ctx = EvalContext::default();
    let expr = ArithmeticExpr::build(ArithmeticOp::Mod, int(1), int(0), &ctx).unwrap();
    assert_eq!(expr.eval(&mut ctx).unwrap(), EvalValue::Null);
    assert_eq!(ctx.warnings, vec![ArithmeticWarning::DivisionByZero]);
}

#[test]
/// 向量维度不匹配时报错，错误信息保留操作数文本。
fn vector_dimension_errors_and_error_messages_keep_operand_text() {
    let err = eval(
        ArithmeticOp::Plus,
        Expression::vector(vec![1.0]),
        Expression::vector(vec![2.0, 3.0]),
    )
    .unwrap_err();
    assert_eq!(err, ArithmeticError::VectorDimension { left: 1, right: 2 });

    let err = eval(
        ArithmeticOp::Plus,
        Expression::named_signed(i64::MAX, "t.a"),
        int(1),
    )
    .unwrap_err();
    assert!(err.to_string().contains("(t.a + 1)"));
}
