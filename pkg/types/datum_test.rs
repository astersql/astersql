// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Datum 存取、转换、比较与序列化的单元测试。
//
// 覆盖 ToBool/ToInt64/浮点转换、JSON、克隆、内存估算、
// 上下界修正、BIT 转换、Marshal、ProduceDec 以及 DatumsToString。

use std::any::Any;

use crate::core_time as core;
use crate::datum::*;

/// 忽略截断警告的默认语句上下文。
fn context() -> Context {
    let base = (*DefaultStmtNoWarningContext).clone();
    base.WithFlags(base.Flags().WithIgnoreTruncateErr(true))
}

/// 构造字符串 Datum。
fn string(value: &str) -> Datum {
    NewStringDatum(value.to_owned())
}

/// 构造字节 Datum。
fn bytes(value: &[u8]) -> Datum {
    NewBytesDatum(value.to_vec())
}

/// 由十进制字符串解析 MyDecimal。
fn decimal(value: &str) -> MyDecimal {
    let mut result = MyDecimal::default();
    result.FromString(value.as_bytes()).unwrap();
    result
}

/// 二进制校对下 Compare 应为 0。
fn assert_equal(left: &Datum, right: &Datum) {
    let result = left
        .Compare(context(), right, &*collate::GetBinaryCollator())
        .unwrap();
    assert_eq!(
        result,
        0,
        "left={}, right={}",
        left.String(),
        right.String()
    );
}

#[test]
/// SetValueWithDefaultCollation 对各标量类型与 interface 的写入。
fn test_datum() {
    let mut datum = Datum::default();

    datum.SetMinNotNull();
    datum.SetValueWithDefaultCollation(&1_i64);
    assert_eq!(datum.GetInt64(), 1);
    assert_eq!(datum.Length(), 0);
    assert!(datum.String().contains('1'));

    datum.SetValueWithDefaultCollation(&1_u64);
    assert_eq!(datum.GetUint64(), 1);

    datum.SetValueWithDefaultCollation(&1.1_f64);
    assert_eq!(datum.GetFloat64(), 1.1);

    datum.SetValueWithDefaultCollation(&"abc".to_owned());
    assert_eq!(datum.GetString(), "abc");

    datum.SetValueWithDefaultCollation(&b"abc".to_vec());
    assert_eq!(datum.GetBytes(), b"abc");

    // Go 的默认 type-switch 将未知切片保存在 interface{}；Rust 用显式 Any 表达同一所有权。
    datum.SetInterface(Box::new(vec![1_i32]));
    assert_eq!(
        datum
            .GetInterface()
            .unwrap()
            .downcast_ref::<Vec<i32>>()
            .unwrap(),
        &vec![1]
    );
}

/// 断言 ToBool 结果。
fn assert_bool(datum: Datum, expected: i64) {
    assert_eq!(
        datum.ToBool(context()).unwrap(),
        expected,
        "{}",
        datum.String()
    );
}

#[test]
/// 各 Kind 转布尔：零值为 0，非零/非空 JSON 等为 1。
fn test_to_bool() {
    for datum in [
        // 逻辑假：数值 0、空串、零字面量、JSON 数字 0
        NewIntDatum(0),
        NewUintDatum(0),
        NewFloat32Datum(0.0),
        NewFloat64Datum(0.0),
        string(""),
        bytes(b""),
        NewBinaryLiteralDatum(NewBinaryLiteralFromUint(0, -1)),
        NewJSONDatum(ParseBinaryJSONFromString("0").unwrap()),
        NewJSONDatum(ParseBinaryJSONFromString("0.0").unwrap()),
    ] {
        assert_bool(datum, 0);
    }

    for datum in [
        NewFloat32Datum(0.1),
        NewFloat64Datum(0.1),
        NewFloat64Datum(0.5),
        NewFloat64Datum(0.499),
        string("0.1"),
        bytes(b"0.1"),
        NewMysqlEnumDatum(Enum {
            Name: "a".to_owned(),
            Value: 1,
        }),
        NewMysqlSetDatum(
            Set {
                Name: "a".to_owned(),
                Value: 1,
            },
            "binary".to_owned(),
        ),
    ] {
        assert_bool(datum, 1);
    }

    for json in [
        "1",
        "\"0\"",
        "\"aaabbb\"",
        "[1,2]",
        "{\"ke\":\"val\"}",
        "\"0000-00-00 00:00:00\"",
        "\"0778\"",
        "\"0000\"",
        "null",
        "[null]",
        "true",
        "false",
        "\"\"",
    ] {
        assert_bool(NewJSONDatum(ParseBinaryJSONFromString(json).unwrap()), 1);
    }

    let time = ParseTime(
        &time_context(&context()),
        "2011-11-10 11:11:11.999999",
        mysql::TypeTimestamp,
        6,
    )
    .unwrap();
    assert_bool(NewTimeDatum(time), 1);

    let (duration, _) = ParseDuration(&time_context(&context()), "11:11:11.999999", 6).unwrap();
    assert_bool(NewDurationDatum(duration), 1);

    assert_bool(NewDecimalDatum(decimal("0.14159")), 1);

    let mut invalid = Datum::default();
    invalid.SetInterface(Box::new(vec![1_i32]));
    assert!(invalid.ToBool(context()).is_err());
}

