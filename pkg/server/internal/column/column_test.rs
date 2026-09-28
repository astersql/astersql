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

// column 模块的 Go 对照单元测试。
//
// 校验列定义包字节、标志/类型映射、列名截断，以及文本行在常见类型与
// 字符集（含 gbk）下的编码结果。

use super::{DefaultValue, DumpFlag, DumpTextRow, Info};
use super::{charset, chunk, mysql, textrow, types, util};

/// 构造带固定 schema/table/name 的基准列元数据，便于断言包布局。
fn base_info(default_value: Option<DefaultValue>) -> Info {
    Info {
        Schema: "testSchema".into(),
        Table: "testTable".into(),
        OrgTable: "testOrgTable".into(),
        Name: "testName".into(),
        OrgName: "testOrgName".into(),
        ColumnLength: 1,
        Charset: 106,
        Flag: 0,
        Decimal: 1,
        Type: 14,
        DefaultValue: default_value,
    }
}

/// 从 Dump 结果末尾取出协议类型字节，用于核对 dumpType 映射。
fn dumped_type(column_type: u8) -> u8 {
    let dumped = Info {
        Type: column_type,
        ..Default::default()
    }
    .Dump(Vec::new(), None);
    dumped[dumped.len() - 6]
}

/// 对照 Go：无默认值的 ColumnDefinition41 字节序列，以及 SET/ENUM 标志与类型映射。
#[test]
fn test_dump_column() {
    let info = base_info(Some(DefaultValue::String(vec![5, 2])));
    let result = info.Dump(Vec::new(), None);
    let expected = vec![
        0x03, 0x64, 0x65, 0x66, 0x0a, 0x74, 0x65, 0x73, 0x74, 0x53, 0x63, 0x68, 0x65, 0x6d, 0x61,
        0x09, 0x74, 0x65, 0x73, 0x74, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x0c, 0x74, 0x65, 0x73, 0x74,
        0x4f, 0x72, 0x67, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x08, 0x74, 0x65, 0x73, 0x74, 0x4e, 0x61,
        0x6d, 0x65, 0x0b, 0x74, 0x65, 0x73, 0x74, 0x4f, 0x72, 0x67, 0x4e, 0x61, 0x6d, 0x65, 0x0c,
        0x6a, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x01, 0x00, 0x00,
    ];
    assert_eq!(result, expected);

    assert_eq!(DumpFlag(mysql::TypeSet, 0), mysql::SetFlag as u16);
    assert_eq!(DumpFlag(mysql::TypeEnum, 0), mysql::EnumFlag as u16);
    assert_eq!(DumpFlag(mysql::TypeString, 0), 0);
    assert_eq!(dumped_type(mysql::TypeSet), mysql::TypeString);
    assert_eq!(dumped_type(mysql::TypeEnum), mysql::TypeString);
    assert_eq!(dumped_type(mysql::TypeBit), mysql::TypeBit);
}

// Stable Rust has no std testing.B. This keeps the Go benchmark's buffer reuse
// and DumpWithDefault hot path available to a future native benchmark target.
/// 预留基准入口：复用缓冲区反复调用 DumpWithDefault，对齐 Go testing.B 热路径。
pub fn benchmark_dump_column(iterations: usize) -> Vec<u8> {
    let info = base_info(Some(DefaultValue::String(b"test".to_vec())));
    let mut encoder = textrow::NewResultEncoder(charset::CharsetUTF8MB4);
    let mut buffer = Vec::with_capacity(1024);
    for _ in 0..iterations {
        buffer.clear();
        buffer = info.DumpWithDefault(buffer, Some(&mut encoder));
    }
    buffer
}

