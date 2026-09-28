// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// FieldType 相关迁移期单元测试。
//
// 覆盖 FSP（小数秒精度）校验、分数舍入、浮点截断、
// 类型默认值/聚合、按值推断类型，以及列修改兼容性与 varchar 长度上限。

use crate::field::*;
use crate::metadata::{charset, mysql};

/// 校验 CheckFsp：未指定/越界归一到默认或 MaxFsp，非法负值报错。
#[test]
fn fsp_validation_preserves_go_result_and_error_shape() {
    let (value, error) = CheckFsp(UnspecifiedFsp);
    assert_eq!(DefaultFsp, value);
    assert!(error.is_none());
    let (value, error) = CheckFsp(-2019);
    assert_eq!(DefaultFsp, value);
    assert_eq!("Invalid fsp -2019", error.unwrap().to_string());
    let (value, error) = CheckFsp(MaxFsp + 2019);
    assert_eq!(MaxFsp, value);
    assert!(error.is_none());
    let (value, error) = CheckFsp(5);
    assert_eq!(5, value);
    assert!(error.is_none());
}

/// 校验 ParseFrac 舍入、进位溢出与非数字错误。
#[test]
fn parse_frac_matches_rounding_overflow_and_error_cases() {
    for (input, fsp, expected, overflow) in [
        ("", 5, 0, false),
        ("123456", 4, 123_500, false),
        ("1234567", 6, 123_457, false),
        ("1236", 3, 124_000, false),
        ("0312", 2, 30_000, false),
        ("999", 2, 0, true),
    ] {
        let (actual, actual_overflow, error) = ParseFrac(input, fsp);
        assert_eq!(expected, actual, "input {input}");
        assert_eq!(overflow, actual_overflow, "input {input}");
        assert!(error.is_none(), "input {input}");
    }
    assert!(ParseFrac("NotNum", MaxFsp).2.is_some());
}

/// 校验 AlignFrac 按目标位数补零且保留负号。
#[test]
fn align_frac_counts_digits_without_the_sign() {
    assert_eq!("100000", AlignFracForTest("100", 6));
    assert_eq!("-100000", AlignFracForTest("-100", 6));
    assert_eq!("10000000000", AlignFracForTest("10000000000", 6));
}

/// 校验 RoundFloat/Truncate/TruncateFloatToString 边界与 Go 一致。
#[test]
fn rounding_and_truncation_match_go_boundaries() {
    assert_eq!(2.0, RoundFloat(2.5));
    assert_eq!(2.0, RoundFloat(1.5));
    assert_eq!(-2.0, RoundFloat(-1.5));
    assert_eq!(123.4, Truncate(123.45, 1));
    assert_eq!(0.0, Truncate(123.45, -400));
    assert_eq!(123.45, Truncate(123.45, 400));
    assert_eq!("0.53", TruncateFloatToString(0.539, 2));
    assert_eq!("-0.45", TruncateFloatToString(-0.456, 2));
}

/// 校验字符串转整数在 i64 边界饱和，并标记溢出/截断错误。
#[test]
fn str_to_int_saturates_at_signed_boundaries() {
    let cases = [
        ("9223372036854775807", i64::MAX, false),
        ("9223372036854775808", i64::MAX, true),
        ("-9223372036854775808", i64::MIN, false),
        ("-9223372036854775809", i64::MIN, true),
        ("12x", 12, true),
    ];
    for (input, expected, has_error) in cases {
        let (actual, error) = StrToIntForTest(input);
        assert_eq!(expected, actual, "input {input}");
        assert_eq!(has_error, error.is_some(), "input {input}");
    }
}

/// 校验 Decimal 显示长度与精度互转。
#[test]
fn decimal_display_length_conversions_match_go() {
    assert_eq!(8, DecimalLength2Precision(10, 2, false));
    assert_eq!(8, DecimalLength2Precision(10, 2, true));
    assert_eq!(10, Precision2LengthNoTruncation(8, 2, false));
    assert_eq!(9, Precision2LengthNoTruncation(7, 2, true));
}

/// 校验 NewFieldType 默认 charset/flen 与 DATETIME flen 补正。
#[test]
fn field_type_defaults_and_datetime_width_match_go() {
    let string_type = NewFieldType(mysql::TypeVarchar);
    assert_eq!(mysql::DefaultCharset, string_type.GetCharset());
    assert_eq!(mysql::DefaultCollationName, string_type.GetCollate());

    let integer_type = NewFieldType(mysql::TypeLong);
    assert_eq!(charset::CharsetBin, integer_type.GetCharset());
    assert_eq!(
        mysql::GetDefaultFieldLengthAndDecimal(mysql::TypeLong).0,
        integer_type.GetFlen()
    );

    let mut datetime = NewFieldType(mysql::TypeDatetime);
    datetime.SetDecimal(3);
    TryToFixFlenOfDatetime(&mut datetime);
    assert_eq!(
        mysql::MaxDatetimeWidthNoFsp as isize + 4,
        datetime.GetFlen()
    );
}