/// 断言 ToInt64 结果。
fn assert_int(datum: Datum, expected: i64) {
    assert_eq!(datum.ToInt64(context()).unwrap(), expected);
}

#[test]
/// 多类型 ToInt64 转换对照。
fn test_to_int64() {
    assert_int(string("0"), 0);
    assert_int(NewIntDatum(0), 0);
    assert_int(NewUintDatum(0), 0);
    assert_int(NewFloat32Datum(3.1), 3);
    assert_int(NewFloat64Datum(3.1), 3);
    assert_int(
        NewBinaryLiteralDatum(NewBinaryLiteralFromUint(100, -1)),
        100,
    );
    assert_int(
        NewMysqlEnumDatum(Enum {
            Name: "a".to_owned(),
            Value: 1,
        }),
        1,
    );
    assert_int(
        NewMysqlSetDatum(
            Set {
                Name: "a".to_owned(),
                Value: 1,
            },
            "binary".to_owned(),
        ),
        1,
    );
    assert_int(NewJSONDatum(ParseBinaryJSONFromString("3").unwrap()), 3);

    let time = ParseTime(
        &time_context(&context()),
        "2011-11-10 11:11:11.999999",
        mysql::TypeTimestamp,
        0,
    )
    .unwrap();
    assert_int(NewTimeDatum(time), 20_111_110_111_112);
    let (duration, _) = ParseDuration(&time_context(&context()), "11:11:11.999999", 6).unwrap();
    assert_int(NewDurationDatum(duration), 111_112);
    assert_int(NewDecimalDatum(decimal("3.14159")), 3);
}

#[test]
/// 转为无符号 32 位整数的边界与截断。
fn test_to_uint32() {
    let mut target = NewFieldType(mysql::TypeLong);
    target.AddFlag(mysql::UnsignedFlag);
    for datum in [
        NewIntDatum(5_000_000_000),
        NewIntDatum(-1),
        string("5000000000"),
    ] {
        assert!(datum.ConvertTo(context(), &target).is_err());
    }
    for (datum, expected) in [
        (NewIntDatum(12_345), 12_345_u64),
        (NewIntDatum(0), 0),
        (NewIntDatum(2_147_483_648), 2_147_483_648),
        (
            NewMysqlEnumDatum(Enum {
                Name: "a".to_owned(),
                Value: 1,
            }),
            1,
        ),
        (
            NewMysqlSetDatum(
                Set {
                    Name: "a".to_owned(),
                    Value: 1,
                },
                "binary".to_owned(),
            ),
            1,
        ),
    ] {
        let converted = datum.ConvertTo(context(), &target).unwrap();
        assert_eq!(converted.Kind(), KindUint64);
        assert_eq!(converted.GetUint64(), expected);
    }
}

