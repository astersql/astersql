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

// column 包迁移对照单测（Aster 补充）。
//
// 相对 Go 协议边界做更细的回归：列定义默认值分支、向量类型元数据、
// 文本/二进制行编码，以及 ConvertColumnInfo 的长度/精度/别名规则。

use std::rc::Rc;

use super::{ConvertColumnInfo, DefaultValue, DumpBinaryRow, DumpFlag, DumpTextRow, Info};
use super::{charset, chunk, model, mysql, resolve, textrow, types};

/// 按指定协议类型构造基准 `Info`。
fn base_info(column_type: u8) -> Info {
    Info {
        DefaultValue: None,
        Schema: "testSchema".into(),
        Table: "testTable".into(),
        OrgTable: "testOrgTable".into(),
        Name: "testName".into(),
        OrgName: "testOrgName".into(),
        ColumnLength: 1,
        Charset: 106,
        Flag: 0,
        Decimal: 1,
        Type: column_type,
    }
}

/// 列定义包字节与默认值分支（NULL / CURRENT_* → 0xfb，普通字符串长度编码）。
#[test]
fn column_definition_matches_go_protocol_and_default_branches() {
    let info = base_info(mysql::TypeDate);
    assert_eq!(
        info.Dump(Vec::new(), None),
        vec![
            0x03,
            b'd',
            b'e',
            b'f',
            0x0a,
            b't',
            b'e',
            b's',
            b't',
            b'S',
            b'c',
            b'h',
            b'e',
            b'm',
            b'a',
            0x09,
            b't',
            b'e',
            b's',
            b't',
            b'T',
            b'a',
            b'b',
            b'l',
            b'e',
            0x0c,
            b't',
            b'e',
            b's',
            b't',
            b'O',
            b'r',
            b'g',
            b'T',
            b'a',
            b'b',
            b'l',
            b'e',
            0x08,
            b't',
            b'e',
            b's',
            b't',
            b'N',
            b'a',
            b'm',
            b'e',
            0x0b,
            b't',
            b'e',
            b's',
            b't',
            b'O',
            b'r',
            b'g',
            b'N',
            b'a',
            b'm',
            b'e',
            0x0c,
            0x6a,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            mysql::TypeDate,
            0x00,
            0x00,
            0x01,
            0x00,
            0x00,
        ]
    );

    let mut with_default = info.clone();
    with_default.DefaultValue = Some(DefaultValue::String(b"test".to_vec()));
    let dumped = with_default.DumpWithDefault(Vec::new(), None);
    assert_eq!(&dumped[dumped.len() - 5..], b"\x04test");

    for value in [
        None,
        Some(DefaultValue::String(b"CURRENT_TIMESTAMP".to_vec())),
        Some(DefaultValue::String(b"CURRENT_DATE".to_vec())),
    ] {
        with_default.DefaultValue = value;
        assert_eq!(
            with_default.DumpWithDefault(Vec::new(), None).last(),
            Some(&0xfb)
        );
    }
}

/// Go 的字符串默认值按原始字节写出，不能做 UTF-8 lossy 转换；浮点使用 `%v`/`%g`。
#[test]
fn column_defaults_preserve_bytes_and_go_float_format() {
    let mut info = base_info(mysql::TypeString);

    info.DefaultValue = Some(DefaultValue::String(vec![0xff, 0x00]));
    let dumped = info.DumpWithDefault(Vec::new(), None);
    assert_eq!(&dumped[dumped.len() - 3..], &[2, 0xff, 0x00]);

    for (value, expected) in [
        (f64::INFINITY, b"+Inf".as_slice()),
        (f64::NEG_INFINITY, b"-Inf".as_slice()),
        (1_000_000.0, b"1e+06".as_slice()),
        (0.000_01, b"1e-05".as_slice()),
        (0.000_001, b"1e-06".as_slice()),
    ] {
        info.DefaultValue = Some(DefaultValue::Float(value));
        let dumped = info.DumpWithDefault(Vec::new(), None);
        let default_offset = dumped.len() - expected.len() - 1;
        assert_eq!(dumped[default_offset], expected.len() as u8);
        assert_eq!(&dumped[default_offset + 1..], expected);
    }
}

