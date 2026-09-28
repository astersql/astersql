// Copyright 2015 PingCAP, Inc.
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

// Codec 包核心行为的表驱动单元测试。
//
// 对应 Go `pkg/util/codec/codec_test.go`：覆盖 key/value 编解码、数字有序性、
// 浮点/字节/时间/Duration/Decimal/JSON、Cut/SetRawValues、DecodeOneToChunk、
// HashGroup/HashChunkRow/HashChunkColumns 以及 Datum Hash64 相等性。

// 本文件由 pkg/util/codec/codec_test.go 迁移而来，覆盖 key/value、数字、时间、
// decimal、JSON、切分、Chunk hash 和 Datum hash 行为。

use super::*;
use base::{Equals as _, Hash64 as _, Hasher as _};
use std::any::Any;
use std::hash::Hasher as StdHasher;

/// 编解码往返用例：输入 Datum 与期望解码结果。
struct CodecDatumCase {
    input: Vec<types::Datum>,
    expect: Vec<types::Datum>,
}

/// 构造 Int64 Datum。
fn int(value: i64) -> types::Datum {
    types::NewIntDatum(value)
}

/// 构造 Uint64 Datum。
fn uint(value: u64) -> types::Datum {
    types::NewUintDatum(value)
}

/// 构造 Float32 Datum。
fn float32(value: f32) -> types::Datum {
    types::NewFloat32Datum(value)
}

/// 构造 Float64 Datum。
fn float64(value: f64) -> types::Datum {
    types::NewFloat64Datum(value)
}

/// 构造 String Datum。
fn string(value: &str) -> types::Datum {
    types::NewStringDatum(value.to_owned())
}

/// 构造 Bytes Datum。
fn bytes(value: &[u8]) -> types::Datum {
    types::NewBytesDatum(value.to_vec())
}

/// 构造 MySQL Enum Datum。
fn enum_datum(name: &str, value: u64) -> types::Datum {
    types::NewMysqlEnumDatum(types::Enum {
        Name: name.to_owned(),
        Value: value,
    })
}

/// 构造 MySQL Set Datum。
fn set_datum(name: &str, value: u64) -> types::Datum {
    types::NewMysqlSetDatum(
        types::Set {
            Name: name.to_owned(),
            Value: value,
        },
        "utf8mb4_bin".to_owned(),
    )
}

/// 构造带 Unsigned 标志的字段类型。
fn unsigned_field(tp: u8) -> types::FieldType {
    let mut field = types::NewFieldType(tp);
    field.AddFlag(mysql::UnsignedFlag);
    *field
}

/// EncodeKey/EncodeValue 共用的表驱动用例集。
fn codec_cases() -> Vec<CodecDatumCase> {
    vec![
        CodecDatumCase {
            input: vec![int(1)],
            expect: vec![int(1)],
        },
        CodecDatumCase {
            input: vec![float32(1.0), float64(3.15), bytes(b"123"), string("123")],
            expect: vec![float64(1.0), float64(3.15), bytes(b"123"), bytes(b"123")],
        },
        CodecDatumCase {
            input: vec![uint(1), float64(3.15), bytes(b"123"), int(-1)],
            expect: vec![uint(1), float64(3.15), bytes(b"123"), int(-1)],
        },
        CodecDatumCase {
            input: vec![int(1), int(0)],
            expect: vec![int(1), int(0)],
        },
        CodecDatumCase {
            input: vec![types::Datum::default()],
            expect: vec![types::Datum::default()],
        },
        CodecDatumCase {
            input: vec![
                types::NewBinaryLiteralDatum(types::NewBinaryLiteralFromUint(100, -1)),
                types::NewBinaryLiteralDatum(types::NewBinaryLiteralFromUint(100, 4)),
            ],
            expect: vec![uint(100), uint(100)],
        },
        CodecDatumCase {
            input: vec![enum_datum("a", 1), set_datum("a", 1)],
            expect: vec![uint(1), uint(1)],
        },
    ]
}

