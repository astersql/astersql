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

// `builtin_other` 标量语义的 Aster 对齐单元测试。
//
// 相对 `builtin_other_test.rs` 覆盖更广：BIT_COUNT 多类型转换、IN 常量缓存与
// BIT 列剪枝、各 EvalType/collation 三值逻辑、用户变量只读折叠、VALUES 类型族
// 与二进制字面量、GET_PARAM 与 ROW panic 契约，对齐 Go `builtin_other_test.go`。

use std::str::FromStr;

use crate::expression_other::*;
use rust_decimal::Decimal;
use serde_json::json;

/// 将十进制字符串解析为 `Decimal`，供测试用例构造。
fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

#[test]
/// 验证 BIT_COUNT：整数/浮点/无符号/大 Decimal/非法串与 NULL，以及补码位计数。
fn bit_count_matches_go_conversion_and_twos_complement_cases() {
    // 覆盖正数、负数补码、浮点截断、无符号全 1、超大 Decimal/字符串与非法串。
    let cases = [
        (Value::Int(8), Some(1)),
        (Value::Int(29), Some(4)),
        (Value::Int(0), Some(0)),
        (Value::Int(-1), Some(64)),
        (Value::Int(-11), Some(62)),
        (Value::Int(-1000), Some(56)),
        (Value::Real(1.1), Some(1)),
        (Value::Real(3.1), Some(2)),
        (Value::Real(-1.1), Some(64)),
        (Value::Real(-3.1), Some(63)),
        (Value::UInt(u64::MAX), Some(64)),
        (
            Value::Decimal(dec("9999999999999999999999999999")),
            Some(64),
        ),
        (
            Value::String("9999999999999999999999999999".into()),
            Some(64),
        ),
        (Value::String("xxx".into()), Some(0)),
        (Value::Null, None),
    ];

    for (input, expected) in cases {
        assert_eq!(bit_count(&input).unwrap(), expected, "input: {input:?}");
    }
}

#[test]
/// 验证整数 IN：常量去重缓存、非常量参数计数、NULL 常量、有符号/无符号互比。
fn in_int_preserves_unsigned_rules_nulls_and_constant_cache_shape() {
    let field = FieldType::new(EvalType::Int);
    let predicate = InPredicate::build(
        field.clone(),
        vec![
            Expr::column(0),
            Expr::constant(Value::UInt(u64::MAX)),
            Expr::constant(Value::Int(2)),
            Expr::constant(Value::Int(2)),
            Expr::constant(Value::Null),
            Expr::context(Value::Int(5)),
        ],
        false,
    )
    .unwrap();

    assert_eq!(predicate.cached_constants(), 2);
    assert_eq!(predicate.non_constant_args(), 1);
    assert!(predicate.has_null_constant());
    assert_eq!(predicate.argument_count(), 5);
    assert_eq!(predicate.eval(&[Value::Int(2)]).unwrap(), Some(true));
    assert_eq!(predicate.eval(&[Value::Int(9)]).unwrap(), None);

    assert_eq!(
        eval_in(
            field.clone(),
            Value::Int(-1),
            vec![Value::UInt(u64::MAX), Value::Int(2)],
        )
        .unwrap(),
        Some(false)
    );
    assert_eq!(
        eval_in(
            field.clone(),
            Value::UInt(u64::MAX),
            vec![Value::Int(-1), Value::Int(2)],
        )
        .unwrap(),
        Some(false)
    );
    assert_eq!(
        eval_in(field, Value::UInt(u64::MAX), vec![Value::UInt(u64::MAX)],).unwrap(),
        Some(true)
    );
}

#[test]
/// BIT 列 IN 会剪掉负数常量，并记录跳过计划缓存的原因。
fn in_bit_prunes_negative_constants_and_marks_plan_cache_skip() {
    let built = InPredicate::build(
        FieldType::bit(),
        vec![
            Expr::column(0),
            Expr::constant(Value::Int(-1)),
            Expr::constant(Value::Int(1)),
        ],
        true,
    )
    .unwrap();

    assert_eq!(built.argument_count(), 2);
    assert_eq!(built.skip_plan_cache_reason(), Some("Bit Column in (-1)"));
    assert_eq!(built.eval(&[Value::UInt(1)]).unwrap(), Some(true));
}

