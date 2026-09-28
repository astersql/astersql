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

// AsterSQL 迁移用单元测试：对照 Go 侧行为校验 textrow 编码路径。
//
// 覆盖 ResultEncoder 字符集选择与 Clean、字符串列类型判定、
// FormatValueText 标量/时态/命名类型路径，以及 AppendFormatFloat 表驱动用例。

use super::{
    AppendFormatFloat, ColumnInfo, ErrInvalidType, FormatValueText, IsStringColumnType,
    NewResultEncoder, ResultEncoder, charset, chunk, mysql, types,
};

/// 将单个 Datum 包成一列 Row 后调用 FormatValueText，便于断言输出字节。
fn format_value(col: ColumnInfo, encoder: &mut ResultEncoder, datum: types::Datum) -> Vec<u8> {
    let row = chunk::MutRowFromDatums(vec![datum]);
    FormatValueText(&row.ToRow(), 0, &col, encoder).expect("supported type must format")
}

/// 校验 utf-8/gbk/binary 下 EncodeMeta、EncodeData、ColumnCharsetID 与 Clean 后行为。
#[test]
fn result_encoder_matches_go_charset_selection_and_cleanup() {
    let mut encoder = NewResultEncoder("utf-8");
    assert_eq!(encoder.EncodeMeta(b"test_string"), b"test_string");

    let mut encoder = NewResultEncoder("gbk");
    assert_eq!(encoder.EncodeMeta("一".as_bytes()), vec![0xd2, 0xbb]);
    // 更新为默认 collation 后，列数据应按结果字符集（gbk）编码。
    encoder.UpdateDataEncoding(mysql::DefaultCollationID);
    assert_eq!(encoder.EncodeData("一".as_bytes()), vec![0xd2, 0xbb]);

    // binary collation 时不再做字符集转换，保留原始 UTF-8 字节。
    encoder.UpdateDataEncoding(u16::from(mysql::BinaryDefaultCollationID));
    assert_eq!(encoder.EncodeData("一".as_bytes()), "一".as_bytes());
    assert_eq!(
        encoder.ColumnCharsetID(mysql::DefaultCollationID, true),
        u16::from(mysql::CharsetNameToID("gbk"))
    );
    assert_eq!(
        encoder.ColumnCharsetID(u16::from(mysql::BinaryDefaultCollationID), true),
        u16::from(mysql::BinaryDefaultCollationID)
    );
    // 非字符串列不改写元数据中的 charset。
    assert_eq!(
        encoder.ColumnCharsetID(mysql::DefaultCollationID, false),
        mysql::DefaultCollationID
    );

    // Clean 释放缓冲后，EncodeMeta 仍应按会话结果字符集工作。
    encoder.Clean();
    assert_eq!(encoder.EncodeMeta("一".as_bytes()), vec![0xd2, 0xbb]);
}

/// 校验 IsStringColumnType 与 Go 一致：Blob/Enum/JSON/向量等为真，Longlong 为假。
#[test]
fn string_column_classification_matches_go() {
    for column_type in [
        mysql::TypeString,
        mysql::TypeVarString,
        mysql::TypeVarchar,
        mysql::TypeBit,
        mysql::TypeTinyBlob,
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        mysql::TypeBlob,
        mysql::TypeEnum,
        mysql::TypeSet,
        mysql::TypeJSON,
        mysql::TypeTiDBVectorFloat32,
    ] {
        assert!(IsStringColumnType(column_type), "type {column_type}");
    }
    assert!(!IsStringColumnType(mysql::TypeLonglong));
}