/// 字节序比较，返回 -1/0/1。
fn compare_bytes(left: &[u8], right: &[u8]) -> i32 {
    use std::cmp::Ordering;
    match left.cmp(right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 用 binary collation 断言两组 Datum 逻辑相等。
fn assert_datums_equal(expect: &[types::Datum], actual: &[types::Datum]) {
    assert_eq!(expect.len(), actual.len());
    for (expect, actual) in expect.iter().zip(actual) {
        let cmp = actual
            .Compare(
                (*types::DefaultStmtNoWarningContext).clone(),
                expect,
                collate::GetBinaryCollator().as_ref(),
            )
            .expect("datum comparison must succeed");
        assert_eq!(cmp, 0);
    }
}

/// 累加多个 Datum 的 EstimateValueSize。
fn estimate_values_size(
    type_ctx: types::Context,
    values: &[types::Datum],
) -> Result<usize, errors::SharedError> {
    values.iter().try_fold(0, |size, value| {
        EstimateValueSize(type_ctx.clone(), value.clone()).map(|length| size + length)
    })
}

#[test]
/// 验证 EncodeKey/EncodeValue 往返与尺寸估算。
fn TestCodecKey() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    for case in codec_cases() {
        let key = EncodeKey(type_ctx.Location(), Vec::new(), case.input.clone()).unwrap();
        assert_datums_equal(&case.expect, &Decode(key, 1).unwrap());

        let value = EncodeValue(type_ctx.Location(), Vec::new(), case.input.clone()).unwrap();
        assert_eq!(
            value.len(),
            estimate_values_size(type_ctx.clone(), &case.input).unwrap()
        );
        assert_datums_equal(&case.expect, &Decode(value, 1).unwrap());
    }

    let mut raw = types::Datum::default();
    raw.SetRaw(b"raw".to_vec());
    assert!(EncodeKey(type_ctx.Location(), Vec::new(), vec![raw]).is_err());
}

#[test]
/// 验证 memcomparable key 的字典序与原始比较一致。
fn TestCodecKeyCompare() {
    let cases = vec![
        (vec![int(1)], vec![int(1)], 0),
        (vec![int(-1)], vec![int(1)], -1),
        (vec![float64(3.15)], vec![float64(3.12)], 1),
        (vec![string("abc")], vec![string("abcd")], -1),
        (
            vec![int(1), string("abc"), string("def")],
            vec![int(1), string("abcd"), string("af")],
            -1,
        ),
        (
            vec![bytes(&[1, 0]), bytes(&[0xff])],
            vec![bytes(&[1, 0, 0xff])],
            -1,
        ),
        (vec![int(0)], vec![types::Datum::default()], 1),
        (
            vec![
                types::NewTimeDatum(parse_time("2011-11-11 00:00:00")),
                int(1),
            ],
            vec![
                types::NewTimeDatum(parse_time("2011-11-11 00:00:00")),
                int(0),
            ],
            1,
        ),
        (
            vec![types::NewDurationDatum(parse_duration("00:00:00")), int(1)],
            vec![types::NewDurationDatum(parse_duration("00:00:01")), int(0)],
            -1,
        ),
        (
            vec![types::MinNotNullDatum()],
            vec![types::MaxValueDatum()],
            -1,
        ),
    ];
    for (left, right, expect) in cases {
        let left = EncodeKey(time::UTC, Vec::new(), left).unwrap();
        let right = EncodeKey(time::UTC, Vec::new(), right).unwrap();
        assert_eq!(compare_bytes(&left, &right), expect);
    }
}

#[test]
/// 整数/无符号整数编解码向量。
fn TestNumberCodec() {
    let signed = [
        i64::MIN,
        i32::MIN as i64,
        i16::MIN as i64,
        i8::MIN as i64,
        0,
        i8::MAX as i64,
        i16::MAX as i64,
        i32::MAX as i64,
        i64::MAX,
        (1_i64 << 47) - 1,
        -1_i64 << 47,
        (1_i64 << 23) - 1,
        -1_i64 << 23,
        (1_i64 << 33) - 1,
        -1_i64 << 33,
        (1_i64 << 55) - 1,
        -1_i64 << 55,
        1,
        -1,
    ];
    for value in signed {
        assert_eq!(DecodeInt(&EncodeInt(Vec::new(), value)).unwrap().1, value);
        assert_eq!(
            DecodeIntDesc(&EncodeIntDesc(Vec::new(), value)).unwrap().1,
            value
        );
        assert_eq!(
            DecodeVarint(&EncodeVarint(Vec::new(), value)).unwrap().1,
            value
        );
        assert_eq!(
            DecodeComparableVarint(&EncodeComparableVarint(Vec::new(), value))
                .unwrap()
                .1,
            value
        );
    }

    let unsigned = [
        0,
        u8::MAX as u64,
        u16::MAX as u64,
        u32::MAX as u64,
        u64::MAX,
        (1_u64 << 24) - 1,
        (1_u64 << 48) - 1,
        (1_u64 << 56) - 1,
        1,
        i16::MAX as u64,
        i8::MAX as u64,
        i32::MAX as u64,
        i64::MAX as u64,
    ];
    for value in unsigned {
        assert_eq!(DecodeUint(&EncodeUint(Vec::new(), value)).unwrap().1, value);
        assert_eq!(
            DecodeUintDesc(&EncodeUintDesc(Vec::new(), value))
                .unwrap()
                .1,
            value
        );
        assert_eq!(
            DecodeUvarint(&EncodeUvarint(Vec::new(), value)).unwrap().1,
            value
        );
        assert_eq!(
            DecodeComparableUvarint(&EncodeComparableUvarint(Vec::new(), value))
                .unwrap()
                .1,
            value
        );
    }

    let encoded = EncodeComparableVarint(Vec::new(), -1);
    let encoded = EncodeComparableUvarint(encoded, 1);
    let encoded = EncodeComparableVarint(encoded, 2);
    let (remain, first) = DecodeComparableVarint(&encoded).unwrap();
    let (remain, second) = DecodeComparableUvarint(remain).unwrap();
    let (remain, third) = DecodeComparableVarint(remain).unwrap();
    // Go's inline signed decoder returns its original input slice.
    assert_eq!(remain, EncodeComparableVarint(Vec::new(), third));
    assert_eq!((first, second, third), (-1, 1, 2));
}

#[test]
/// 整数编码保持数值有序。
fn TestNumberOrder() {
    let signed = [
        (-1, 1, -1),
        (i64::MAX, i64::MIN, 1),
        (i64::MAX, i32::MAX as i64, 1),
        (i32::MIN as i64, i16::MAX as i64, -1),
        (1, -1, 1),
        (0, 0, 0),
    ];
    for (left, right, expect) in signed {
        assert_eq!(
            compare_bytes(&EncodeInt(Vec::new(), left), &EncodeInt(Vec::new(), right)),
            expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeIntDesc(Vec::new(), left),
                &EncodeIntDesc(Vec::new(), right)
            ),
            -expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeComparableVarint(Vec::new(), left),
                &EncodeComparableVarint(Vec::new(), right)
            ),
            expect
        );
    }

    let unsigned = [
        (0, 0, 0),
        (1, 0, 1),
        (0, 1, -1),
        (u32::MAX as u64, i32::MAX as u64, 1),
        (u64::MAX, 0, 1),
        (0, u64::MAX, -1),
    ];
    for (left, right, expect) in unsigned {
        assert_eq!(
            compare_bytes(
                &EncodeUint(Vec::new(), left),
                &EncodeUint(Vec::new(), right)
            ),
            expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeUintDesc(Vec::new(), left),
                &EncodeUintDesc(Vec::new(), right)
            ),
            -expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeComparableUvarint(Vec::new(), left),
                &EncodeComparableUvarint(Vec::new(), right)
            ),
            expect
        );
    }
}

