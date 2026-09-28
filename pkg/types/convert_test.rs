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

// 类型转换单测：数值/字符串/时间/JSON 互转与截断策略。
//
// 覆盖 Convert*、StrTo*、科学计数法、合法前缀提取，
// 以及 JSON→整数/浮点/DECIMAL 与 Duration 编码路径。

use std::any::Any;

use crate::datum as types;
use crate::json_binary as json_core;
use crate::metadata::{TypeToStr, mysql};
use crate::scalar as conv;

/// 默认语句 Flags。
fn default_stmt_flags() -> types::Flags {
    crate::DefaultStmtFlags
}

/// 从文本构造 MyDecimal。
fn decimal_from_text(value: &str) -> types::MyDecimal {
    let mut decimal = types::MyDecimal::default();
    decimal.FromString(value.as_bytes()).unwrap();
    decimal
}

/// 字符串经 ConvertTo 转为 YEAR。
fn year_from_str(value: &str) -> Result<i64, types::errors::Error> {
    let target = types::NewFieldType(mysql::TypeYear);
    types::NewStringDatum(value.to_owned())
        .ConvertTo(ignored_truncation_context(), &target)
        .map(|datum| datum.GetInt64())
}

/// 调用 AdjustYear，对齐 Go 年份规范化。
fn adjust_year_go(value: i64, adjust_zero: bool) -> Result<i64, crate::time::TimeError> {
    types::AdjustYear(value, adjust_zero)
}

/// 浮点字符串转整数字符串（可带回截断错误）。
fn float_str_to_int_str_safe(
    value: &str,
    original: &str,
) -> (String, Option<conv::errors::SharedError>) {
    conv::floatStrToIntStr(value, original)
}

/// JSON→i64；失败时按类型给出与 Go 一致的回退值。
fn json_to_int64(json: types::BinaryJSON) -> types::ValueResult<i64> {
    match types::ConvertJSONToInt64(strict_context(), json.clone(), false) {
        Ok(value) => Ok(value),
        Err(error) => {
            let fallback = match json.TypeCode {
                json_core::JSONTypeCodeInt64 => json.GetInt64(),
                json_core::JSONTypeCodeUint64 => json.GetUint64() as i64,
                json_core::JSONTypeCodeFloat64 => json.GetFloat64().round() as i64,
                json_core::JSONTypeCodeLiteral
                    if json.Value.first() == Some(&json_core::JSONLiteralTrue) =>
                {
                    1
                }
                json_core::JSONTypeCodeString => conv::StrToInt(
                    ignored_truncation_context(),
                    &String::from_utf8_lossy(&json.GetString()),
                    false,
                )
                .unwrap_or_else(|error| error.value),
                _ => 0,
            };
            Err(types::ErrorWithValue::new(
                fallback,
                conv::errors::New(error.to_string()),
            ))
        }
    }
}

/// JSON→f64；字符串失败时用忽略截断 Context 重试。
fn json_to_float(json: types::BinaryJSON) -> types::ValueResult<f64> {
    match types::ConvertJSONToFloat(strict_context(), json.clone()) {
        Ok(value) => Ok(value),
        Err(error) => {
            let fallback = if json.TypeCode == json_core::JSONTypeCodeString {
                conv::StrToFloat(
                    ignored_truncation_context(),
                    &String::from_utf8_lossy(&json.GetString()),
                    false,
                )
                .unwrap_or_else(|error| error.value)
            } else {
                0.0
            };
            Err(types::ErrorWithValue::new(
                fallback,
                conv::errors::New(error.to_string()),
            ))
        }
    }
}

/// JSON→DECIMAL。
fn json_to_decimal(json: types::BinaryJSON) -> types::ValueResult<types::MyDecimal> {
    match types::ConvertJSONToDecimal(strict_context(), json) {
        Ok(value) => Ok(value),
        Err(error) => Err(types::ErrorWithValue::new(
            types::MyDecimal::default(),
            conv::errors::New(error.to_string()),
        )),
    }
}

/// 整数 Datum 转为 Duration。
fn number_to_duration(number: i64, fsp: i32) -> Result<types::Duration, types::errors::Error> {
    let mut target = types::NewFieldType(mysql::TypeDuration);
    target.SetDecimal(fsp as isize);
    types::NewIntDatum(number)
        .ConvertTo(strict_context(), &target)
        .map(|datum| datum.GetMysqlDuration())
}

