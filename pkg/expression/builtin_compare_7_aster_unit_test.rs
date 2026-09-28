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

// 比较函数与 Go 对齐的单元测试。
//
// 覆盖 COALESCE、GREATEST/LEAST（含排序规则与时态字符串）、INTERVAL、
// 比较类型解析、NULL/无符号比较、多类型比较器，以及常量/无符号精化规则。

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde_json::json;

use crate::builtin_compare::{
    Collation, CompareType, Datum, EvalType, ExpressionMeta, FieldType, IntValue, MysqlType, Op,
    RefinedConstant, TemporalMode, UnsignedRefinement, coalesce, compare_datums,
    fix_flen_and_decimal_for_greatest_and_least, get_accurate_cmp_type, greatest, interval_int,
    interval_real, least, refine_compared_constant, refine_unsigned_comparison,
    resolve_type_for_between,
};

/// 构造有符号整型 Datum。
fn int(value: i64) -> Datum {
    Datum::Int(value)
}

/// COALESCE：返回首个非 NULL；短路后侧错误；全 NULL 或遇错则传播。
#[test]
fn coalesce_matches_go_first_non_null_and_error_order() {
    assert_eq!(coalesce(&[Datum::Null, int(1)]).unwrap(), int(1));
    assert_eq!(coalesce(&[Datum::Null, Datum::Null]).unwrap(), Datum::Null);
    assert_eq!(
        coalesce(&[int(1), Datum::Error("must not be evaluated".into())]).unwrap(),
        int(1)
    );
    assert_eq!(
        coalesce(&[Datum::Null, Datum::Error("boom".into())])
            .unwrap_err()
            .to_string(),
        "boom"
    );
}

/// GREATEST/LEAST：NULL 传播、ci 排序规则、AsDate 模式下日期与字符串统一比较。
#[test]
fn greatest_and_least_preserve_null_and_temporal_string_rules() {
    assert_eq!(
        greatest(
            &[int(1), int(4), int(2)],
            TemporalMode::Direct,
            Collation::Binary
        )
        .unwrap(),
        int(4)
    );
    assert_eq!(
        least(
            &[Datum::String("b".into()), Datum::String("A".into())],
            TemporalMode::Direct,
            Collation::Utf8Mb4GeneralCi,
        )
        .unwrap(),
        Datum::String("A".into())
    );
    assert_eq!(
        greatest(
            &[int(1), Datum::Null],
            TemporalMode::Direct,
            Collation::Binary
        )
        .unwrap(),
        Datum::Null
    );
    assert_eq!(
        greatest(
            &[Datum::Error("boom".into()), Datum::Null],
            TemporalMode::Direct,
            Collation::Binary,
        )
        .unwrap_err()
        .to_string(),
        "boom"
    );
    assert_eq!(
        least(
            &[Datum::Null, Datum::Error("must not be evaluated".into())],
            TemporalMode::Direct,
            Collation::Binary,
        )
        .unwrap(),
        Datum::Null
    );

    let date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
    assert_eq!(
        greatest(
            &[Datum::Date(date), Datum::String("2023-12-31".into()),],
            TemporalMode::AsDate,
            Collation::Binary,
        )
        .unwrap(),
        Datum::String("2024-01-02".into())
    );
}

/// INTERVAL：有序边界二分/线性扫描；NULL 目标为 -1；无符号与 Real 路径。
#[test]
fn interval_matches_go_binary_linear_and_mixed_sign_paths() {
    assert_eq!(
        interval_int(
            Some(IntValue::Signed(2)),
            &[Some(IntValue::Signed(1)), Some(IntValue::Signed(3))]
        ),
        1
    );
    assert_eq!(interval_int(None, &[Some(IntValue::Signed(1))]), -1);
    assert_eq!(
        interval_int(
            Some(IntValue::Signed(1)),
            &[None, None, None, Some(IntValue::Signed(2))],
        ),
        3
    );
    assert_eq!(
        interval_int(
            Some(IntValue::Unsigned(1_u64 << 63)),
            &[
                Some(IntValue::Signed(i64::MAX)),
                Some(IntValue::Unsigned((1_u64 << 63) + 1))
            ],
        ),
        1
    );
    assert_eq!(
        interval_real(Some(23.0), &[Some(1.7), Some(15.3), Some(23.1)]),
        2
    );
    assert_eq!(interval_real(Some(1.0), &[None, None, Some(2.0)]), 2);
}

/// 精确比较类型：DECIMAL/时间/JSON 例外，BETWEEN 三参数类型决议。
#[test]
fn comparison_type_resolution_keeps_go_precision_and_temporal_exceptions() {
    let decimal_column = ExpressionMeta::column(FieldType::new(MysqlType::NewDecimal));
    let string_constant = ExpressionMeta::constant(FieldType::new(MysqlType::Varchar));
    assert_eq!(
        get_accurate_cmp_type(&decimal_column, &string_constant),
        EvalType::Decimal
    );

    let datetime_column = ExpressionMeta::column(FieldType::new(MysqlType::Datetime));
    assert_eq!(
        get_accurate_cmp_type(&datetime_column, &string_constant),
        EvalType::Datetime
    );

    let json_column = ExpressionMeta::column(FieldType::new(MysqlType::Json));
    let int_constant = ExpressionMeta::constant(FieldType::new(MysqlType::LongLong));
    assert_eq!(
        get_accurate_cmp_type(&json_column, &int_constant),
        EvalType::Json
    );

    let duration = ExpressionMeta::column(FieldType::new(MysqlType::Duration));
    assert_eq!(
        get_accurate_cmp_type(&duration, &duration),
        EvalType::Duration
    );

    let between = [
        ExpressionMeta::constant(FieldType::new(MysqlType::Varchar)),
        ExpressionMeta::constant(FieldType::new(MysqlType::Date)),
        ExpressionMeta::constant(FieldType::new(MysqlType::Varchar)),
    ];
    assert_eq!(resolve_type_for_between(&between), EvalType::Datetime);
}