/// 校验 AggFieldType 混合符号提升与 UnsignedFlag 保留。
#[test]
fn field_type_aggregation_preserves_flags_and_range_promotion() {
    let signed = NewFieldType(mysql::TypeLong);
    let mut unsigned = NewFieldType(mysql::TypeLong);
    unsigned.SetFlag(mysql::UnsignedFlag);
    let aggregated = AggFieldType(&[&signed, &unsigned]);
    assert_eq!(mysql::TypeLonglong, aggregated.GetType());
    assert_eq!(0, aggregated.GetFlag() & mysql::UnsignedFlag);

    let mut both_unsigned = NewFieldType(mysql::TypeLonglong);
    both_unsigned.SetFlag(mysql::UnsignedFlag);
    let aggregated = AggFieldType(&[&both_unsigned, &both_unsigned]);
    assert_eq!(
        mysql::UnsignedFlag,
        aggregated.GetFlag() & mysql::UnsignedFlag
    );
}

/// 校验 AggregateEvalType 字符串优先与混合符号→Decimal 规则。
#[test]
fn aggregate_eval_type_matches_string_numeric_and_mixed_sign_rules() {
    let string_type = NewFieldType(mysql::TypeVarchar);
    let int_type = NewFieldType(mysql::TypeLong);
    let mut flag = 0;
    assert_eq!(
        ETString,
        AggregateEvalType(&[&string_type, &int_type], &mut flag)
    );
    assert_eq!(0, flag & mysql::BinaryFlag);

    let mut unsigned = NewFieldType(mysql::TypeLonglong);
    unsigned.SetFlag(mysql::UnsignedFlag);
    flag = 0;
    assert_eq!(
        ETDecimal,
        AggregateEvalType(&[&int_type, &unsigned], &mut flag)
    );
    assert_eq!(mysql::BinaryFlag, flag & mysql::BinaryFlag);
}

/// 校验 DefaultTypeForValue 对布尔/整数/字符串/十六进制字面量推断。
#[test]
fn default_type_for_value_covers_go_scalar_and_literal_cases() {
    let mut field_type = FieldType::default();
    DefaultTypeForValue(
        Some(&true),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(mysql::TypeLonglong, field_type.GetType());
    assert_ne!(0, field_type.GetFlag() & mysql::IsBooleanFlag);

    let value = -42_i64;
    field_type = FieldType::default();
    DefaultTypeForValue(
        Some(&value),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(3, field_type.GetFlen());

    let value = "abc".to_owned();
    field_type = FieldType::default();
    DefaultTypeForValue(
        Some(&value),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(mysql::TypeVarString, field_type.GetType());
    assert_eq!(3, field_type.GetFlen());

    let literal = HexLiteral(BinaryLiteral(vec![0xab, 0xcd]));
    field_type = FieldType::default();
    DefaultTypeForValue(
        Some(&literal),
        &mut field_type,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    assert_eq!(6, field_type.GetFlen());
    assert_ne!(0, field_type.GetFlag() & mysql::UnsignedFlag);
}

/// 校验列修改兼容性（变短需 reorg）与 varchar 长度上限。
#[test]
fn modify_compatibility_and_varchar_limits_match_go() {
    let mut origin = NewFieldType(mysql::TypeVarchar);
    origin.SetFlen(20);
    let mut wider = NewFieldType(mysql::TypeVarchar);
    wider.SetFlen(30);
    assert!(CheckModifyTypeCompatible(&origin, &wider).1.is_none());

    let mut narrower = NewFieldType(mysql::TypeVarchar);
    narrower.SetFlen(10);
    let (can_reorg, error) = CheckModifyTypeCompatible(&origin, &narrower);
    assert!(can_reorg);
    assert!(
        error
            .unwrap()
            .to_string()
            .contains("length 10 is less than origin 20")
    );

    assert!(IsVarcharTooBigFieldLength(UnspecifiedLength, "c", charset::CharsetUTF8MB4).is_ok());
    assert!(
        IsVarcharTooBigFieldLength(
            mysql::MaxFieldVarCharLength as isize,
            "c",
            charset::CharsetUTF8MB4
        )
        .is_err()
    );
}