#[test]
/// 浮点编解码与特殊值（NaN/Inf/-0）。
fn TestFloatCodec() {
    let values = [
        -1.0,
        0.0,
        1.0,
        f64::MAX,
        f32::MAX as f64,
        f32::MIN_POSITIVE as f64,
        f64::MIN_POSITIVE,
        f64::NEG_INFINITY,
        f64::INFINITY,
    ];
    for value in values {
        assert_eq!(
            DecodeFloat(&EncodeFloat(Vec::new(), value)).unwrap().1,
            value
        );
        assert_eq!(
            DecodeFloatDesc(&EncodeFloatDesc(Vec::new(), value))
                .unwrap()
                .1,
            value
        );
    }
    for (left, right, expect) in [
        (1.0, -1.0, 1),
        (0.0, 0.0, 0),
        (f64::MAX, f64::MIN_POSITIVE, 1),
        (f64::NEG_INFINITY, f64::INFINITY, -1),
    ] {
        assert_eq!(
            compare_bytes(
                &EncodeFloat(Vec::new(), left),
                &EncodeFloat(Vec::new(), right)
            ),
            expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeFloatDesc(Vec::new(), left),
                &EncodeFloatDesc(Vec::new(), right)
            ),
            -expect
        );
    }
}

#[test]
/// Bytes/CompactBytes 编解码。
fn TestBytes() {
    let values = vec![
        vec![],
        vec![0, 1],
        vec![0xff, 0xff],
        vec![1, 0],
        b"abc".to_vec(),
        b"hello world".to_vec(),
    ];
    for value in values {
        assert_eq!(
            DecodeBytes(&EncodeBytes(Vec::new(), &value), None)
                .unwrap()
                .1,
            value
        );
        assert_eq!(
            DecodeBytesDesc(&EncodeBytesDesc(Vec::new(), &value), None)
                .unwrap()
                .1,
            value
        );
        assert_eq!(
            DecodeCompactBytes(&EncodeCompactBytes(Vec::new(), &value))
                .unwrap()
                .1,
            value
        );
    }
    for (left, right, expect) in [
        (vec![], vec![0], -1),
        (vec![0], vec![0], 0),
        (vec![0xff], vec![0], 1),
        (b"a".to_vec(), b"b".to_vec(), -1),
        (vec![1, 2, 3, 0], vec![1, 2, 3], 1),
    ] {
        assert_eq!(
            compare_bytes(
                &EncodeBytes(Vec::new(), &left),
                &EncodeBytes(Vec::new(), &right)
            ),
            expect
        );
        assert_eq!(
            compare_bytes(
                &EncodeBytesDesc(Vec::new(), &left),
                &EncodeBytesDesc(Vec::new(), &right)
            ),
            -expect
        );
    }
}

/// 解析测试用时间字符串。
fn parse_time(value: &str) -> types::Time {
    let context = types::BasicTimeContext::default();
    types::ParseTime(&context, value, mysql::TypeDatetime, types::DefaultFsp).unwrap()
}

/// 解析测试用 Duration 字符串。
fn parse_duration(value: &str) -> types::Duration {
    let context = types::BasicTimeContext::default();
    types::ParseDuration(&context, value, types::DefaultFsp)
        .unwrap()
        .0
}

/// 从 u64 构造测试用 Decimal。
fn decimal_from_uint(value: u64) -> types::MyDecimal {
    let mut decimal = types::MyDecimal::default();
    decimal.FromUint(value);
    decimal
}

#[test]
/// MySQL 时间类型编解码与时区。
fn TestTime() {
    for value in [
        "2011-01-01 00:00:00",
        "2011-01-01 00:00:00",
        "0001-01-01 00:00:00",
    ] {
        let expected = parse_time(value);
        let encoded =
            EncodeKey(time::UTC, Vec::new(), vec![types::NewTimeDatum(expected)]).unwrap();
        let decoded = Decode(encoded, 1).unwrap();
        let mut actual = types::Time::default();
        actual.SetType(mysql::TypeDatetime);
        actual.FromPackedUint(decoded[0].GetUint64()).unwrap();
        assert_eq!(expected.String(), actual.String());
    }
    for (left, right, expect) in [
        ("2011-10-10 00:00:00", "2000-12-12 11:11:11", 1),
        ("2000-10-10 00:00:00", "2001-10-10 00:00:00", -1),
        ("2000-10-10 00:00:00", "2000-10-10 00:00:00", 0),
    ] {
        let left = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewTimeDatum(parse_time(left))],
        )
        .unwrap();
        let right = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewTimeDatum(parse_time(right))],
        )
        .unwrap();
        assert_eq!(compare_bytes(&left, &right), expect);
    }
}

#[test]
/// Duration 编解码（含负数）。
fn TestDuration() {
    for value in ["11:11:11", "00:00:00", "1 11:11:11"] {
        let mut expected = parse_duration(value);
        let encoded = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewDurationDatum(expected)],
        )
        .unwrap();
        let decoded = Decode(encoded, 1).unwrap();
        expected.Fsp = types::MaxFsp;
        assert_eq!(decoded[0].GetMysqlDuration().String(), expected.String());
    }
    for (left, right, expect) in [
        ("20:00:00", "11:11:11", 1),
        ("00:00:00", "00:00:01", -1),
        ("00:00:00", "00:00:00", 0),
    ] {
        let left = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewDurationDatum(parse_duration(left))],
        )
        .unwrap();
        let right = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewDurationDatum(parse_duration(right))],
        )
        .unwrap();
        assert_eq!(compare_bytes(&left, &right), expect);
    }
}