#[test]
/// ConvertTo 浮点目标类型。
fn test_convert_to_float() {
    for (datum, target_type, expected) in [
        (NewFloat32Datum(3.0), mysql::TypeDouble, 3.0),
        (NewFloat64Datum(12_345.678), mysql::TypeDouble, 12_345.678),
        (string("12345.678"), mysql::TypeDouble, 12_345.678),
        (bytes(b"12345.678"), mysql::TypeDouble, 12_345.678),
        (NewIntDatum(12_345), mysql::TypeDouble, 12_345.0),
        (NewUintDatum(123_456), mysql::TypeDouble, 123_456.0),
        (NewFloat32Datum(281.37), mysql::TypeFloat, 281.37),
        (string("281.37"), mysql::TypeFloat, 281.37),
    ] {
        let converted = datum
            .ConvertTo(context(), &NewFieldType(target_type))
            .unwrap();
        if target_type == mysql::TypeDouble {
            assert_eq!(converted.GetFloat64(), expected);
        } else {
            assert_eq!(converted.GetFloat32(), expected as f32);
        }
    }

    let mut unsupported = Datum::default();
    unsupported.SetInterface(Box::new(123_u8));
    assert!(
        unsupported
            .ConvertTo(context(), &NewFieldType(mysql::TypeDouble))
            .is_err()
    );
    for value in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        assert!(
            NewFloat64Datum(value)
                .ConvertTo(context(), &NewFieldType(mysql::TypeDouble))
                .is_err()
        );
    }
}

#[test]
/// ToMysqlJSON / 相关 JSON 转换。
fn test_to_json() {
    let target = NewFieldType(mysql::TypeJSON);
    for (datum, expected) in [
        (NewIntDatum(1), "1"),
        (NewFloat64Datum(2.0), "2.0"),
        (string("\"hello, 世界\""), "\"hello, 世界\""),
        (string("[1, 2, 3]"), "[1,2,3]"),
        (string("{}"), "{}"),
        (
            string("{\"a\": \"9223372036854775809\"}"),
            "{\"a\":\"9223372036854775809\"}",
        ),
    ] {
        let actual = datum.ConvertTo(context(), &target).unwrap().GetMysqlJSON();
        let expected = ParseBinaryJSONFromString(expected).unwrap();
        assert_eq!(CompareBinaryJSON(&actual, &expected), 0);
    }

    let time = ParseTime(
        &time_context(&context()),
        "2011-11-10 11:11:11.111111",
        mysql::TypeTimestamp,
        6,
    )
    .unwrap();
    let actual = NewTimeDatum(time).ConvertTo(context(), &target).unwrap();
    assert_eq!(actual.GetMysqlJSON().TypeCode, JSONTypeCodeTimestamp);

    assert!(
        NewBinaryLiteralDatum(BinaryLiteral(vec![0x81]))
            .ConvertTo(context(), &target)
            .is_err()
    );
    assert!(string("hello, 世界").ConvertTo(context(), &target).is_err());
}

#[test]
/// IsNull 与 SetNull。
fn test_is_null() {
    assert!(Datum::default().IsNull());
    for datum in [
        NewIntDatum(0),
        NewIntDatum(1),
        NewFloat64Datum(1.1),
        string("string"),
        string(""),
    ] {
        assert!(!datum.IsNull());
    }
}

#[test]
/// ToBytes 输出。
fn test_to_bytes() {
    for (datum, expected) in [
        (NewIntDatum(1), b"1".to_vec()),
        (NewDecimalDatum(decimal("1")), b"1".to_vec()),
        (NewFloat64Datum(1.23), b"1.23".to_vec()),
        (string("abc"), b"abc".to_vec()),
        (Datum::default(), Vec::new()),
    ] {
        assert_eq!(datum.ToBytes().unwrap(), expected);
    }
}

#[test]
/// ComputePlus 加法路径（与减法相关断言）。
fn test_compute_plus_and_minus() {
    for (left, right, expected) in [
        (
            core::NewIntDatum(72),
            core::NewIntDatum(28),
            core::NewIntDatum(100),
        ),
        (
            core::NewIntDatum(72),
            core::NewUintDatum(28),
            core::NewUintDatum(100),
        ),
        (
            core::NewUintDatum(72),
            core::NewUintDatum(28),
            core::NewUintDatum(100),
        ),
        (
            core::NewUintDatum(72),
            core::NewIntDatum(28),
            core::NewUintDatum(100),
        ),
        (
            core::NewFloat64Datum(72.0),
            core::NewFloat64Datum(28.0),
            core::NewFloat64Datum(100.0),
        ),
        (
            core::NewDecimalDatum(core::NewDecFromStringForTest("72.5")),
            core::NewDecimalDatum(core::NewDecFromStringForTest("3")),
            core::NewDecimalDatum(core::NewDecFromStringForTest("75.5")),
        ),
    ] {
        assert_eq!(core::ComputePlus(left, right).unwrap(), expected);
    }
    assert!(core::ComputePlus(core::NewIntDatum(72), core::NewFloat64Datum(42.0)).is_err());
    assert!(core::ComputePlus(core::NewStringDatum("abcd"), core::NewIntDatum(42)).is_err());
}