/// 列名截断、向量类型的 charset/length/type/flag 与 Go 兼容行为一致。
#[test]
fn metadata_limits_and_mysql_compatibility_match_go() {
    let mut info = base_info(mysql::TypeTiDBVectorFloat32);
    info.Name = "a".repeat(300);
    info.OrgName = "b".repeat(300);
    info.Flag = mysql::BinaryFlag as u16 | mysql::NotNullFlag as u16;
    let dumped = info.Dump(Vec::new(), None);

    assert!(
        dumped
            .windows(259)
            .any(|window| { window[..3] == [0xfc, 0x00, 0x01] && window[3..] == vec![b'a'; 256] })
    );
    assert_eq!(DumpFlag(mysql::TypeSet, 0), mysql::SetFlag as u16);
    assert_eq!(DumpFlag(mysql::TypeEnum, 0), mysql::EnumFlag as u16);
    assert_eq!(
        DumpFlag(mysql::TypeTiDBVectorFloat32, info.Flag),
        mysql::NotNullFlag as u16
    );
    assert_eq!(info.dumpCharset(), mysql::DefaultCollationID);
    assert_eq!(info.dumpLength(), mysql::MaxLongBlobWidth as u32);
    assert_eq!(
        super::dumpType(mysql::TypeTiDBVectorFloat32),
        mysql::TypeLongBlob
    );
    assert_eq!(super::dumpType(mysql::TypeTinyBlob), mysql::TypeBlob);
}