/// 对照 Go：带默认值字符串时，包尾应为长度编码的默认值字节。
#[test]
fn test_dump_column_with_default() {
    let info = base_info(Some(DefaultValue::String(b"test".to_vec())));
    let result = info.DumpWithDefault(Vec::new(), None);
    let expected = vec![
        0x03, 0x64, 0x65, 0x66, 0x0a, 0x74, 0x65, 0x73, 0x74, 0x53, 0x63, 0x68, 0x65, 0x6d, 0x61,
        0x09, 0x74, 0x65, 0x73, 0x74, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x0c, 0x74, 0x65, 0x73, 0x74,
        0x4f, 0x72, 0x67, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x08, 0x74, 0x65, 0x73, 0x74, 0x4e, 0x61,
        0x6d, 0x65, 0x0b, 0x74, 0x65, 0x73, 0x74, 0x4f, 0x72, 0x67, 0x4e, 0x61, 0x6d, 0x65, 0x0c,
        0x6a, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x01, 0x00, 0x00, 0x04, 0x74, 0x65,
        0x73, 0x74,
    ];
    assert_eq!(result, expected);

    assert_eq!(DumpFlag(mysql::TypeSet, 0), mysql::SetFlag as u16);
    assert_eq!(DumpFlag(mysql::TypeEnum, 0), mysql::EnumFlag as u16);
    assert_eq!(DumpFlag(mysql::TypeString, 0), 0);
    assert_eq!(dumped_type(mysql::TypeSet), mysql::TypeString);
    assert_eq!(dumped_type(mysql::TypeEnum), mysql::TypeString);
    assert_eq!(dumped_type(mysql::TypeBit), mysql::TypeBit);
}

/// 列名超过 256 字节时按协议截断，长度前缀为 0xfc 0001（小端 256）。
#[test]
fn test_column_name_limit() {
    let mut info = base_info(Some(DefaultValue::String(vec![5, 2])));
    info.Name = "a".repeat(300);
    let result = info.Dump(Vec::new(), None);

    let mut expected = vec![
        0x03, 0x64, 0x65, 0x66, 0x0a, 0x74, 0x65, 0x73, 0x74, 0x53, 0x63, 0x68, 0x65, 0x6d, 0x61,
        0x09, 0x74, 0x65, 0x73, 0x74, 0x54, 0x61, 0x62, 0x6c, 0x65, 0x0c, 0x74, 0x65, 0x73, 0x74,
        0x4f, 0x72, 0x67, 0x54, 0x61, 0x62, 0x6c, 0x65, 0xfc, 0x00, 0x01,
    ];
    expected.extend(std::iter::repeat_n(b'a', 256));
    expected.extend_from_slice(&[
        0x0b, 0x74, 0x65, 0x73, 0x74, 0x4f, 0x72, 0x67, 0x4e, 0x61, 0x6d, 0x65, 0x0c, 0x6a, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x01, 0x00, 0x00,
    ]);
    assert_eq!(result, expected);
}

/// 将单个 Datum 包成一行并走 DumpTextRow，便于按类型断言文本编码。
fn dump_text_value(
    columns: &[Info],
    datum: types::Datum,
    encoder: &mut textrow::ResultEncoder,
) -> Vec<u8> {
    let row = chunk::mutrow::MutRowFromDatums(vec![datum]);
    DumpTextRow(Vec::new(), columns, row.ToRow(), Some(encoder))
        .expect("the Go test only supplies supported column types")
}