/// 校验整数、Year、浮点精度、Blob/Varchar 以及 GBK/binary 编码路径。
#[test]
fn format_value_text_matches_go_scalar_and_charset_paths() {
    let mut utf8 = NewResultEncoder(charset::CharsetUTF8MB4);

    // Tiny/Short/Int24/Long 统一走有符号十进制追加。
    for column_type in [
        mysql::TypeTiny,
        mysql::TypeShort,
        mysql::TypeInt24,
        mysql::TypeLong,
    ] {
        assert_eq!(
            format_value(
                ColumnInfo {
                    Type: column_type,
                    ..Default::default()
                },
                &mut utf8,
                types::NewIntDatum(-12)
            ),
            b"-12"
        );
    }
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeLonglong,
                Decimal: mysql::NotFixedDec as u8,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(10),
        ),
        b"10"
    );
    // UnsignedFlag 时 Longlong 走无符号路径。
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeLonglong,
                Flag: mysql::UnsignedFlag as u16,
                ..Default::default()
            },
            &mut utf8,
            types::NewUintDatum(11),
        ),
        b"11"
    );
    // Year 零值固定输出四位 "0000"。
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeYear,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(0)
        ),
        b"0000"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeYear,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(1984)
        ),
        b"1984"
    );

    // Decimal 精度：Table 为空时覆盖默认精度；非空时保留完整 strconv 输出。
    let float = types::NewFloat32Datum(1.2);
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeFloat,
                Decimal: 1,
                ..Default::default()
            },
            &mut utf8,
            float.clone()
        ),
        b"1.2"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeFloat,
                Decimal: 2,
                ..Default::default()
            },
            &mut utf8,
            float
        ),
        b"1.20"
    );
    let double = types::NewFloat64Datum(2.2);
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeDouble,
                Decimal: 2,
                ..Default::default()
            },
            &mut utf8,
            double.clone()
        ),
        b"2.20"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Table: "t".to_owned(),
                Type: mysql::TypeDouble,
                Decimal: 2,
                ..Default::default()
            },
            &mut utf8,
            double,
        ),
        b"2.2"
    );

    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeBlob,
                ..Default::default()
            },
            &mut utf8,
            types::NewBytesDatum(b"foo".to_vec()),
        ),
        b"foo"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeVarchar,
                ..Default::default()
            },
            &mut utf8,
            types::NewStringDatum("bar".to_owned()),
        ),
        b"bar"
    );

    // GBK 结果字符集下 Varchar 转码；Bit + binary collation 原样透传。
    let mut gbk = NewResultEncoder("gbk");
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeVarchar,
                Charset: mysql::DefaultCollationID,
                ..Default::default()
            },
            &mut gbk,
            types::NewStringDatum("一".to_owned()),
        ),
        vec![0xd2, 0xbb]
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeBit,
                Charset: u16::from(mysql::BinaryDefaultCollationID),
                ..Default::default()
            },
            &mut gbk,
            types::NewBytesDatum(vec![0xff, 0x00]),
        ),
        vec![0xff, 0x00]
    );
}

/// 校验 Datetime/Duration/Decimal/Enum/Set/JSON/向量等命名类型文本输出。
#[test]
fn format_value_text_matches_go_temporal_decimal_and_named_paths() {
    let mut encoder = NewResultEncoder(charset::CharsetUTF8MB4);
    // 与 Go 测试一致：洛杉矶时区 + 忽略日期中的零位。
    let context = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::America::Los_Angeles,
    };

    // 亚秒四舍五入到秒精度后日期进位。
    let time = types::ParseTime(
        &context,
        "2017-01-05 23:59:59.575601",
        mysql::TypeDatetime,
        0,
    )
    .expect("datetime parses");
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeDatetime,
                ..Default::default()
            },
            &mut encoder,
            types::NewTimeDatum(time)
        ),
        b"2017-01-06 00:00:00"
    );
    let (duration, _) = types::ParseDuration(&context, "11:30:45", 0).expect("duration parses");
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeDuration,
                Decimal: 0,
                ..Default::default()
            },
            &mut encoder,
            types::NewDurationDatum(duration),
        ),
        b"11:30:45"
    );

    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"1.23").expect("decimal parses");
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeNewDecimal,
                ..Default::default()
            },
            &mut encoder,
            types::NewDecimalDatum(decimal),
        ),
        b"1.23"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeEnum,
                Charset: mysql::DefaultCollationID,
                ..Default::default()
            },
            &mut encoder,
            types::NewMysqlEnumDatum(types::Enum {
                Name: "ename".to_owned(),
                Value: 0
            }),
        ),
        b"ename"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeSet,
                Charset: mysql::DefaultCollationID,
                ..Default::default()
            },
            &mut encoder,
            types::NewMysqlSetDatum(
                types::Set {
                    Name: "sname".to_owned(),
                    Value: 0
                },
                mysql::DefaultCollationName.to_owned(),
            ),
        ),
        b"sname"
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeJSON,
                ..Default::default()
            },
            &mut encoder,
            types::NewJSONDatum(
                types::ParseBinaryJSONFromString(r#"{"a": 1, "b": 2}"#).expect("json parses")
            ),
        ),
        br#"{"a": 1, "b": 2}"#
    );
    assert_eq!(
        format_value(
            ColumnInfo {
                Type: mysql::TypeTiDBVectorFloat32,
                ..Default::default()
            },
            &mut encoder,
            types::NewVectorFloat32Datum(
                types::ParseVectorFloat32("[1,2.5]").expect("vector parses")
            ),
        ),
        b"[1,2.5]"
    );
}