/// 比较算子：NULL-safe 相等、普通 NULL→None、无符号与有符号混比、七种 Op。
#[test]
fn all_comparison_operators_match_null_and_unsigned_semantics() {
    assert_eq!(
        compare_datums(
            &Datum::Null,
            &Datum::Null,
            CompareType::Int,
            Op::NullEq,
            Collation::Binary
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        compare_datums(
            &Datum::Null,
            &int(1),
            CompareType::Int,
            Op::Eq,
            Collation::Binary
        )
        .unwrap(),
        None
    );
    assert_eq!(
        compare_datums(
            &Datum::Null,
            &int(1),
            CompareType::Int,
            Op::NullEq,
            Collation::Binary
        )
        .unwrap(),
        Some(false)
    );
    assert_eq!(
        compare_datums(
            &Datum::UInt(1_u64 << 63),
            &Datum::Int(i64::MAX),
            CompareType::Int,
            Op::Gt,
            Collation::Binary,
        )
        .unwrap(),
        Some(true)
    );
    for (op, expected) in [
        (Op::Lt, false),
        (Op::Le, true),
        (Op::Gt, false),
        (Op::Ge, true),
        (Op::Eq, true),
        (Op::Ne, false),
        (Op::NullEq, true),
    ] {
        assert_eq!(
            compare_datums(&int(7), &int(7), CompareType::Int, op, Collation::Binary).unwrap(),
            Some(expected)
        );
    }
}

/// DECIMAL/时间/时长/JSON/向量比较器均已接通。
#[test]
fn decimal_time_duration_json_and_vector_comparators_are_live() {
    let decimal = Decimal::new(123_123, 3);
    assert_eq!(
        compare_datums(
            &Datum::Decimal(decimal),
            &Datum::Decimal(decimal),
            CompareType::Decimal,
            Op::Eq,
            Collation::Binary,
        )
        .unwrap(),
        Some(true)
    );
    let time = NaiveDateTime::parse_from_str("2024-01-02 03:04:05", "%Y-%m-%d %H:%M:%S").unwrap();
    assert_eq!(
        compare_datums(
            &Datum::DateTime(time),
            &Datum::DateTime(time),
            CompareType::Time,
            Op::Le,
            Collation::Binary
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        compare_datums(
            &Datum::Duration(10),
            &Datum::Duration(20),
            CompareType::Duration,
            Op::Lt,
            Collation::Binary
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        compare_datums(
            &Datum::Json(json!(1)),
            &Datum::Json(json!("1")),
            CompareType::Json,
            Op::Lt,
            Collation::Binary
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        compare_datums(
            &Datum::VectorFloat32(vec![1.0, 2.0]),
            &Datum::VectorFloat32(vec![1.0, 3.0]),
            CompareType::VectorFloat32,
            Op::Lt,
            Collation::Binary,
        )
        .unwrap(),
        Some(true)
    );
}

/// 常量精化：小数对整列用 ceil/floor；相等例外保留 Real；YEAR 两位数扩展。
#[test]
fn compared_constant_refinement_uses_ceil_floor_and_equality_exception() {
    let int_type = FieldType::new(MysqlType::LongLong).not_null();
    assert_eq!(
        refine_compared_constant(
            &int_type,
            RefinedConstant::Decimal(Decimal::new(11, 1)),
            Op::Lt
        ),
        (RefinedConstant::Int(2), false)
    );
    assert_eq!(
        refine_compared_constant(
            &int_type,
            RefinedConstant::Decimal(Decimal::new(11, 1)),
            Op::Le
        ),
        (RefinedConstant::Int(1), false)
    );
    assert_eq!(
        refine_compared_constant(&int_type, RefinedConstant::Real(1.1), Op::Eq),
        (RefinedConstant::Real(1.1), true)
    );
    assert_eq!(
        refine_compared_constant(
            &FieldType::new(MysqlType::Year),
            RefinedConstant::Int(2),
            Op::Eq
        ),
        (RefinedConstant::Int(2002), false)
    );
}

/// 无符号列与负常量：NOT NULL 可折叠为恒真/恒假；可空列保留原比较。
#[test]
fn unsigned_refinement_respects_nullable_columns() {
    let unsigned_not_null = FieldType::new(MysqlType::LongLong).unsigned().not_null();
    assert_eq!(
        refine_unsigned_comparison(&unsigned_not_null, IntValue::Signed(-1), Op::Gt),
        UnsignedRefinement::Always(true)
    );
    assert_eq!(
        refine_unsigned_comparison(&unsigned_not_null, IntValue::Signed(-1), Op::Eq),
        UnsignedRefinement::Always(false)
    );
    assert_eq!(
        refine_unsigned_comparison(
            &FieldType::new(MysqlType::LongLong).unsigned(),
            IntValue::Signed(-1),
            Op::Eq
        ),
        UnsignedRefinement::Keep
    );
}

/// GREATEST/LEAST 结果 flen/decimal 取参数中的最大值。
#[test]
fn greatest_and_least_metadata_uses_max_flen_and_decimal() {
    let fields = [
        FieldType::new(MysqlType::NewDecimal).with_flen_decimal(5, 2),
        FieldType::new(MysqlType::NewDecimal).with_flen_decimal(12, 4),
        FieldType::new(MysqlType::NewDecimal).with_flen_decimal(7, 3),
    ];
    assert_eq!(
        fix_flen_and_decimal_for_greatest_and_least(&fields),
        (12, 4)
    );
}
