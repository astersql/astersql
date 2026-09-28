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

// CAST 标量语义与 Go 对齐的单元测试。
//
// 覆盖字符串长度（字符/二进制）、数值矩阵舍入与无符号、DECIMAL 精度钳制、
// 时间/时长、JSON/数组、VectorFloat32 支持路径，以及字段元数据辅助函数边界。

use super::*;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;

/// 构造 CAST 表达式并求值，返回结果与上下文（含 warnings）。
fn eval(value: Value, source: FieldType, target: FieldType) -> (Value, CastContext) {
    let mut ctx = CastContext::default();
    let expression = BuildCastFunction(Expression::constant(value, source), target);
    let result = expression.eval(&mut ctx).unwrap();
    (result, ctx)
}

/// 字符串 CAST 按字符数截断，二进制目标则按字节填充零。
#[test]
fn cast_string_length_obeys_character_and_binary_rules() {
    let source = FieldType::new(FieldKind::String);
    // 字符集路径：flen 按 Unicode 字符计数（“你好world”→“你好wor”）。
    let character = FieldType::new(FieldKind::String).with_flen(5);
    let (value, _) = eval(Value::String("你好world".into()), source.clone(), character);
    assert_eq!(value, Value::String("你好wor".into()));

    // 二进制路径：flen 按字节，不足右侧补 0x00。
    let binary = FieldType::new(FieldKind::String)
        .with_flen(5)
        .with_binary(true);
    let (value, _) = eval(Value::String("a".into()), source, binary);
    assert_eq!(value, Value::Bytes(vec![b'a', 0, 0, 0, 0]));
}

/// 数值 CAST 矩阵：四舍五入、无符号负数警告、科学计数法截断。
#[test]
fn cast_numeric_matrix_matches_go_rounding_and_unsigned_rules() {
    let (value, _) = eval(
        Value::Real(1.6),
        FieldType::new(FieldKind::Real),
        FieldType::new(FieldKind::Int),
    );
    assert_eq!(value, Value::Int(2));

    let (value, ctx) = eval(
        Value::String("-1".into()),
        FieldType::new(FieldKind::String),
        FieldType::new(FieldKind::Int).with_unsigned(true),
    );
    // 负数字符串转为无符号整型：结果为全 1 位型，并记录 CastNegativeAsUnsigned。
    assert_eq!(value, Value::UInt(u64::MAX));
    assert_eq!(ctx.warnings, vec![CastWarning::CastNegativeAsUnsigned]);

    let (value, ctx) = eval(
        Value::String("125e342.83".into()),
        FieldType::new(FieldKind::String),
        FieldType::new(FieldKind::Int),
    );
    assert_eq!(value, Value::Int(125));
    assert_eq!(ctx.warnings, vec![CastWarning::Truncated]);
}

/// DECIMAL CAST：按声明精度舍入，溢出时钳制到最大可表示值。
#[test]
fn decimal_cast_rounds_and_clamps_to_declared_precision() {
    let target = FieldType::new(FieldKind::Decimal)
        .with_flen(7)
        .with_decimal(3);
    let (value, _) = eval(
        Value::Real(1234.1234),
        FieldType::new(FieldKind::Real),
        target,
    );
    assert_eq!(
        value,
        Value::Decimal(Decimal::from_str("1234.123").unwrap())
    );

    let target = FieldType::new(FieldKind::Decimal)
        .with_flen(5)
        .with_decimal(2);
    let (value, ctx) = eval(Value::Int(99999), FieldType::new(FieldKind::Int), target);
    assert_eq!(value, Value::Decimal(Decimal::from_str("999.99").unwrap()));
    assert_eq!(ctx.warnings, vec![CastWarning::Overflow]);
}