/// 字符串解析 Duration；足够长时优先 DATETIME。返回是否为 Duration。
fn str_to_duration(
    context: types::Context,
    value: &str,
    fsp: i32,
) -> Result<bool, types::errors::Error> {
    let trimmed = value.trim().trim_start_matches('-');
    let integer_len = trimmed.split('.').next().unwrap_or_default().len();
    if integer_len >= 12
        && types::ParseTime(
            &types::time_context(&context),
            value,
            mysql::TypeDatetime,
            fsp,
        )
        .is_ok()
    {
        return Ok(false);
    }
    types::ParseDuration(&types::time_context(&context), value, fsp)
        .map(|_| true)
        .map_err(Into::into)
}

/// 严格/默认无 warning Context。
fn strict_context() -> types::Context {
    types::DefaultStmtNoWarningContext.clone()
}

/// 忽略截断错误的 Context。
fn ignored_truncation_context() -> types::Context {
    strict_context().WithFlags(default_stmt_flags().WithIgnoreTruncateErr(true))
}

/// 从 ValueResult 取出成功值或错误中的裁剪值。
fn result_value<T: Copy>(result: &types::ValueResult<T>) -> T {
    match result {
        Ok(value) => *value,
        Err(error) => error.value,
    }
}

/// 浮点/整数/无符号互转与溢出裁剪。
/// Datum ConvertTo 跨类型综合用例。
#[test]
fn test_convert_type() {
    let float_cases = [
        (111.114, 111.11, false),
        (999.999, 999.99, true),
        (-999.999, -999.99, true),
        (1111.11, 999.99, true),
        (999.916, 999.92, false),
        (999.914, 999.91, false),
        (999.9155, 999.92, false),
    ];
    for (input, expected, has_error) in float_cases {
        let (actual, error) = types::TruncateFloat(input, 5, 2);
        assert_eq!(actual, expected, "input={input}");
        assert_eq!(error.is_some(), has_error, "input={input}");
    }

    let mut varchar = types::NewFieldType(mysql::TypeVarchar);
    varchar.SetFlen(3);
    varchar.SetCharset("utf8".to_owned());
    let (actual, error) =
        types::ProduceStrWithSpecifiedTp("12345".to_owned(), &varchar, strict_context(), true);
    assert_eq!(actual, "123");
    assert!(error.is_some());

    let context = types::time_context(&strict_context());
    let duration = types::ParseDuration(&context, "10:11:12.123456", 3)
        .unwrap()
        .0;
    assert_eq!(duration.String(), "10:11:12.123");
    let timestamp = types::ParseTime(
        &context,
        "2010-10-10 10:11:11.12345",
        mysql::TypeTimestamp,
        2,
    )
    .unwrap();
    assert_eq!(timestamp.String(), "2010-10-10 10:11:11.12");

    assert_eq!(conv::StrToInt(strict_context(), "100", false).unwrap(), 100);
    assert_eq!(
        types::NewBinaryLiteralFromUint(3_223_600, 3).0,
        vec![0x31, 0x30, 0x30]
    );

    let decimal = decimal_from_text("3.1416");
    assert_eq!(decimal.String(), "3.1416");
    let elems = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    assert_eq!(types::ParseEnum(&elems, "a", "utf8_bin").unwrap().Value, 1);
    assert_eq!(types::ParseEnumValue(&elems, 2).unwrap().Name, "b");
    assert_eq!(types::ParseSet(&elems, "a,b", "utf8_bin").unwrap().Value, 3);
    assert!(types::ParseSetValue(&elems, 9).is_err());

    let mut bit_field = types::NewFieldType(mysql::TypeBit);
    bit_field.SetFlen(24);
    assert_eq!(
        types::NewStringDatum("100".to_owned())
            .ConvertTo(strict_context(), &bit_field)
            .unwrap()
            .GetBinaryLiteral(),
        types::NewBinaryLiteralFromUint(3_223_600, 3)
    );
    bit_field.SetFlen(1);
    assert!(
        types::NewIntDatum(2)
            .ConvertTo(strict_context(), &bit_field)
            .is_err()
    );

    let mut decimal_field = types::NewFieldType(mysql::TypeNewDecimal);
    decimal_field.SetFlen(8);
    decimal_field.SetDecimal(4);
    for (input, expected) in [
        ("3.1416", "3.1416"),
        ("3.1415926", "3.1416"),
        ("199.00 ", "199.0000"),
    ] {
        assert_eq!(
            types::NewStringDatum(input.to_owned())
                .ConvertTo(strict_context(), &decimal_field)
                .unwrap()
                .GetMysqlDecimal()
                .String(),
            expected
        );
    }

    let year_field = types::NewFieldType(mysql::TypeYear);
    assert_eq!(
        types::NewStringDatum("2015".to_owned())
            .ConvertTo(strict_context(), &year_field)
            .unwrap()
            .GetInt64(),
        2015
    );
    assert!(
        types::NewIntDatum(1800)
            .ConvertTo(strict_context(), &year_field)
            .is_err()
    );
}