/// 生成 Decimal 的可比较 key 编码。
fn decimal_key(decimal: types::MyDecimal) -> Vec<u8> {
    let mut datum = types::NewDecimalDatum(decimal);
    datum.SetLength(30);
    datum.SetFrac(6);
    EncodeKey(time::UTC, Vec::new(), vec![datum]).unwrap()
}

#[test]
/// Decimal 编解码、有序性与尺寸。
fn TestDecimal() {
    for value in [
        "1234.00", "1234", "12.34", "12.340", "0.1234", "0.0", "0", "-0.0", "-0.0000", "-1234.00",
        "-1234", "-12.34", "-12.340", "-0.1234",
    ] {
        let decimal = types::NewDecFromStringForTest(value);
        let encoded = EncodeKey(
            time::UTC,
            Vec::new(),
            vec![types::NewDecimalDatum(decimal.clone())],
        )
        .unwrap();
        let decoded = Decode(encoded, 1).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].GetMysqlDecimal().Compare(&decimal), 0);
    }

    let string_cases = [
        ("1234", "123400", -1),
        ("12340", "123400", -1),
        ("1234", "1234.5", -1),
        ("1234", "1234.0000", 0),
        ("1234", "12.34", 1),
        ("12.34", "12.35", -1),
        ("0.12", "0.1234", -1),
        ("0.1234", "12.3400", -1),
        ("0.1234", "0.1235", -1),
        ("0.123400", "12.34", -1),
        ("12.34000", "12.34", 0),
        ("0.01234", "0.01235", -1),
        ("0.1234", "0", 1),
        ("0.0000", "0", 0),
        ("0.0001", "0", 1),
        ("0.0001", "0.0000", 1),
        ("0", "-0.0000", 0),
        ("-0.0001", "0", -1),
        ("-0.1234", "0", -1),
        ("-0.1234", "-0.12", -1),
        ("-0.12", "-0.1234", 1),
        ("-0.12", "-0.1200", 0),
        ("-0.1234", "0.1234", -1),
        ("-1.234", "-12.34", 1),
        ("-0.1234", "-12.34", 1),
        ("-12.34", "1234", -1),
        ("-12.34", "-12.35", 1),
        ("-0.01234", "-0.01235", 1),
        ("-1234", "-123400", 1),
        ("-12340", "-123400", 1),
    ];
    for (left, right, expect) in string_cases {
        assert_eq!(
            compare_bytes(
                &decimal_key(types::NewDecFromStringForTest(left)),
                &decimal_key(types::NewDecFromStringForTest(right))
            ),
            expect
        );
    }

    let signed_cases = [
        (-1, 1, -1),
        (i64::MAX, i64::MIN, 1),
        (i64::MAX, i32::MAX as i64, 1),
        (i32::MIN as i64, i16::MAX as i64, -1),
        (i64::MIN, i8::MAX as i64, -1),
        (0, i8::MAX as i64, -1),
        (i8::MIN as i64, 0, -1),
        (i16::MIN as i64, i16::MAX as i64, -1),
        (1, -1, 1),
        (1, 0, 1),
        (-1, 0, -1),
        (0, 0, 0),
        (i16::MAX as i64, i16::MAX as i64, 0),
    ];
    for (left, right, expect) in signed_cases {
        assert_eq!(
            compare_bytes(
                &decimal_key(types::NewDecFromInt(left)),
                &decimal_key(types::NewDecFromInt(right))
            ),
            expect
        );
    }

    let unsigned_cases = [
        (0, 0, 0),
        (1, 0, 1),
        (0, 1, -1),
        (i8::MAX as u64, i16::MAX as u64, -1),
        (u32::MAX as u64, i32::MAX as u64, 1),
        (u8::MAX as u64, i8::MAX as u64, 1),
        (u16::MAX as u64, i32::MAX as u64, -1),
        (u64::MAX, i64::MAX as u64, 1),
        (i64::MAX as u64, u32::MAX as u64, 1),
        (u64::MAX, 0, 1),
        (0, u64::MAX, -1),
    ];
    for (left, right, expect) in unsigned_cases {
        assert_eq!(
            compare_bytes(
                &decimal_key(decimal_from_uint(left)),
                &decimal_key(decimal_from_uint(right))
            ),
            expect
        );
    }

    let floats = [
        -123.45,
        -123.40,
        -23.45,
        -1.43,
        -0.93,
        -0.4333,
        -0.068,
        -0.0099,
        0.0,
        0.001,
        0.0012,
        0.12,
        1.2,
        1.23,
        123.3,
        2424.242424,
    ];
    let mut encoded = Vec::new();
    for value in floats {
        let decimal = types::NewDecFromFloatForTest(value);
        let bytes = EncodeDecimal(Vec::new(), &decimal, 20, 6).unwrap();
        assert_eq!(
            bytes.len() + 1,
            EstimateValueSize((*types::DefaultStmtNoWarningContext).clone(), {
                let mut datum = types::NewDecimalDatum(decimal);
                datum.SetLength(20);
                datum.SetFrac(6);
                datum
            })
            .unwrap()
        );
        encoded.push(bytes);
    }
    assert!(encoded.windows(2).all(|pair| pair[0] <= pair[1]));

    let decimal = types::NewDecFromStringForTest("-123.123456789");
    assert!(EncodeDecimal(Vec::new(), &decimal, 20, 5).is_err());
    assert!(EncodeDecimal(Vec::new(), &decimal, 12, 10).is_err());
    let mut datum = types::NewDecimalDatum(decimal);
    datum.SetLength(20);
    datum.SetFrac(5);
    assert!(EncodeValue(time::UTC, Vec::new(), vec![datum.clone()]).is_err());
    datum.SetLength(12);
    datum.SetFrac(10);
    assert!(EncodeValue(time::UTC, Vec::new(), vec![datum]).is_err());
}