/// 时间与时长 CAST：整型日期时间与带小数秒的时长字符串路径。
#[test]
fn time_and_duration_casts_cover_numeric_string_and_cross_type_paths() {
    let current = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
    let mut ctx = CastContext::new(current);
    let time_target = FieldType::new(FieldKind::DateTime).with_decimal(0);
    let expression = BuildCastFunction(
        Expression::constant(Value::Int(20260715125959), FieldType::new(FieldKind::Int)),
        time_target,
    );
    assert_eq!(
        expression.eval(&mut ctx).unwrap(),
        Value::Time(MysqlTime::DateTime(
            NaiveDateTime::new(current, NaiveTime::from_hms_opt(12, 59, 59).unwrap()),
            0,
        ))
    );

    let duration_target = FieldType::new(FieldKind::Duration).with_decimal(3);
    let expression = BuildCastFunction(
        Expression::constant(
            Value::String("12:59:59.5556".into()),
            FieldType::new(FieldKind::String),
        ),
        duration_target,
    );
    assert_eq!(
        expression.eval(&mut ctx).unwrap(),
        Value::Duration(MysqlDuration::new(false, 12, 59, 59, 556_000, 3).unwrap())
    );
}

/// JSON CAST：parse_to_json 解析对象；JSON 数组元素按目标元素类型转换。
#[test]
fn json_conversion_preserves_parse_flag_and_typed_array_semantics() {
    let parse_json = FieldType::new(FieldKind::Json).with_parse_to_json(true);
    let (value, _) = eval(
        Value::String("{\"a\":1}".into()),
        FieldType::new(FieldKind::String),
        parse_json,
    );
    assert_eq!(value, Value::Json(json!({"a": 1})));

    let array_target = FieldType::array(FieldType::new(FieldKind::Int));
    let (value, _) = eval(
        Value::Json(json!([1, "2", null])),
        FieldType::new(FieldKind::Json),
        array_target,
    );
    assert_eq!(
        value,
        Value::Array(vec![Value::Int(1), Value::Int(2), Value::Null])
    );
}

/// VectorFloat32 仅接受 Go 已支持的来源路径，其它组合返回 Unsupported。
#[test]
fn vector_float32_only_accepts_go_supported_paths() {
    let (value, _) = eval(
        Value::String("[1, 2.5, -3]".into()),
        FieldType::new(FieldKind::String),
        FieldType::new(FieldKind::VectorFloat32),
    );
    assert_eq!(value, Value::VectorFloat32(vec![1.0, 2.5, -3.0]));

    let expression = BuildCastFunction(
        Expression::constant(
            Value::VectorFloat32(vec![1.0]),
            FieldType::new(FieldKind::VectorFloat32),
        ),
        FieldType::new(FieldKind::Int),
    );
    assert!(matches!(
        expression.eval(&mut CastContext::default()),
        Err(CastError::Unsupported { .. })
    ));
}

/// 字段元数据辅助：整型最小 DECIMAL 长度、DOUBLE flen、DECIMAL 显示宽度。
#[test]
fn field_metadata_helpers_match_go_boundaries() {
    assert_eq!(minimalDecimalLenForHoldingInteger(MysqlType::Tiny), 3);
    assert_eq!(minimalDecimalLenForHoldingInteger(MysqlType::LongLong), 20);
    assert_eq!(setDataTypeDouble(-1), (23, -1));
    assert_eq!(setDataTypeDouble(5), (16, -1));

    let signed = FieldType::new(FieldKind::Decimal)
        .with_flen(10)
        .with_decimal(2);
    assert_eq!(decimalPrecisionToLength(&signed), 12);
    let unsigned = signed.with_unsigned(true);
    assert_eq!(decimalPrecisionToLength(&unsigned), 11);
}

/// NULL CAST 保留可空性；同源同型 WrapWithCastAsInt 为恒等。
#[test]
fn null_and_identity_casts_are_preserved() {
    let source = FieldType::new(FieldKind::Int).with_not_null(false);
    let target = FieldType::new(FieldKind::Real).with_not_null(true);
    let expression = BuildCastFunction(Expression::constant(Value::Null, source), target);
    assert!(!expression.target.not_null);
    assert_eq!(
        expression.eval(&mut CastContext::default()).unwrap(),
        Value::Null
    );

    let expr = Expression::constant(Value::Int(7), FieldType::new(FieldKind::Int));
    assert_eq!(WrapWithCastAsInt(expr.clone(), None), expr);
}