/// 测试辅助：Any 值转字符串。
fn to_string<T: Any>(value: T) -> String {
    conv::ToString(&value).unwrap()
}

/// 校验各类标量 ToString。
#[test]
fn test_convert_to_string() {
    assert_eq!(to_string("0".to_owned()), "0");
    assert_eq!(to_string(true), "1");
    assert_eq!(to_string("false".to_owned()), "false");
    assert_eq!(to_string(0_i32), "0");
    assert_eq!(to_string(0_i64), "0");
    assert_eq!(to_string(0_u64), "0");
    assert_eq!(to_string(1.6_f32), "1.6");
    assert_eq!(to_string(-0.6_f64), "-0.6");
    assert_eq!(to_string(vec![1_u8]), "\x01");
    assert_eq!(
        to_string(types::NewBinaryLiteralFromUint(0x4D7953514C, -1)),
        "MySQL"
    );
    assert_eq!(to_string(types::NewBinaryLiteralFromUint(0x41, -1)), "A");

    let elems = vec!["a".to_owned()];
    assert_eq!(types::ParseEnumValue(&elems, 1).unwrap().String(), "a");
    assert_eq!(types::ParseSetValue(&elems, 1).unwrap().String(), "a");
    assert!(conv::ToString(&vec![1_i16, 2_i16] as &dyn Any).is_err());

    let cases = [
        (5, "utf8", "你好，世界", "你好，世界"),
        (5, "utf8mb4", "你好，世界", "你好，世界"),
        (4, "utf8", "你好，世界", "你好，世"),
        (4, "utf8mb4", "你好，世界", "你好，世"),
        (15, "binary", "你好，世界", "你好，世界"),
        (12, "binary", "你好，世界", "你好，世"),
        (0, "binary", "你好，世界", ""),
    ];
    for (flen, charset, input, expected) in cases {
        let mut field = types::NewFieldType(mysql::TypeVarchar);
        field.SetFlen(flen);
        field.SetCharset(charset.to_owned());
        let (actual, error) =
            types::ProduceStrWithSpecifiedTp(input.to_owned(), &field, strict_context(), true);
        assert_eq!(actual, expected, "charset={charset}, flen={flen}");
        assert_eq!(error.is_some(), input != expected);
    }
}

/// 带字符集检查的字符串转换。
#[test]
fn test_convert_to_string_with_check() {
    let flags = default_stmt_flags();
    assert!(!flags.WithSkipUTF8Check(false).SkipUTF8Check());
    assert!(flags.WithSkipUTF8Check(true).SkipUTF8Check());
    assert!(!flags.WithSkipSACIICheck(false).SkipASCIICheck());
    assert!(flags.WithSkipSACIICheck(true).SkipASCIICheck());
    assert!(!flags.WithSkipUTF8MB4Check(false).SkipUTF8MB4Check());
    assert!(flags.WithSkipUTF8MB4Check(true).SkipUTF8MB4Check());

    for (charset, input) in [("utf8mb4", "你好"), ("utf8mb4", "你好👋"), ("utf8", "你好")] {
        let mut field = types::NewFieldType(mysql::TypeVarchar);
        field.SetFlen(255);
        field.SetCharset(charset.to_owned());
        let datum = types::NewStringDatum(input.to_owned());
        assert_eq!(
            datum
                .ConvertTo(strict_context(), &field)
                .unwrap()
                .GetString(),
            input
        );
    }

    for (bytes, charset) in [
        (vec![0xe4, 0xbd, 0xa0, 0xe5, 0xa5, 0xbd, 0x81], "utf8mb4"),
        (vec![0xe4, 0xbd, 0xa0, 0xe5, 0xa5, 0xbd, 0x81], "ascii"),
        (vec![0xf0, 0x9f, 0x91, 0x8b], "utf8"),
    ] {
        let mut field = types::NewFieldType(mysql::TypeVarchar);
        field.SetFlen(255);
        field.SetCharset(charset.to_owned());
        assert!(
            types::NewBytesDatum(bytes)
                .ConvertTo(strict_context(), &field)
                .is_err()
        );
    }
}