/// 覆盖文本协议：NULL、整数、浮点精度、blob/varchar、gbk、时间、decimal、year、enum/set、JSON。
#[test]
fn test_dump_text_value() {
    let mut columns = vec![Info {
        Type: mysql::TypeLonglong,
        Decimal: mysql::NotFixedDec as u8,
        ..Default::default()
    }];
    let mut encoder = textrow::NewResultEncoder(charset::CharsetUTF8MB4);

    let mut null = types::NewIntDatum(0);
    null.SetNull();
    let bytes = dump_text_value(&columns, null, &mut encoder);
    let (_, is_null, _, error) = util::ParseLengthEncodedBytes(&bytes);
    assert!(error.is_none());
    assert!(is_null);

    let bytes = dump_text_value(&columns, types::NewIntDatum(10), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "10");

    let bytes = dump_text_value(&columns, types::NewUintDatum(11), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "11");

    columns[0].Flag |= mysql::UnsignedFlag as u16;
    let bytes = dump_text_value(&columns, types::NewUintDatum(11), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "11");

    columns[0].Type = mysql::TypeFloat;
    columns[0].Decimal = 1;
    let float = types::NewFloat32Datum(1.2);
    let bytes = dump_text_value(&columns, float.clone(), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "1.2");

    columns[0].Decimal = 2;
    let bytes = dump_text_value(&columns, float, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "1.20");

    let double = types::NewFloat64Datum(2.2);
    columns[0].Type = mysql::TypeDouble;
    columns[0].Decimal = 1;
    let bytes = dump_text_value(&columns, double.clone(), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "2.2");

    columns[0].Decimal = 2;
    let bytes = dump_text_value(&columns, double, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "2.20");

    columns[0].Type = mysql::TypeBlob;
    let bytes = dump_text_value(
        &columns,
        types::NewBytesDatum(b"foo".to_vec()),
        &mut encoder,
    );
    assert_eq!(must_decode_str(&bytes), "foo");

    columns[0].Type = mysql::TypeVarchar;
    let bytes = dump_text_value(
        &columns,
        types::NewStringDatum("bar".to_owned()),
        &mut encoder,
    );
    assert_eq!(must_decode_str(&bytes), "bar");

    encoder = textrow::NewResultEncoder("gbk");
    columns[0].Type = mysql::TypeVarchar;
    let chinese = types::NewStringDatum("一".to_owned());
    let bytes = dump_text_value(&columns, chinese.clone(), &mut encoder);
    assert_eq!(must_decode_bytes(&bytes), vec![0xd2, 0xbb]);

    columns[0].Charset = mysql::CharsetNameToID("gbk") as u16;
    encoder = textrow::NewResultEncoder("binary");
    let bytes = dump_text_value(&columns, chinese, &mut encoder);
    assert_eq!(must_decode_bytes(&bytes), vec![0xd2, 0xbb]);

    let context = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::America::Los_Angeles,
    };
    let mysql_time = types::ParseTime(
        &context,
        "2017-01-05 23:59:59.575601",
        mysql::TypeDatetime,
        0,
    )
    .expect("datetime parses");
    let mut datum = types::Datum::default();
    datum.SetMysqlTime(mysql_time);
    columns[0].Type = mysql::TypeDatetime;
    let bytes = dump_text_value(&columns, datum.clone(), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "2017-01-06 00:00:00");

    let (duration, _) = types::ParseDuration(&context, "11:30:45", 0).expect("duration parses");
    datum.SetMysqlDuration(duration);
    columns[0].Type = mysql::TypeDuration;
    columns[0].Decimal = 0;
    let bytes = dump_text_value(&columns, datum.clone(), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "11:30:45");

    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"1.23").expect("decimal parses");
    datum.SetMysqlDecimal(decimal);
    columns[0].Type = mysql::TypeNewDecimal;
    let bytes = dump_text_value(&columns, datum, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "1.23");

    let mut year = types::NewIntDatum(0);
    columns[0].Type = mysql::TypeYear;
    let bytes = dump_text_value(&columns, year.clone(), &mut encoder);
    assert_eq!(must_decode_str(&bytes), "0000");

    year.SetInt64(1984);
    let bytes = dump_text_value(&columns, year, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "1984");

    let enum_datum = types::NewMysqlEnumDatum(types::Enum {
        Name: "ename".into(),
        Value: 0,
    });
    columns[0].Type = mysql::TypeEnum;
    let bytes = dump_text_value(&columns, enum_datum, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "ename");

    let set_datum = types::NewMysqlSetDatum(
        types::Set {
            Name: "sname".into(),
            Value: 0,
        },
        mysql::DefaultCollationName.to_owned(),
    );
    columns[0].Type = mysql::TypeSet;
    let bytes = dump_text_value(&columns, set_datum, &mut encoder);
    assert_eq!(must_decode_str(&bytes), "sname");

    let json = types::ParseBinaryJSONFromString(r#"{"a": 1, "b": 2}"#).expect("binary JSON parses");
    columns[0].Type = mysql::TypeJSON;
    let bytes = dump_text_value(&columns, types::NewJSONDatum(json), &mut encoder);
    assert_eq!(must_decode_str(&bytes), r#"{"a": 1, "b": 2}"#);
}

/// 解析长度编码字节并断言非 NULL，失败则测试中止。
fn must_decode_bytes(input: &[u8]) -> Vec<u8> {
    let (bytes, is_null, _, error) = util::ParseLengthEncodedBytes(input);
    assert!(error.is_none(), "length-encoded bytes must parse");
    assert!(!is_null, "test value must not be NULL");
    bytes.expect("non-NULL value has bytes").to_vec()
}

/// 在 must_decode_bytes 之上按 UTF-8 还原字符串。
fn must_decode_str(input: &[u8]) -> String {
    String::from_utf8(must_decode_bytes(input)).expect("test value must be UTF-8")
}