#[test]
/// Clone/Copy 深浅拷贝语义。
fn test_clone_datum() {
    for datum in [
        NewIntDatum(72),
        NewUintDatum(72),
        string("abcd"),
        bytes(b"abcd"),
        {
            let mut raw = Datum::default();
            raw.SetRaw(b"raw".to_vec());
            raw
        },
    ] {
        let cloned = datum.Clone();
        if datum.Kind() == KindString || datum.Kind() == KindBytes || datum.Kind() == KindRaw {
            let original_bytes = datum.GetBytes();
            let cloned_bytes = cloned.GetBytes();
            assert_ne!(original_bytes.as_ptr(), cloned_bytes.as_ptr());
        }
        if datum.Kind() == KindRaw {
            assert_eq!(datum.GetRaw(), cloned.GetRaw());
        } else {
            assert_equal(&datum, &cloned);
        }
    }
}

/// 构造带符号/长度/小数位的 FieldType。
fn type_with(tp: u8, unsigned: bool, flen: isize, decimal: isize) -> FieldType {
    let mut field = NewFieldType(tp);
    if unsigned {
        field.AddFlag(mysql::UnsignedFlag);
    }
    field.SetFlen(flen);
    field.SetDecimal(decimal);
    *field
}

#[test]
/// EstimatedMemUsage 按行估算。
fn test_estimated_mem_usage() {
    let bytes = b"abcd".to_vec();
    let enum_value = Enum {
        Name: "a".to_owned(),
        Value: 1,
    };
    let datums = vec![
        NewIntDatum(1),
        NewFloat64Datum(1.0),
        NewFloat32Datum(1.0),
        string("abcd"),
        NewBytesDatum(bytes.clone()),
        NewDecimalDatum(decimal("1234.1234")),
        NewMysqlEnumDatum(enum_value.clone()),
    ];
    let expected = 10
        * (datums.len() as i64 * i64::from(sizeOfEmptyDatum)
            + i64::from(sizeOfMyDecimal)
            + (bytes.len() * 2 + enum_value.Name.len()) as i64);
    assert_eq!(EstimatedMemUsage(&datums, 10), expected);
}

#[test]
/// 反转结果按类型上下界修正（Ceiling/Floor）。
fn test_change_reverse_result_by_upper_lower_bound() {
    let cases = vec![
        (
            NewIntDatum(1),
            NewUintDatum(2),
            type_with(mysql::TypeLonglong, true, 0, 0),
            Ceiling,
        ),
        (
            NewIntDatum(1),
            NewUintDatum(1),
            type_with(mysql::TypeLonglong, true, 0, 0),
            Floor,
        ),
        (
            NewIntDatum(i64::MAX),
            NewUintDatum(u64::MAX),
            type_with(mysql::TypeLonglong, true, 0, 0),
            Ceiling,
        ),
        (
            NewIntDatum(i64::MAX),
            NewUintDatum(i64::MAX as u64),
            type_with(mysql::TypeLonglong, true, 0, 0),
            Floor,
        ),
        (
            NewIntDatum(1),
            NewFloat64Datum(2.0),
            type_with(mysql::TypeDouble, false, 23, UnspecifiedLength as isize),
            Ceiling,
        ),
        (
            NewIntDatum(1),
            NewFloat64Datum(1.0),
            type_with(mysql::TypeDouble, false, 23, UnspecifiedLength as isize),
            Floor,
        ),
        {
            let target = type_with(mysql::TypeDouble, false, 23, UnspecifiedLength as isize);
            let expected = GetMaxValue(&target);
            (NewIntDatum(i64::MAX), expected, target, Ceiling)
        },
        (
            NewIntDatum(i64::MAX),
            NewFloat64Datum(i64::MAX as f64),
            type_with(mysql::TypeDouble, false, 23, UnspecifiedLength as isize),
            Floor,
        ),
        (
            NewIntDatum(1),
            NewDecimalDatum(decimal("2")),
            type_with(mysql::TypeNewDecimal, false, 30, 3),
            Ceiling,
        ),
        (
            NewIntDatum(1),
            NewDecimalDatum(decimal("1")),
            type_with(mysql::TypeNewDecimal, false, 30, 3),
            Floor,
        ),
        {
            let target = type_with(mysql::TypeNewDecimal, false, 30, 3);
            let expected = GetMaxValue(&target);
            (NewIntDatum(i64::MAX), expected, target, Ceiling)
        },
        (
            NewIntDatum(i64::MAX),
            NewDecimalDatum(decimal("9223372036854775807")),
            type_with(mysql::TypeNewDecimal, false, 30, 3),
            Floor,
        ),
    ];
    for (source, expected, target, rounding) in cases {
        let actual =
            ChangeReverseResultByUpperLowerBound(context(), &target, source, rounding).unwrap();
        assert_equal(&actual, &expected);
    }
}