/// 二进制字符串转换。
#[test]
fn test_convert_to_binary_string() {
    let cases = [
        ("你好", "utf8_bin", "utf8", "你好"),
        ("你好", "utf8mb4_bin", "utf8mb4", "你好"),
        ("你好", "gbk_bin", "utf8", "你好"),
        ("你好", "gbk_bin", "gbk", "你好"),
        ("你好", "binary", "utf8mb4", "你好"),
    ];
    for (input, collation, charset, expected) in cases {
        let mut field = types::NewFieldType(mysql::TypeVarchar);
        field.SetFlen(255);
        field.SetCharset(charset.to_owned());
        let datum = types::NewCollationStringDatum(input.to_owned(), collation.to_owned());
        let output = datum.ConvertTo(strict_context(), &field).unwrap();
        assert_eq!(output.GetString(), expected, "{collation}->{charset}");
    }

    let mut gbk_field = types::NewFieldType(mysql::TypeVarchar);
    gbk_field.SetFlen(255);
    gbk_field.SetCharset("gbk".to_owned());
    let gbk = types::NewBytesDatum(vec![0xC4, 0xE3, 0xBA, 0xC3]);
    assert_eq!(
        gbk.ConvertTo(strict_context(), &gbk_field)
            .unwrap()
            .GetString(),
        "你好"
    );
    let invalid_gbk = types::NewBytesDatum(vec![0xC4, 0xE3, 0xBA, 0xC3, 0x81]);
    assert!(invalid_gbk.ConvertTo(strict_context(), &gbk_field).is_err());
}

/// 字符串转整数/浮点及截断策略。
#[test]
fn test_str_to_num() {
    let int_cases = [
        ("0", 0, true, false),
        ("-1", -1, true, false),
        ("100", 100, true, false),
        ("65.0", 65, false, false),
        ("65.0", 65, true, false),
        ("", 0, false, false),
        ("", 0, true, true),
        ("xx", 0, true, true),
        ("xx", 0, false, false),
        ("11xx", 11, true, true),
        ("11xx", 11, false, false),
        ("xx11", 0, false, false),
    ];
    for (input, expected, truncate_as_error, expect_error) in int_cases {
        let context = strict_context()
            .WithFlags(default_stmt_flags().WithIgnoreTruncateErr(!truncate_as_error));
        let result = conv::StrToInt(context, input, false);
        if !expect_error {
            assert_eq!(result_value(&result), expected, "input={input}");
        }
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }

    let uint_cases = [
        ("0", 0, true, false),
        ("", 0, false, false),
        ("-1", u64::MAX, false, true),
        ("100", 100, true, false),
        ("+100", 100, true, false),
        ("65.0", 65, true, false),
        ("xx", 0, true, true),
        ("11xx", 11, true, true),
        ("xx11", 0, true, true),
        ("-00", 0, true, false),
    ];
    for (input, expected, truncate_as_error, expect_error) in uint_cases {
        let context = strict_context()
            .WithFlags(default_stmt_flags().WithIgnoreTruncateErr(!truncate_as_error));
        let result = conv::StrToUint(context, input, false);
        if !expect_error {
            assert_eq!(result_value(&result), expected, "input={input}");
        }
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }

    let float_cases = [
        ("", 0.0, true, true),
        ("-1", -1.0, true, false),
        ("1.11", 1.11, true, false),
        ("1.11.00", 1.11, false, false),
        ("1.11.00", 1.11, true, true),
        ("xx", 0.0, false, false),
        ("0x00", 0.0, false, false),
        ("11.xx", 11.0, false, false),
        ("11.xx", 11.0, true, true),
        ("xx.11", 0.0, false, false),
        ("1e649", f64::MAX, true, true),
        ("1e649", f64::MAX, false, false),
        ("-1e649", -f64::MAX, true, true),
        ("-1e649", -f64::MAX, false, false),
    ];
    for (input, expected, truncate_as_error, expect_error) in float_cases {
        let context = strict_context()
            .WithFlags(default_stmt_flags().WithIgnoreTruncateErr(!truncate_as_error));
        let result = conv::StrToFloat(context, input, false);
        if !expect_error {
            assert_eq!(result_value(&result), expected, "input={input}");
        }
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }

    let warning_context =
        strict_context().WithFlags(default_stmt_flags().WithTruncateAsWarning(true));
    assert_eq!(
        conv::StrToInt(warning_context.clone(), "", false).unwrap(),
        0
    );
    assert_eq!(
        conv::StrToUint(warning_context.clone(), "", false).unwrap(),
        0
    );
    assert_eq!(conv::StrToFloat(warning_context, "", false).unwrap(), 0.0);
}