#[test]
/// JSON 编解码往返。
fn TestJSON() {
    let original: Vec<_> = ["1234.00", r#"{"a": "b"}"#]
        .into_iter()
        .map(|value| types::NewJSONDatum(types::ParseBinaryJSONFromString(value).unwrap()))
        .collect();
    let encoded = EncodeValue(time::UTC, Vec::with_capacity(4096), original.clone()).unwrap();
    let decoded = Decode(encoded, 2).unwrap();
    for (left, right) in original.iter().zip(decoded) {
        assert_eq!(left.GetMysqlJSON().String(), right.GetMysqlJSON().String());
    }
}

#[test]
/// CutOne 按编码边界切分。
fn TestCut() {
    for case in codec_cases() {
        let mut encoded = EncodeKey(time::UTC, Vec::new(), case.input).unwrap();
        for expected in case.expect {
            let (cut, remain) = CutOne(encoded).unwrap();
            encoded = remain;
            assert_eq!(
                cut,
                EncodeKey(time::UTC, Vec::new(), vec![expected]).unwrap()
            );
        }
        assert!(encoded.is_empty());
    }

    let encoded = EncodeValue(time::UTC, Vec::new(), vec![int(42)]).unwrap();
    let (remain, value) = CutColumnID(encoded).unwrap();
    assert!(remain.is_empty());
    assert_eq!(value, 42);
}

#[test]
/// CutOne 对非法输入报错。
fn TestCutOneError() {
    let error = CutOne(Vec::new()).unwrap_err();
    assert_eq!(error.to_string(), "invalid encoded key");
    let error = CutOne(vec![4, 0, 0, 0]).unwrap_err();
    assert!(error.to_string().starts_with("invalid encoded key"));

    let negative_compact_length = vec![2 /* codec.compactBytesFlag */, 1];
    let (data, remain) = CutOne(negative_compact_length).unwrap();
    assert_eq!(data, vec![2]);
    assert_eq!(remain, vec![1]);
}

#[test]
/// SetRawValues 按列切分 Raw。
fn TestSetRawValues() {
    let datums = vec![int(1), string("abc"), float64(1.1), bytes(b"def")];
    let row_data = EncodeValue(time::UTC, Vec::new(), datums.clone()).unwrap();
    let mut values = vec![types::Datum::default(); datums.len()];
    SetRawValues(row_data, &mut values).unwrap();
    for (raw, datum) in values.iter().zip(datums) {
        assert_eq!(raw.Kind(), types::KindRaw);
        assert_eq!(
            raw.GetBytes(),
            EncodeValue(time::UTC, Vec::new(), vec![datum]).unwrap()
        );
    }
}

/// DecodeOneToChunk / Hash 测试用的 Datum 与字段类型。
fn datums_for_test() -> (Vec<types::Datum>, Vec<types::FieldType>) {
    let mut datums = Vec::new();
    let mut fields = Vec::new();
    let mut push = |datum: types::Datum, field: Box<types::FieldType>| {
        datums.push(datum);
        fields.push(*field);
    };

    for tp in [
        mysql::TypeNull,
        mysql::TypeLonglong,
        mysql::TypeFloat,
        mysql::TypeDate,
        mysql::TypeDuration,
        mysql::TypeNewDecimal,
        mysql::TypeEnum,
        mysql::TypeSet,
        mysql::TypeBit,
        mysql::TypeJSON,
        mysql::TypeVarchar,
        mysql::TypeDouble,
    ] {
        push(types::Datum::default(), types::NewFieldType(tp));
    }
    for tp in [
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeInt24,
        mysql::TypeLong,
    ] {
        push(int(1), types::NewFieldType(tp));
    }
    push(int(-1), types::NewFieldType(mysql::TypeLong));
    push(int(1), types::NewFieldType(mysql::TypeLonglong));
    push(uint(1), types::NewFieldType(mysql::TypeLonglong));
    push(float32(1.0), types::NewFieldType(mysql::TypeFloat));
    push(float64(1.0), types::NewFieldType(mysql::TypeDouble));
    push(
        types::NewDecimalDatum(types::NewDecFromInt(1)),
        types::NewFieldType(mysql::TypeNewDecimal),
    );
    let mut decimal_field = types::NewFieldType(mysql::TypeNewDecimal);
    decimal_field.SetDecimal(2);
    push(
        types::NewDecimalDatum(types::NewDecFromStringForTest("1.123")),
        decimal_field,
    );
    for (value, tp) in [
        ("abc", mysql::TypeString),
        ("def", mysql::TypeVarchar),
        ("ghi", mysql::TypeVarString),
    ] {
        push(string(value), types::NewFieldType(tp));
    }
    for tp in [
        mysql::TypeBlob,
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
    ] {
        push(bytes(b"abc"), types::NewFieldType(tp));
    }
    push(
        types::NewTimeDatum(parse_time("2026-07-14 12:00:00")),
        types::NewFieldType(mysql::TypeDatetime),
    );
    let mut date = parse_time("2026-07-14 00:00:00");
    date.SetType(mysql::TypeDate);
    push(
        types::NewTimeDatum(date),
        types::NewFieldType(mysql::TypeDate),
    );
    let mut timestamp = parse_time("2026-07-14 12:00:00");
    timestamp.SetType(mysql::TypeTimestamp);
    push(
        types::NewTimeDatum(timestamp),
        types::NewFieldType(mysql::TypeTimestamp),
    );
    push(
        types::NewDurationDatum(types::Duration {
            Duration: 1_000_000_000,
            Fsp: 1,
        }),
        types::NewFieldType(mysql::TypeDuration),
    );
    let mut enum_field = types::NewFieldType(mysql::TypeEnum);
    enum_field.SetElems(vec!["a".to_owned()]);
    push(enum_datum("a", 1), enum_field);
    let mut set_field = types::NewFieldType(mysql::TypeSet);
    set_field.SetElems(vec!["a".to_owned()]);
    push(set_datum("a", 1), set_field);
    let mut set_field = types::NewFieldType(mysql::TypeSet);
    set_field.SetElems(
        ["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    push(set_datum("f", 32), set_field);
    let mut bit_field = types::NewFieldType(mysql::TypeBit);
    bit_field.SetFlen(8);
    push(
        types::NewMysqlBitDatum(types::BinaryLiteral(vec![100])),
        bit_field,
    );
    push(
        types::NewJSONDatum(types::CreateBinaryJSON("abc")),
        types::NewFieldType(mysql::TypeJSON),
    );
    push(int(1), types::NewFieldType(mysql::TypeYear));
    (datums, fields)
}

/// 由 Datum 列表构造测试 Chunk。
fn chunk_for_test(
    timezone: time::Location,
    datums: &[types::Datum],
    fields: &mut [types::FieldType],
    row_count: usize,
) -> Box<chunk::Chunk> {
    let mut chunk = chunk::New(fields.to_vec(), 32, 32);
    {
        let mut decoder = NewDecoder(&mut *chunk, timezone);
        for _ in 0..row_count {
            let mut encoded = EncodeValue(timezone, Vec::new(), datums.to_vec()).unwrap();
            for (column, field) in fields.iter_mut().enumerate() {
                encoded = decoder.DecodeOne(encoded, column, field).unwrap();
            }
            assert!(encoded.is_empty());
        }
    }
    chunk
}

#[test]
/// Decoder::DecodeOne 写入 Chunk 与 Datum 解码一致。
fn TestDecodeOneToChunk() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let (datums, mut fields) = datums_for_test();
    let chunk = chunk_for_test(time::UTC, &datums, &mut fields, 3);
    for (column, field) in fields.iter().enumerate() {
        for row in 0..3 {
            let actual = chunk.GetRow(row).GetDatum(column, field);
            let expected = &datums[column];
            if actual.IsNull() {
                assert!(expected.IsNull());
            } else if actual.Kind() == types::KindMysqlDecimal {
                assert_eq!(actual.GetString(), expected.GetString());
            } else {
                assert_eq!(
                    actual
                        .Compare(
                            type_ctx.clone(),
                            expected,
                            collate::GetCollator(field.GetCollate()).as_ref()
                        )
                        .unwrap(),
                    0
                );
            }
        }
    }
}

#[test]
/// HashGroupKey 同值同 key。
fn TestHashGroup() {
    let mut field = types::NewFieldType(mysql::TypeNewDecimal);
    let mut chunk = chunk::New(vec![(*field).clone()], 3, 3);
    let decimal = types::NewDecFromStringForTest("-123.123456789");
    for _ in 0..3 {
        chunk.AppendMyDecimal(0, &decimal);
    }
    field.SetFlen(20);
    field.SetDecimal(5);
    assert!(
        HashGroupKey(
            time::UTC,
            3,
            &mut chunk.columns[0],
            vec![Vec::new(); 3],
            &mut *field
        )
        .is_err()
    );
    field.SetFlen(12);
    field.SetDecimal(10);
    assert!(
        HashGroupKey(
            time::UTC,
            3,
            &mut chunk.columns[0],
            vec![Vec::new(); 3],
            &mut *field
        )
        .is_err()
    );
}

#[test]
/// SerializeKeys 的预分配只设置容量，不得把零填充算进实际 key 长度。
fn TestSerializeKeysUsesPreallocatedCapacityWithoutZeroPrefix() {
    let mut field = types::NewFieldType(mysql::TypeLonglong);
    let mut chunk = chunk::New(vec![(*field).clone()], 2, 2);
    chunk.AppendInt64(0, 42);
    chunk.AppendInt64(0, -7);
    chunk.AppendNull(0);

    let expected = vec![
        chunk.Column(0).GetRaw(0).to_vec(),
        chunk.Column(0).GetRaw(1).to_vec(),
        Vec::new(),
    ];
    let mut null_vector = vec![false; 3];
    let mut serialized_keys = vec![Vec::new(), Vec::new(), Vec::new()];
    let mut serialized_key_lens = vec![0, 0, 0];
    let buffer = SerializeKeys(
        (*types::DefaultStmtNoWarningContext).clone(),
        &mut *chunk,
        vec![&mut *field],
        vec![0],
        vec![0, 1, 2],
        None,
        &mut null_vector,
        vec![SerializeMode::Normal],
        &mut serialized_keys,
        &mut serialized_key_lens,
        Vec::new(),
    )
    .unwrap();

    assert_eq!(null_vector, vec![false, false, true]);
    assert_eq!(serialized_key_lens, vec![8, 8, 0]);
    assert_eq!(buffer.len(), 16);
    assert_eq!(serialized_keys, expected);
}

#[test]
/// DecodeRange 处理末尾边界 flag。
fn TestDecodeRange() {
    assert!(DecodeRange(Vec::new(), 0, None, time::UTC).is_err());
    let datums = vec![int(1), string("abc"), float64(1.1), bytes(b"def")];
    let row_data = EncodeValue(time::UTC, Vec::new(), datums.clone()).unwrap();
    let (decoded, remain) = DecodeRange(row_data.clone(), datums.len(), None, time::UTC).unwrap();
    assert!(remain.is_empty());
    assert_datums_equal(&datums, &decoded);
    for flag in [NilFlag, 1, 250, 251] {
        let mut data = row_data.clone();
        data.push(flag);
        assert!(DecodeRange(data, datums.len() + 1, None, time::UTC).is_ok());
    }
}

/// 辅助：比较 HashChunkRow / EqualChunkRow 结果。
fn hash_chunk_row_equal(
    left: types::Datum,
    mut left_field: types::FieldType,
    right: types::Datum,
    mut right_field: types::FieldType,
    equal: bool,
) {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let mut left_chunk = chunk::New(vec![left_field.clone()], 1, 1);
    let mut right_chunk = chunk::New(vec![right_field.clone()], 1, 1);
    left_chunk.AppendDatum(0, &left);
    right_chunk.AppendDatum(0, &right);
    let mut left_bytes = Vec::new();
    let mut right_bytes = Vec::new();
    HashChunkRow(
        type_ctx.clone(),
        &mut left_bytes,
        left_chunk.GetRow(0),
        vec![&mut left_field],
        vec![0],
        vec![0],
    )
    .unwrap();
    HashChunkRow(
        type_ctx.clone(),
        &mut right_bytes,
        right_chunk.GetRow(0),
        vec![&mut right_field],
        vec![0],
        vec![0],
    )
    .unwrap();
    assert_eq!(
        left_bytes == right_bytes,
        equal,
        "left kind/type={}/{}, right kind/type={}/{}, left={left_bytes:?}, right={right_bytes:?}",
        left.Kind(),
        left_field.GetType(),
        right.Kind(),
        right_field.GetType(),
    );
    assert_eq!(
        EqualChunkRow(
            type_ctx,
            left_chunk.GetRow(0),
            vec![&mut left_field],
            vec![0],
            right_chunk.GetRow(0),
            vec![&mut right_field],
            vec![0]
        )
        .unwrap(),
        equal
    );
}

#[test]
/// HashChunkRow 与 EqualChunkRow 一致性。
fn TestHashChunkRow() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let (datums, mut fields) = datums_for_test();
    let chunk = chunk_for_test(time::UTC, &datums, &mut fields, 1);
    let indexes: Vec<_> = (0..fields.len()).collect();
    let pointers: Vec<_> = fields.iter_mut().map(|field| field as *mut _).collect();
    let mut first = Vec::new();
    let mut second = Vec::new();
    HashChunkRow(
        type_ctx.clone(),
        &mut first,
        chunk.GetRow(0),
        pointers.clone(),
        indexes.clone(),
        vec![0],
    )
    .unwrap();
    HashChunkRow(
        type_ctx.clone(),
        &mut second,
        chunk.GetRow(0),
        pointers.clone(),
        indexes.clone(),
        vec![0],
    )
    .unwrap();
    assert_eq!(first, second);
    assert!(
        EqualChunkRow(
            type_ctx,
            chunk.GetRow(0),
            pointers.clone(),
            indexes.clone(),
            chunk.GetRow(0),
            pointers,
            indexes
        )
        .unwrap()
    );

    hash_chunk_row_equal(
        types::Datum::default(),
        *types::NewFieldType(mysql::TypeNull),
        types::Datum::default(),
        *types::NewFieldType(mysql::TypeNull),
        true,
    );
    hash_chunk_row_equal(
        uint(1),
        unsigned_field(mysql::TypeLonglong),
        int(1),
        *types::NewFieldType(mysql::TypeLonglong),
        true,
    );
    hash_chunk_row_equal(
        uint(u64::MAX),
        unsigned_field(mysql::TypeLonglong),
        int(-1),
        *types::NewFieldType(mysql::TypeLonglong),
        false,
    );
    hash_chunk_row_equal(
        types::NewDecimalDatum(types::NewDecFromStringForTest("1.1")),
        *types::NewFieldType(mysql::TypeNewDecimal),
        types::NewDecimalDatum(types::NewDecFromStringForTest("01.100")),
        *types::NewFieldType(mysql::TypeNewDecimal),
        true,
    );
    hash_chunk_row_equal(
        types::NewDecimalDatum(types::NewDecFromStringForTest("1.1")),
        *types::NewFieldType(mysql::TypeNewDecimal),
        types::NewDecimalDatum(types::NewDecFromStringForTest("01.200")),
        *types::NewFieldType(mysql::TypeNewDecimal),
        false,
    );
    hash_chunk_row_equal(
        float32(1.0),
        *types::NewFieldType(mysql::TypeFloat),
        float64(1.0),
        *types::NewFieldType(mysql::TypeDouble),
        true,
    );
    hash_chunk_row_equal(
        float32(1.0),
        *types::NewFieldType(mysql::TypeFloat),
        float64(1.1),
        *types::NewFieldType(mysql::TypeDouble),
        false,
    );
    hash_chunk_row_equal(
        string("x"),
        *types::NewFieldType(mysql::TypeString),
        bytes(b"x"),
        *types::NewFieldType(mysql::TypeBlob),
        true,
    );
    hash_chunk_row_equal(
        string("x"),
        *types::NewFieldType(mysql::TypeString),
        bytes(b"y"),
        *types::NewFieldType(mysql::TypeBlob),
        false,
    );
    hash_chunk_row_equal(
        types::NewJSONDatum(types::CreateBinaryJSON(1_i64)),
        *types::NewFieldType(mysql::TypeJSON),
        types::NewJSONDatum(types::CreateBinaryJSON(1.0_f64)),
        *types::NewFieldType(mysql::TypeJSON),
        true,
    );
    hash_chunk_row_equal(
        types::NewJSONDatum(types::CreateBinaryJSON(u64::MAX)),
        *types::NewFieldType(mysql::TypeJSON),
        types::NewJSONDatum(types::CreateBinaryJSON(u64::MAX as f64)),
        *types::NewFieldType(mysql::TypeJSON),
        false,
    );
    hash_chunk_row_equal(
        types::NewJSONDatum(types::CreateBinaryJSON(i64::MIN)),
        *types::NewFieldType(mysql::TypeJSON),
        types::NewJSONDatum(types::CreateBinaryJSON(i64::MIN as f64)),
        *types::NewFieldType(mysql::TypeJSON),
        true,
    );
}

#[test]
/// 有符号整数 EstimateValueSize 边界。
fn TestValueSizeOfSignedInt() {
    for boundary in [
        64_i64,
        8192,
        1048576,
        134217728,
        17179869184,
        2199023255552,
        281474976710656,
        36028797018963968,
        4611686018427387904,
    ] {
        for value in [
            boundary - 10,
            boundary,
            boundary + 10,
            -boundary,
            -boundary + 10,
            -boundary - 10,
        ] {
            let datum = int(value);
            let encoded = EncodeValue(time::UTC, Vec::new(), vec![datum.clone()]).unwrap();
            assert_eq!(
                encoded.len(),
                EstimateValueSize((*types::DefaultStmtNoWarningContext).clone(), datum).unwrap()
            );
        }
    }
}

#[test]
/// 无符号整数 EstimateValueSize 边界。
fn TestValueSizeOfUnsignedInt() {
    for boundary in [
        128_u64,
        16384,
        2097152,
        268435456,
        34359738368,
        4398046511104,
        562949953421312,
        72057594037927936,
        9223372036854775808,
    ] {
        for value in [boundary - 10, boundary, boundary + 10] {
            let datum = uint(value);
            let encoded = EncodeValue(time::UTC, Vec::new(), vec![datum.clone()]).unwrap();
            assert_eq!(
                encoded.len(),
                EstimateValueSize((*types::DefaultStmtNoWarningContext).clone(), datum).unwrap()
            );
        }
    }
}

#[test]
/// HashChunkColumns 与逐行哈希一致。
fn TestHashChunkColumns() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let (datums, mut fields) = datums_for_test();
    let mut chunk = chunk_for_test(time::UTC, &datums, &mut fields, 4);
    for column in 0..fields.len() {
        assert_eq!(chunk.GetRow(0).IsNull(column), column < 12);
        let mut vector_hashes: Vec<Box<dyn StdHasher>> = (0..3)
            .map(|_| Box::new(fnv::FnvHasher::default()) as Box<dyn StdHasher>)
            .collect();
        let mut is_null = vec![false; 4];
        HashChunkSelected(
            type_ctx.clone(),
            &mut vector_hashes,
            &mut *chunk,
            &mut fields[column],
            column,
            vec![0],
            &mut is_null,
            Some(vec![true, true, true, false]),
            false,
        )
        .unwrap();
        assert_eq!(is_null[..3], [column < 12; 3]);
        for row in 0..3 {
            let mut encoded = Vec::new();
            HashChunkRow(
                type_ctx.clone(),
                &mut encoded,
                chunk.GetRow(row),
                vec![&mut fields[column]],
                vec![column],
                vec![0],
            )
            .unwrap();
            let mut row_hash = fnv::FnvHasher::default();
            row_hash.write(&encoded);
            assert_eq!(row_hash.finish(), vector_hashes[row].finish());
        }
    }
}