#[test]
/// 字符串转 MySQL BIT。
fn test_string_to_mysql_bit() {
    for (input, flen, truncated, expected) in [
        ("true", 1, true, vec![1]),
        ("true", 32, false, b"true".to_vec()),
        ("false", 1, true, vec![1]),
        ("false", 40, false, b"false".to_vec()),
        ("1", 1, true, vec![1]),
        ("1", 8, false, b"1".to_vec()),
        ("0", 1, true, vec![1]),
        ("0", 8, false, b"0".to_vec()),
        ("b'1'", 32, false, b"b'1'".to_vec()),
        ("b'0'", 32, false, b"b'0'".to_vec()),
    ] {
        let mut target = NewFieldType(mysql::TypeBit);
        target.SetFlen(flen);
        let result = string(input).convertToMysqlBit(context(), &target);
        assert_eq!(result.is_err(), truncated);
        if let Ok(result) = result {
            assert_eq!(result.GetBinaryLiteral().0, expected);
        }
    }
}

#[test]
/// JSON Marshal/Unmarshal 往返。
fn test_marshal_datum() {
    let set = ParseSetValue(
        &["a", "b", "c", "d", "e"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        1,
    )
    .unwrap();
    let datums = vec![
        NewIntDatum(1),
        NewUintDatum(72),
        NewFloat32Datum(1.23),
        NewFloat64Datum(1.23),
        NewFloat64Datum(f64::NEG_INFINITY),
        NewDecimalDatum(decimal("1.2345")),
        string("abcde"),
        NewCollationStringDatum("abcde".to_owned(), "binary".to_owned()),
        NewDurationDatum(Duration {
            Duration: 1,
            Fsp: 0,
        }),
        NewTimeDatum(NewTime(
            FromDate(2018, 3, 8, 16, 1, 0, 315_313),
            mysql::TypeTimestamp,
            6,
        )),
        bytes(b"abcde"),
        NewBinaryLiteralDatum(BinaryLiteral(vec![0x81])),
        NewMysqlBitDatum(NewBinaryLiteralFromUint(0x9876_5432, 4)),
        NewMysqlEnumDatum(Enum {
            Name: "a".to_owned(),
            Value: 1,
        }),
        NewCollateMysqlEnumDatum(
            Enum {
                Name: "a".to_owned(),
                Value: 1,
            },
            "ascii_bin".to_owned(),
        ),
        NewMysqlSetDatum(set, "gbk_bin".to_owned()),
        NewJSONDatum(CreateBinaryJSON(1_i64)),
        MinNotNullDatum(),
        MaxValueDatum(),
    ];
    for datum in datums {
        let encoded = datum.MarshalJSON().unwrap();
        let mut decoded = Datum::default();
        decoded.UnmarshalJSON(&encoded).unwrap();
        assert!(datum.Equals(&decoded as &dyn Any), "{}", datum.String());
    }
}

#[test]
/// ProduceDecWithSpecifiedTp 精度/标度裁剪。
fn test_produce_dec_with_specified_tp() {
    for (input, flen, frac, expected, overflow) in [
        ("0.0000", 4, 3, "0.000", false),
        ("0.0001", 4, 3, "0.000", false),
        (
            "0.000000000000000000000000000000",
            30,
            30,
            "0.000000000000000000000000000000",
            false,
        ),
        (
            "0.400000000000000000000000000000",
            30,
            30,
            "0.400000000000000000000000000000",
            false,
        ),
        ("123", 8, 5, "123.00000", false),
        ("-123", 8, 5, "-123.00000", false),
        ("123.899", 5, 2, "123.90", false),
        ("-123.899", 5, 2, "-123.90", false),
        ("123.899", 6, 2, "123.90", false),
        ("-123.899", 6, 2, "-123.90", false),
        ("123.99", 4, 1, "124.0", false),
        ("123.99", 3, 0, "124", false),
        ("-123.99", 3, 0, "-124", false),
        ("123.99", 3, 1, "99.9", true),
        ("-123.99", 3, 1, "-99.9", true),
        ("99.9999", 5, 3, "99.999", true),
        ("-99.9999", 5, 3, "-99.999", true),
        ("99.9999", 6, 3, "100.000", false),
        ("-99.9999", 6, 3, "-100.000", false),
    ] {
        let target = type_with(mysql::TypeNewDecimal, false, flen, frac);
        let (actual, error) = ProduceDecWithSpecifiedTp(context(), decimal(input), &target);
        assert_eq!(error.is_some(), overflow, "input={input}");
        assert_eq!(actual.unwrap().String(), expected, "input={input}");
    }
}

#[test]
/// NULL 与其它值比较不相等。
fn test_null_not_equal_with_others() {
    let values = vec![
        NewIntDatum(0),
        NewUintDatum(0),
        NewFloat32Datum(0.0),
        NewFloat64Datum(0.0),
        NewFloat64Datum(f64::INFINITY),
        NewDecimalDatum(decimal("0")),
        string(""),
        NewCollationStringDatum(String::new(), "binary".to_owned()),
        NewDurationDatum(Duration {
            Duration: 0,
            Fsp: 0,
        }),
        NewTimeDatum(Time::default()),
        bytes(b""),
        NewBinaryLiteralDatum(BinaryLiteral(vec![])),
        NewMysqlBitDatum(NewBinaryLiteralFromUint(0, 4)),
        NewJSONDatum(ParseBinaryJSONFromString("null").unwrap()),
        MinNotNullDatum(),
        MaxValueDatum(),
    ];
    let null = Datum::default();
    for value in values {
        assert_ne!(
            value
                .Compare(context(), &null, &*collate::GetBinaryCollator())
                .unwrap(),
            0
        );
    }
}

#[test]
/// DatumsToString 行文本格式。
fn test_datums_to_string() {
    let datums = vec![
        NewIntDatum(1),
        NewUintDatum(2),
        NewFloat32Datum(-3.1111111),
        NewFloat64Datum(4.123),
        NewFloat64Datum(f64::INFINITY),
        NewDecimalDatum(decimal("6.6")),
        string("abc"),
        NewCollationStringDatum(String::new(), "binary".to_owned()),
        NewDurationDatum(Duration {
            Duration: 11_111,
            Fsp: 0,
        }),
        NewTimeDatum(Time::default()),
        bytes(b"xxx"),
        NewBinaryLiteralDatum(BinaryLiteral(vec![])),
        NewJSONDatum(ParseBinaryJSONFromString("null").unwrap()),
        MinNotNullDatum(),
        MaxValueDatum(),
    ];
    assert_eq!(
        DatumsToString(&datums, true).unwrap(),
        "(1, 2, -3.1111112, 4.123, +Inf, 6.6, \"abc\", \"\", 00:00:00, 0000-00-00 00:00:00, xxx, , null, -inf, +inf)"
    );
}

#[test]
/// isPrintable 可打印字符判定。
fn test_is_printable() {
    assert!(isPrintable("abc"));
    assert!(!isPrintable("a\0bc"));
    assert!(isPrintable("abcé"));
    let invalid = vec![0x61, 0x62, 0x63, 0xc3];
    assert!(std::str::from_utf8(&invalid).is_err());
}