/// FieldType 到类型名字符串。
#[test]
fn test_field_type_to_str() {
    assert_eq!(
        TypeToStr(mysql::TypeUnspecified, "not binary"),
        types::TypeStr(mysql::TypeUnspecified)
    );
    assert_eq!(TypeToStr(mysql::TypeBlob, "binary"), "blob");
    assert_eq!(TypeToStr(mysql::TypeString, "binary"), "binary");
}

#[test]
fn test_convert() {
    let integer_types = [
        (
            mysql::TypeTiny,
            i8::MIN as i64,
            i8::MAX as i64,
            u8::MAX as u64,
        ),
        (
            mysql::TypeShort,
            i16::MIN as i64,
            i16::MAX as i64,
            u16::MAX as u64,
        ),
        (mysql::TypeInt24, -8_388_608, 8_388_607, 16_777_215),
        (
            mysql::TypeLong,
            i32::MIN as i64,
            i32::MAX as i64,
            u32::MAX as u64,
        ),
        (mysql::TypeLonglong, i64::MIN, i64::MAX, u64::MAX),
    ];
    for (tp, lower, upper, unsigned_upper) in integer_types {
        assert_eq!(
            conv::ConvertIntToInt(lower, lower, upper, tp).unwrap(),
            lower
        );
        assert_eq!(
            conv::ConvertIntToInt(upper, lower, upper, tp).unwrap(),
            upper
        );
        if lower != i64::MIN {
            let error = conv::ConvertIntToInt(lower - 1, lower, upper, tp).unwrap_err();
            assert_eq!(error.value, lower);
        }
        if upper != i64::MAX {
            let error = conv::ConvertIntToInt(upper + 1, lower, upper, tp).unwrap_err();
            assert_eq!(error.value, upper);
        }
        assert_eq!(
            conv::ConvertUintToUint(unsigned_upper, unsigned_upper, tp).unwrap(),
            unsigned_upper
        );
        if unsigned_upper != u64::MAX {
            assert_eq!(
                conv::ConvertUintToUint(unsigned_upper + 1, unsigned_upper, tp)
                    .unwrap_err()
                    .value,
                unsigned_upper
            );
        }
    }

    for (input, expected) in [
        ("  234  ", 234),
        (" 2.35e3 ", 2350),
        (" 2.e3 ", 2000),
        (" -2.e3 ", -2000),
        (" 2e2 ", 200),
        (" 0.002e3 ", 2),
        (" .002e3 ", 2),
        (" 20e-2 ", 0),
        (" -20e-2 ", 0),
        (" +2.51 ", 3),
        (" -9999.5 ", -10000),
        (" 999.4", 999),
        (" -3.58", -4),
    ] {
        assert_eq!(
            conv::StrToInt(ignored_truncation_context(), input, false).unwrap(),
            expected
        );
    }
    assert_eq!(
        conv::ConvertFloatToInt(234.5456, i32::MIN as i64, i32::MAX as i64, mysql::TypeLong)
            .unwrap(),
        235
    );
    assert_eq!(
        conv::ConvertFloatToInt(-23.45, i32::MIN as i64, i32::MAX as i64, mysql::TypeLong).unwrap(),
        -23
    );

    for (value, expected) in [("23.523", 23.523), ("-23.54", -23.54), ("1e+1", 10.0)] {
        assert_eq!(
            conv::StrToFloat(ignored_truncation_context(), value, false).unwrap(),
            expected
        );
    }

    for (input, expected) in [
        ("2000", 2000),
        ("abc", 0),
        ("00abc", 2000),
        ("0019", 2019),
        ("0", 2000),
        ("00", 2000),
        (" 0", 2000),
        (" 00", 2000),
        (" 000", 0),
        (" 0000 ", 2000),
        (" 0ab", 0),
        ("00bc", 0),
        ("000a", 0),
        (" 000a ", 2000),
        ("1", 2001),
        ("01", 2001),
        ("69", 2069),
        ("70", 1970),
        ("99", 1999),
    ] {
        assert_eq!(year_from_str(input).unwrap(), expected, "year={input}");
    }
    for (input, expected) in [
        (0, 0),
        (1, 2001),
        (69, 2069),
        (70, 1970),
        (99, 1999),
        (1901, 1901),
        (2155, 2155),
    ] {
        assert_eq!(
            adjust_year_go(input, false).unwrap(),
            expected,
            "year={input}"
        );
    }
    for invalid in [100, 123, 1800, 1900, 2156, 3000] {
        assert!(adjust_year_go(invalid, false).is_err());
    }

    for (target, input, expected) in [
        (mysql::TypeDate, "2012-08-23", "2012-08-23"),
        (
            mysql::TypeDatetime,
            "2012-08-23 12:34:03.123456",
            "2012-08-23 12:34:03",
        ),
        (
            mysql::TypeTimestamp,
            "2012-08-23 12:34:03.123456",
            "2012-08-23 12:34:03",
        ),
        (mysql::TypeDuration, "10:11:12", "10:11:12"),
    ] {
        assert_eq!(
            types::NewStringDatum(input.to_owned())
                .ConvertTo(strict_context(), &types::NewFieldType(target))
                .unwrap()
                .ToString()
                .unwrap(),
            expected,
            "target={target}"
        );
    }
    for target in [mysql::TypeDate, mysql::TypeDatetime, mysql::TypeTimestamp] {
        assert!(
            types::NewStringDatum("2012-08-x".to_owned())
                .ConvertTo(strict_context(), &types::NewFieldType(target))
                .is_err()
        );
    }

    let string_field = types::NewFieldType(mysql::TypeString);
    for (datum, expected) in [
        (types::NewStringDatum("abc".to_owned()), "abc"),
        (types::NewIntDatum(5678), "5678"),
        (types::NewBytesDatum(b"123".to_vec()), "123"),
    ] {
        assert_eq!(
            datum
                .ConvertTo(strict_context(), &string_field)
                .unwrap()
                .ToString()
                .unwrap(),
            expected
        );
    }

    let decimal_field = types::NewFieldType(mysql::TypeNewDecimal);
    for (datum, expected) in [
        (types::NewIntDatum(123), "123"),
        (types::NewUintDatum(123), "123"),
        (types::NewFloat32Datum(123.0), "123"),
        (types::NewFloat64Datum(123.456), "123.456"),
        (types::NewStringDatum("-123.456".to_owned()), "-123.456"),
    ] {
        assert_eq!(
            datum
                .ConvertTo(strict_context(), &decimal_field)
                .unwrap()
                .GetMysqlDecimal()
                .String(),
            expected
        );
    }
}