/// Geometry 等不受支持的列类型应返回 ErrInvalidType 哨兵错误。
#[test]
fn invalid_column_type_returns_go_sentinel() {
    let mut encoder = NewResultEncoder(charset::CharsetUTF8MB4);
    let row = chunk::MutRowFromDatums(vec![types::NewIntDatum(1)]);
    let error = FormatValueText(
        &row.ToRow(),
        0,
        &ColumnInfo {
            Type: mysql::TypeGeometry,
            ..Default::default()
        },
        &mut encoder,
    )
    .expect_err("geometry is unsupported");
    assert_eq!(error, ErrInvalidType);
    assert_eq!(
        error.to_string(),
        "invalid column type for text serialization"
    );
}

/// 表驱动校验 AppendFormatFloat：科学计数法阈值、精度、32/64 位与无穷值。
#[test]
fn append_format_float_matches_go_table() {
    let cases = [
        (99999999999999999999.0, "1e20", -1, 64),
        (1e15, "1e15", -1, 64),
        (9e14, "900000000000000", -1, 64),
        (-9999999999999999.0, "-1e16", -1, 64),
        (999999999999999.0, "999999999999999", -1, 64),
        (0.000000000000001, "0.000000000000001", -1, 64),
        (0.0000000000000009, "9e-16", -1, 64),
        (-0.0000000000000009, "-9e-16", -1, 64),
        (0.11111, "0.111", 3, 64),
        (0.1111111111111111111, "0.11111111", -1, 32),
        (0.1111111111111111111, "0.1111111111111111", -1, 64),
        (0.0000000000000009, "9e-16", 3, 64),
        (0.0, "0", -1, 64),
        (
            -340282346638528860000000000000000000000.0,
            "-3.40282e38",
            -1,
            32,
        ),
        (-34028236.0, "-34028236.00", 2, 32),
        (-17976921.34, "-17976921.34", 2, 64),
        (-3.402823466e38, "-3.40282e38", -1, 32),
        (-1.7976931348623157e308, "-1.7976931348623157e308", -1, 64),
        (10.0e20, "1e21", -1, 32),
        (1e20, "1e20", -1, 32),
        (10.0, "10", -1, 32),
        (999999986991104.0, "1e15", -1, 32),
        (1e15, "1e15", -1, 32),
        (f64::INFINITY, "0", -1, 64),
        (f64::NEG_INFINITY, "0", -1, 64),
        (1e14, "100000000000000", -1, 64),
        (1e308, "1e308", -1, 64),
    ];

    for (value, expected, precision, bit_size) in cases {
        assert_eq!(
            String::from_utf8(AppendFormatFloat(Vec::new(), value, precision, bit_size)).unwrap(),
            expected
        );
    }
}