#[test]
/// 各值类型 IN 命中，以及 ci collation 折叠与 binary 下 NULL 三值逻辑。
fn in_all_go_value_families_and_collation_follow_three_valued_logic() {
    let cases = [
        (
            FieldType::new(EvalType::Real),
            Value::Real(1.1),
            vec![Value::Real(1.2), Value::Real(1.1)],
        ),
        (
            FieldType::new(EvalType::Decimal),
            Value::Decimal(dec("123.121")),
            vec![
                Value::Decimal(dec("123.122")),
                Value::Decimal(dec("123.121")),
            ],
        ),
        (
            FieldType::new(EvalType::Time),
            Value::Time(1_483_232_461_000_001),
            vec![
                Value::Time(1_483_318_861_000_001),
                Value::Time(1_483_232_461_000_001),
            ],
        ),
        (
            FieldType::new(EvalType::Duration),
            Value::Duration(43_261_000_000_000),
            vec![
                Value::Duration(43_260_000_000_000),
                Value::Duration(43_261_000_000_000),
            ],
        ),
        (
            FieldType::new(EvalType::Json),
            Value::Json(json!(123.1)),
            vec![Value::Json(json!(123.2)), Value::Json(json!(123.1))],
        ),
        (
            FieldType::new(EvalType::VectorFloat32),
            Value::VectorFloat32(vec![1.0, 2.0]),
            vec![
                Value::VectorFloat32(vec![2.0]),
                Value::VectorFloat32(vec![1.0, 2.0]),
            ],
        ),
    ];

    for (field, needle, candidates) in cases {
        assert_eq!(eval_in(field, needle, candidates).unwrap(), Some(true));
    }

    let ci = FieldType::string("utf8_general_ci");
    assert_eq!(
        eval_in(
            ci,
            Value::String("a".into()),
            vec![Value::String("Á".into())]
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        eval_in(
            FieldType::string("binary"),
            Value::String("a".into()),
            vec![Value::Null, Value::String("b".into())],
        )
        .unwrap(),
        None
    );
}

#[test]
/// 用户变量写入/按类型读取、只读折叠为常量，以及 NULL 赋值删除变量。
fn set_get_user_vars_preserve_values_conversions_and_readonly_folding() {
    let mut session = Session::default();
    let values = [
        ("a", Value::String("中".into())),
        ("b", Value::Int(3)),
        ("c", Value::Real(2.5)),
        ("d", Value::Decimal(dec("5"))),
        ("e", Value::Time(1_700_000_000_000_000)),
    ];
    for (name, value) in values {
        assert_eq!(session.set_user_var(name, value.clone()).unwrap(), value);
    }
    assert_eq!(
        session.get_user_var("A", EvalType::String).unwrap(),
        Some(Value::String("中".into()))
    );
    assert_eq!(
        session.get_user_var("b", EvalType::Real).unwrap(),
        Some(Value::Real(3.0))
    );
    assert_eq!(
        session.get_user_var("b", EvalType::Decimal).unwrap(),
        Some(Value::Decimal(dec("3")))
    );
    assert_eq!(
        session.get_user_var("missing", EvalType::String).unwrap(),
        None
    );

    session.mark_readonly("a");
    assert!(matches!(
        GetVarExpr::build(&session, "a", EvalType::String).unwrap(),
        GetVarExpr::Constant(_)
    ));
    assert!(matches!(
        GetVarExpr::build(&session, "b", EvalType::Int).unwrap(),
        GetVarExpr::Runtime { .. }
    ));

    assert_eq!(
        session.set_user_var("null", Value::Null).unwrap(),
        Value::Null
    );
    assert!(!session.contains_user_var("null"));
}

#[test]
/// 用户变量应拥有字符串副本，输入行后续修改不影响已存储值。
fn set_string_from_row_owns_the_value_after_input_changes() {
    let mut session = Session::default();
    let mut row = vec![Value::String("a".into())];
    session.set_user_var("name", row[0].clone()).unwrap();
    row[0] = Value::String("b".into());

    assert_eq!(
        session.get_user_var("name", EvalType::String).unwrap(),
        Some(Value::String("a".into()))
    );
}

#[test]
/// VALUES：参数个数错误、空插入行、偏移越界、多类型取值与过长二进制字面量。
fn values_function_covers_types_null_empty_and_offset_errors() {
    let mut session = Session::default();
    assert!(matches!(
        ValuesFunction::build(1, FieldType::string("binary"), 1),
        Err(OtherError::ArgumentCount { function: "values" })
    ));
    let string_values = ValuesFunction::build(1, FieldType::string("binary"), 0).unwrap();
    assert_eq!(string_values.eval(&session).unwrap(), Value::Null);
    session.curr_insert_values = vec![Value::String("1".into())];
    assert!(matches!(
        string_values.eval(&session),
        Err(OtherError::ValuesOffset { .. })
    ));
    session.curr_insert_values = vec![Value::String("1".into()), Value::String("2".into())];
    assert_eq!(
        string_values.eval(&session).unwrap(),
        Value::String("2".into())
    );

    // 各 EvalType 的 VALUES 直接返回 curr_insert_values 中对应偏移的值。
    let typed = [
        (FieldType::new(EvalType::Int), Value::Int(7)),
        (FieldType::float32(), Value::Real(1.25)),
        (
            FieldType::new(EvalType::Decimal),
            Value::Decimal(dec("1.20")),
        ),
        (FieldType::new(EvalType::Time), Value::Time(42)),
        (FieldType::new(EvalType::Duration), Value::Duration(9)),
        (FieldType::new(EvalType::Json), Value::Json(json!({"a": 1}))),
        (
            FieldType::new(EvalType::VectorFloat32),
            Value::VectorFloat32(vec![1.0, 2.0]),
        ),
    ];
    for (field, value) in typed {
        session.curr_insert_values = vec![value.clone()];
        assert_eq!(ValuesFunction::new(0, field).eval(&session).unwrap(), value);
    }

    session.curr_insert_values = vec![Value::BinaryLiteral(vec![0x01, 0x02])];
    assert_eq!(
        ValuesFunction::new(0, FieldType::new(EvalType::Int))
            .eval(&session)
            .unwrap(),
        Value::Int(258)
    );
    session.curr_insert_values = vec![Value::BinaryLiteral(vec![0; 9])];
    assert!(matches!(
        ValuesFunction::new(0, FieldType::new(EvalType::Int)).eval(&session),
        Err(OtherError::InsertValueTooLong)
    ));
}

#[test]
/// GET_PARAM 将计划缓存参数转字符串，并报告越界/负索引。
fn get_param_returns_strings_and_reports_bad_indexes() {
    let mut session = Session::default();
    session.plan_cache_params = vec![Value::Int(123), Value::String("abc".into())];

    assert_eq!(get_param(&session, 0).unwrap(), Some("123".into()));
    assert_eq!(get_param(&session, 1).unwrap(), Some("abc".into()));
    assert!(matches!(
        get_param(&session, 3),
        Err(OtherError::ParamIndex { index: 3, count: 2 })
    ));
    assert!(matches!(
        get_param(&session, -1),
        Err(OtherError::ParamIndex { .. })
    ));
}

#[test]
/// ROW 可构建元数据，标量求值按 Go 契约 panic。
fn row_signature_builds_but_scalar_evaluation_panics_like_go() {
    let row = RowFunction::new(vec![
        FieldType::string("binary"),
        FieldType::new(EvalType::Real),
        FieldType::new(EvalType::Int),
    ]);
    assert_eq!(row.argument_types().len(), 3);
    assert!(std::panic::catch_unwind(|| row.eval()).is_err());
}