/// 整数字符串四舍五入。
#[test]
fn test_round_int_str() {
    assert_eq!(conv::roundIntStr(b'5', "+999"), "+1000");
    assert_eq!(conv::roundIntStr(b'5', "999"), "1000");
    assert_eq!(conv::roundIntStr(b'5', "-999"), "-1000");
}

/// 合法整数前缀提取。
#[test]
fn test_get_valid_int() {
    let cases = [
        ("100", "100", false),
        ("-100", "-100", false),
        ("9223372036854775808", "9223372036854775808", false),
        ("1abc", "1", true),
        ("-1-1", "-1", true),
        ("+1+1", "+1", true),
        ("123..34", "123", true),
        ("123.23E-10", "0", false),
        ("1.1e1.3", "11", true),
        ("11e1.3", "110", true),
        ("1.", "1", false),
        (".1", "0", false),
        ("", "0", true),
        ("123e+", "123", true),
        ("123de", "123", true),
    ];
    for (input, expected, warning) in cases {
        let warning_context =
            strict_context().WithFlags(default_stmt_flags().WithTruncateAsWarning(true));
        let (actual, error) = conv::getValidIntPrefix(warning_context, input, false);
        assert_eq!(actual, expected, "input={input}");
        assert!(error.is_none(), "warnings are appended to the context");
        let (_, strict_error) = conv::getValidIntPrefix(strict_context(), input, false);
        assert_eq!(strict_error.is_some(), warning, "input={input}");
    }
}