#[test]
/// Datum Hash64 / Equals 语义。
fn TestDatumHashEquals() {
    // Go package init installs codec.Hash64 into types.Hash64ForDatum.
    init();
    let first_time = parse_time("2026-07-14 12:00:00");
    let second_time = parse_time("2026-07-14 12:00:01");
    let equal_cases = vec![
        (int(1), int(1)),
        (uint(1), uint(1)),
        (float64(1.1), float64(1.1)),
        (string("abc"), string("abc")),
        (bytes(b"abc"), bytes(b"abc")),
        (enum_datum("a", 1), enum_datum("a", 1)),
        (set_datum("a", 1), set_datum("a", 1)),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![1])),
            types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![1])),
        ),
        (
            types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(1, -1)),
            types::NewMysqlBitDatum(types::NewBinaryLiteralFromUint(1, -1)),
        ),
        (
            types::NewTimeDatum(first_time),
            types::NewTimeDatum(first_time),
        ),
        (
            types::NewDurationDatum(types::Duration {
                Duration: 1_000_000_000,
                Fsp: 0,
            }),
            types::NewDurationDatum(types::Duration {
                Duration: 1_000_000_000,
                Fsp: 0,
            }),
        ),
        (
            types::NewJSONDatum(types::CreateBinaryJSON("a")),
            types::NewJSONDatum(types::CreateBinaryJSON("a")),
        ),
    ];
    for (left, right) in equal_cases {
        let mut left_hasher = base::NewHashEqualer();
        let mut right_hasher = base::NewHashEqualer();
        left.Hash64(&mut *left_hasher);
        right.Hash64(&mut *right_hasher);
        assert_eq!(left_hasher.Sum64(), right_hasher.Sum64());
        assert!(left.Equals(&right as &dyn Any));
    }

    let left = types::NewTimeDatum(first_time);
    let right = types::NewTimeDatum(second_time);
    let mut left_hasher = base::NewHashEqualer();
    let mut right_hasher = base::NewHashEqualer();
    left.Hash64(&mut *left_hasher);
    right.Hash64(&mut *right_hasher);
    assert_ne!(left_hasher.Sum64(), right_hasher.Sum64());
    assert!(!left.Equals(&right as &dyn Any));
}