/// 文本行：NULL(0xfb)、有符号整数、ENUM、JSON 的长度编码结果。
#[test]
fn text_rows_cover_null_numeric_enum_and_json_go_paths() {
    let mut encoder = textrow::NewResultEncoder(charset::CharsetUTF8MB4);

    let mut null = types::NewIntDatum(0);
    null.SetNull();
    let null_row = chunk::mutrow::MutRowFromDatums(vec![null]);
    let row = null_row.ToRow();
    assert_eq!(
        DumpTextRow(
            Vec::new(),
            &[base_info(mysql::TypeLonglong)],
            row,
            Some(&mut encoder)
        )
        .unwrap(),
        vec![0xfb]
    );

    let signed_row = chunk::mutrow::MutRowFromDatums(vec![types::NewIntDatum(-42)]);
    let row = signed_row.ToRow();
    assert_eq!(
        DumpTextRow(
            Vec::new(),
            &[base_info(mysql::TypeLonglong)],
            row,
            Some(&mut encoder)
        )
        .unwrap(),
        b"\x03-42"
    );

    let enum_value = types::NewMysqlEnumDatum(types::Enum {
        Name: "ename".into(),
        Value: 7,
    });
    let enum_row = chunk::mutrow::MutRowFromDatums(vec![enum_value]);
    let row = enum_row.ToRow();
    assert_eq!(
        DumpTextRow(
            Vec::new(),
            &[base_info(mysql::TypeEnum)],
            row,
            Some(&mut encoder)
        )
        .unwrap(),
        b"\x05ename"
    );

    let mut json = types::Datum::default();
    json.SetMysqlJSON(types::ParseBinaryJSONFromString(r#"{"a": 1, "b": 2}"#).unwrap());
    let json_row = chunk::mutrow::MutRowFromDatums(vec![json]);
    let row = json_row.ToRow();
    assert_eq!(
        DumpTextRow(
            Vec::new(),
            &[base_info(mysql::TypeJSON)],
            row,
            Some(&mut encoder)
        )
        .unwrap(),
        b"\x10{\"a\": 1, \"b\": 2}"
    );
}

/// 二进制行：NULL 位图布局、定长/ENUM 载荷，以及对未知类型报错。
#[test]
fn binary_rows_match_go_bitmap_and_type_specific_getters() {
    let mut null = types::NewIntDatum(0);
    null.SetNull();
    let enum_value = types::NewMysqlEnumDatum(types::Enum {
        Name: "ename".into(),
        Value: 7,
    });
    let binary_row = chunk::mutrow::MutRowFromDatums(vec![null, types::NewIntDatum(7), enum_value]);
    let row = binary_row.ToRow();
    let columns = [
        base_info(mysql::TypeLonglong),
        base_info(mysql::TypeTiny),
        base_info(mysql::TypeEnum),
    ];

    assert_eq!(
        DumpBinaryRow(Vec::new(), &columns, row, None).unwrap(),
        vec![
            mysql::OKHeader,
            0b0000_0100,
            7,
            5,
            b'e',
            b'n',
            b'a',
            b'm',
            b'e'
        ]
    );

    let invalid_row = chunk::mutrow::MutRowFromDatums(vec![types::NewIntDatum(1)]);
    let row = invalid_row.ToRow();
    let error = DumpBinaryRow(Vec::new(), &[base_info(u8::MAX)], row, None).unwrap_err();
    assert!(error.to_string().contains("invalid type 255"));
}

/// 组装 ConvertColumnInfo 测试用的 ResultField（含别名与可选表信息）。
fn result_field(
    column: model::ColumnInfo,
    table: Option<model::TableInfo>,
    empty_org_name: bool,
) -> resolve::ResultField {
    resolve::ResultField {
        column: Some(Rc::new(column)),
        column_as_name: resolve::ast::NewCIStr("alias_col"),
        empty_org_name,
        table: table.map(Rc::new),
        table_as_name: resolve::ast::NewCIStr("alias_table"),
        db_name: resolve::ast::NewCIStr("db"),
    }
}

/// ConvertColumnInfo：别名、VARCHAR→VAR_STRING、utf8mb4 长度放大与默认值。
#[test]
fn convert_column_info_matches_go_length_decimal_and_alias_rules() {
    let mut column = model::ColumnInfo::default();
    column.Name = resolve::ast::NewCIStr("org_col");
    column.SetType(mysql::TypeVarchar);
    column.SetFlen(10);
    column.SetDecimal(types::UnspecifiedLength as isize);
    column.SetCharset("utf8mb4".into());
    column
        .SetDefaultValue(Some(model::DefaultValue::String(b"default".to_vec())))
        .unwrap();
    let table = model::TableInfo {
        Name: resolve::ast::NewCIStr("org_table"),
        ..Default::default()
    };

    let converted = ConvertColumnInfo(&result_field(column, Some(table), false));
    assert_eq!(converted.Name, "alias_col");
    assert_eq!(converted.OrgName, "org_col");
    assert_eq!(converted.Table, "alias_table");
    assert_eq!(converted.OrgTable, "org_table");
    assert_eq!(converted.Schema, "db");
    assert_eq!(converted.Type, mysql::TypeVarString);
    assert_eq!(converted.ColumnLength, 40);
    assert_eq!(converted.Decimal, mysql::NotFixedDec as u8);
    assert_eq!(
        converted.DefaultValue,
        Some(DefaultValue::String(b"default".to_vec()))
    );
}

/// ConvertColumnInfo：DECIMAL 符号/小数点加宽、未知字符集 Maxlen=4、DURATION 默认精度。
#[test]
fn convert_column_info_covers_decimal_unknown_charset_and_unspecified_flen() {
    let mut decimal = model::ColumnInfo::default();
    decimal.Name = resolve::ast::NewCIStr("decimal_col");
    decimal.SetType(mysql::TypeNewDecimal);
    decimal.SetFlen(10);
    decimal.SetDecimal(4);
    let converted = ConvertColumnInfo(&result_field(decimal, None, true));
    assert_eq!(converted.ColumnLength, 12);
    assert_eq!(converted.Decimal, 4);
    assert!(converted.OrgName.is_empty());

    let mut unknown_charset = model::ColumnInfo::default();
    unknown_charset.SetType(mysql::TypeString);
    unknown_charset.SetFlen(3);
    unknown_charset.SetCharset("unknown_charset".into());
    assert_eq!(
        ConvertColumnInfo(&result_field(unknown_charset, None, false)).ColumnLength,
        12
    );

    let mut duration = model::ColumnInfo::default();
    duration.SetType(mysql::TypeDuration);
    duration.SetFlen(types::UnspecifiedLength as isize);
    duration.SetDecimal(types::UnspecifiedLength as isize);
    let converted = ConvertColumnInfo(&result_field(duration, None, false));
    assert_eq!(
        converted.ColumnLength,
        mysql::GetDefaultFieldLengthAndDecimal(mysql::TypeDuration).0 as u32
    );
    assert_eq!(converted.Decimal, types::DefaultFsp as u8);
}

/// 长度计算使用 u32 wrapping，与 Go uint32 溢出语义对齐。
#[test]
fn convert_column_length_keeps_go_uint32_wrapping_semantics() {
    let mut decimal = model::ColumnInfo::default();
    decimal.SetType(mysql::TypeNewDecimal);
    decimal.SetFlen(u32::MAX as isize);
    decimal.SetDecimal(1);
    assert_eq!(
        ConvertColumnInfo(&result_field(decimal, None, false)).ColumnLength,
        1
    );

    let mut string = model::ColumnInfo::default();
    string.SetType(mysql::TypeString);
    string.SetFlen(u32::MAX as isize);
    string.SetCharset("utf8mb4".into());
    assert_eq!(
        ConvertColumnInfo(&result_field(string, None, false)).ColumnLength,
        u32::MAX.wrapping_mul(4)
    );
}