/// 合法浮点前缀提取。
#[test]
fn test_get_valid_float() {
    let cases = [
        ("-100", "-100", false, false),
        ("1abc", "1", false, true),
        ("-1-1", "-1", false, true),
        ("+1+1", "+1", false, true),
        ("123..34", "123.", false, true),
        ("123.23E-10", "123.23E-10", false, false),
        ("1.1e1.3", "1.1e1", false, true),
        ("11e1.3", "11e1", false, true),
        ("1.1e-13a", "1.1e-13", false, true),
        ("1.", "1.", false, false),
        (".1", ".1", false, false),
        ("", "0", false, true),
        ("", "0", true, false),
        ("123e+", "123", false, true),
        ("0-123", "0", false, true),
        ("9-3", "9", false, true),
        ("1001001\0\0\0", "1001001", false, false),
        ("5e", "5", false, false),
        ("+.e", "0", false, true),
        ("1e5e", "1e5", false, true),
        ("e", "0", false, true),
        ("e123", "0", false, true),
        ("e+", "0", false, true),
    ];
    for (input, expected, cast, expect_error) in cases {
        let (actual, error) = conv::getValidFloatPrefix(strict_context(), input, cast);
        assert_eq!(actual, expected, "input={input}");
        assert_eq!(error.is_some(), expect_error, "input={input}");
        actual.parse::<f64>().unwrap();
    }

    let integer_cases = [
        ("1e29223372036854775807", conv::maxUintStr, true),
        ("1e9223372036854775807", conv::maxUintStr, true),
        ("125e342", conv::maxUintStr, true),
        ("1e21", conv::maxUintStr, true),
        ("-1e29223372036854775807", conv::minIntStr, true),
        ("-1e9223372036854775807", conv::minIntStr, true),
        ("1e5", "100000", false),
        ("-123.45678e5", "-12345678", false),
        ("+0.5", "1", false),
        ("-0.5", "-1", false),
        (".5e0", "1", false),
        ("+.5e0", "+1", false),
        ("-.5e0", "-1", false),
        (".5", "1", false),
        ("123.456789e5", "12345679", false),
        ("123.456784e5", "12345678", false),
        ("+999.9999e2", "+100000", false),
    ];
    for (input, expected, overflow) in integer_cases {
        let (actual, error) = float_str_to_int_str_safe(input, input);
        assert_eq!(actual, expected, "input={input}");
        assert_eq!(error.is_some(), overflow, "input={input}");
    }
}

/// 时间类型转换。
#[test]
fn test_convert_time() {
    let raw = types::FromDate(2002, 3, 4, 4, 6, 7, 8);
    for (source, target) in [
        (mysql::TypeDatetime, mysql::TypeTimestamp),
        (mysql::TypeTimestamp, mysql::TypeDatetime),
    ] {
        let input = types::NewTime(raw, source, types::DefaultFsp);
        let mut datum = types::Datum::default();
        datum.SetMysqlTime(input);
        let field = types::NewFieldType(target);
        let output = datum
            .ConvertTo(strict_context(), &field)
            .unwrap()
            .GetMysqlTime();
        assert_eq!(output.Type(), target);
        assert_eq!(output.CoreTime(), raw);
    }
}

/// JSON 转整数。
#[test]
fn test_convert_json_to_int() {
    let cases = [
        ("{}", 0, true),
        ("[]", 0, true),
        ("3", 3, false),
        ("-3", -3, false),
        ("4.5", 4, false),
        ("true", 1, false),
        ("false", 0, false),
        ("null", 0, true),
        ("\"hello\"", 0, true),
        ("\"123hello\"", 123, true),
        ("\"1234\"", 1234, false),
    ];
    for (input, expected, expect_error) in cases {
        let json = types::ParseBinaryJSONFromString(input).unwrap();
        let result = json_to_int64(json);
        assert_eq!(result_value(&result), expected, "input={input}");
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }
}

/// JSON 转浮点。
#[test]
fn test_convert_json_to_float() {
    let cases = [
        ("{}", 0.0, true),
        ("[]", 0.0, true),
        ("3", 3.0, false),
        ("-3", -3.0, false),
        ("4.5", 4.5, false),
        ("true", 1.0, false),
        ("false", 0.0, false),
        ("null", 0.0, true),
        ("\"hello\"", 0.0, true),
        ("\"123.456hello\"", 123.456, true),
        ("\"1234\"", 1234.0, false),
    ];
    for (input, expected, expect_error) in cases {
        let json = types::ParseBinaryJSONFromString(input).unwrap();
        let result = json_to_float(json);
        assert_eq!(result_value(&result), expected, "input={input}");
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }
}

/// JSON 转 DECIMAL。
#[test]
fn test_convert_json_to_decimal() {
    let cases = [
        ("3", "3", false),
        ("-3", "-3", false),
        ("4.5", "4.5", false),
        ("\"1234\"", "1234", false),
        (
            "\"1234567890123456789012345678901234567890123456789012345\"",
            "1234567890123456789012345678901234567890123456789012345",
            false,
        ),
        ("true", "1", false),
        ("false", "0", false),
        ("null", "0", true),
    ];
    for (input, expected, expect_error) in cases {
        let json = types::ParseBinaryJSONFromString(input).unwrap();
        let result = json_to_decimal(json);
        let actual = match &result {
            Ok(value) => value,
            Err(error) => &error.value,
        };
        assert_eq!(
            actual.Compare(&decimal_from_text(expected)),
            0,
            "input={input}"
        );
        assert_eq!(result.is_err(), expect_error, "input={input}");
    }
}

/// 数字编码转 Duration。
#[test]
fn test_number_to_duration() {
    let cases = [
        (20_171_222, 0, true, 0, 0, 0),
        (171_222, 0, false, 17, 12, 22),
        (20_171_222_020_005, 0, false, 2, 0, 5),
        (10_000_000_000, 0, true, 0, 0, 0),
        (171_222, 1, false, 17, 12, 22),
        (176_022, 1, true, 0, 0, 0),
        (8_391_222, 1, true, 0, 0, 0),
        (8_381_222, 0, false, 838, 12, 22),
        (1_001_222, 0, false, 100, 12, 22),
        (171_260, 1, true, 0, 0, 0),
    ];
    for (number, fsp, expect_error, hour, minute, second) in cases {
        let result = number_to_duration(number, fsp);
        assert_eq!(result.is_err(), expect_error, "number={number}");
        if let Ok(duration) = result {
            assert_eq!(
                (duration.Hour(), duration.Minute(), duration.Second()),
                (hour, minute, second)
            );
        }
    }
    assert_eq!(
        number_to_duration(171_222, 0).unwrap().Duration,
        61_942_000_000_000
    );
    assert_eq!(
        number_to_duration(-171_222, 0).unwrap().Duration,
        -61_942_000_000_000
    );
}

/// 字符串转 Duration/Datetime 分支。
#[test]
fn test_str_to_duration() {
    let cases = [
        ("20190412120000", 4, false),
        ("20190101180000", 6, false),
        ("20190101180000", 1, false),
        ("20190101181234", 3, false),
        ("00:00:00.000000", 6, true),
        ("00:00:00", 0, true),
    ];
    for (input, fsp, expected) in cases {
        assert_eq!(
            str_to_duration(strict_context(), input, fsp).unwrap(),
            expected
        );
    }
}

/// 科学计数法展开。
#[test]
fn test_convert_scientific_notation() {
    let cases = [
        ("123.456e0", "123.456", true),
        ("123.456e1", "1234.56", true),
        ("123.456e3", "123456", true),
        ("123.456e4", "1234560", true),
        ("123.456e5", "12345600", true),
        ("123.456e6", "123456000", true),
        ("123.456e7", "1234560000", true),
        ("123.456e-1", "12.3456", true),
        ("123.456e-2", "1.23456", true),
        ("123.456e-3", "0.123456", true),
        ("123.456e-4", "0.0123456", true),
        ("123.456e-5", "0.00123456", true),
        ("123.456e-6", "0.000123456", true),
        ("123.456e-7", "0.0000123456", true),
        ("123.456e-", "", false),
        ("123.456e-7.5", "", false),
        ("123.456e", "", false),
    ];
    for (input, expected, success) in cases {
        let result = conv::convertScientificNotation(input);
        assert_eq!(result.is_ok(), success, "input={input}");
        assert_eq!(
            result.map_or_else(|error| error.value, |value| value),
            expected
        );
    }
}

/// DECIMAL 字符串转无符号整数。
#[test]
fn test_convert_decimal_str_to_uint() {
    let cases = [
        ("0.", 0, true),
        ("72.40", 72, true),
        ("072.40", 72, true),
        ("123.456e2", 12_346, true),
        ("123.456e-2", 1, true),
        ("072.50000000001", 73, true),
        (".5757", 1, true),
        (".12345E+4", 1235, true),
        ("9223372036854775807.5", 9_223_372_036_854_775_808, true),
        ("9223372036854775807.4999", 9_223_372_036_854_775_807, true),
        ("18446744073709551614.55", u64::MAX, true),
        ("18446744073709551615.344", u64::MAX, true),
        ("18446744073709551615.544", u64::MAX, false),
        ("-111.111", 0, false),
        ("-10000000000000000000.0", 0, false),
    ];
    for (input, expected, success) in cases {
        let result = conv::convertDecimalStrToUint(input, u64::MAX, 0);
        assert_eq!(result_value(&result), expected, "input={input}");
        assert_eq!(result.is_ok(), success, "input={input}");
    }
    for input in ["-99.0", "-100.0"] {
        let result = conv::convertDecimalStrToUint(input, u8::MAX as u64, 0);
        assert!(result.is_err());
        assert_eq!(result_value(&result), 0);
    }
}
