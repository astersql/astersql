// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// parser/types 迁移对照单元测试：校验类型名、FieldType 与 Restore 与 Go 一致。
//
// 覆盖 TypeStr/TypeToStr/StrToType、EvalType 名称、FieldType 串化与字符集判定、
// ENUM 元素/数组/JSON 编解码，以及 Restore / FormatAsCastType 输出与写错误传播。

use parser_types::types::*;
use parser_types::{format, mysql};
use std::io::{self, Write};

/// 对照 Go：类型显示名、EvalType 字符串与错误码映射是否一致。
#[test]
fn type_names_and_eval_types_match_go() {
    // (存储类型, 文本 charset 下名称, binary 下名称, StrToType(binary) 期望类型)
    let cases = [
        (mysql::TypeBlob, "text", "blob", mysql::TypeBlob),
        (
            mysql::TypeLongBlob,
            "longtext",
            "longblob",
            mysql::TypeLongBlob,
        ),
        (mysql::TypeString, "char", "binary", mysql::TypeString),
        (mysql::TypeNull, "null", "binary", mysql::TypeString),
        (
            mysql::TypeTiDBVectorFloat32,
            "vector",
            "vector",
            mysql::TypeTiDBVectorFloat32,
        ),
    ];
    // 对若干类型码核对明文/二进制字符集下的类型名与解析回环。
    for (tp, plain, binary, parsed_binary) in cases {
        assert_eq!(TypeStr(tp), plain);
        assert_eq!(TypeToStr(tp, "utf8mb4"), plain);
        assert_eq!(TypeToStr(tp, "binary"), binary);
        assert_eq!(StrToType(binary), parsed_binary);
    }
    assert_eq!(StrToType("unknown"), mysql::TypeUnspecified);
    assert_eq!(ErrInvalidDefault.Code(), mysql::ErrInvalidDefault as i32);
    assert_eq!(ErrDataOutOfRange.Code(), mysql::ErrDataOutOfRange as i32);
    assert_eq!(
        ErrTruncatedWrongValue.Code(),
        mysql::ErrTruncatedWrongValue as i32
    );
    assert_eq!(
        ErrIllegalValueForType.Code(),
        mysql::ErrIllegalValueForType as i32
    );

    let names = [
        "Int",
        "Real",
        "Decimal",
        "String",
        "Datetime",
        "Timestamp",
        "Time",
        "Json",
        "VectorFloat32",
    ];
    // 按 iota 顺序核对 EvalType 的 String 名称。
    for (value, name) in names.into_iter().enumerate() {
        assert_eq!(EvalType(value as u8).String(), name);
    }
    assert!(ETVectorFloat32.IsStringKind());
    assert!(ETVectorFloat32.IsVectorKind());
    assert!(!ETInt.IsStringKind());
}

/// 校验 FieldType 默认显示宽度、标志位串化、ENUM 转义与 Decimal 校验。
/// 对照 Go：FieldType 默认 flen/decimal、String/InfoSchemaStr、ENUM 转义与 Decimal 校验。
#[test]
fn field_type_core_behavior_matches_go() {
    let mut ft = NewFieldType(mysql::TypeDuration);
    assert_eq!(ft.GetFlen(), UnspecifiedLength);
    assert_eq!(ft.GetDecimal(), UnspecifiedLength);
    ft.SetDecimal(5);
    assert_eq!(ft.String(), "time(5)");
    assert!(!HasCharset(&ft));

    let mut int_ft = NewFieldType(mysql::TypeLong);
    int_ft.SetFlen(5);
    int_ft.SetFlag(mysql::UnsignedFlag | mysql::ZerofillFlag);
    assert_eq!(int_ft.String(), "int(5) UNSIGNED ZEROFILL");
    assert_eq!(int_ft.InfoSchemaStr(), "int(5) unsigned");

    let mut enum_ft = NewFieldType(mysql::TypeEnum);
    enum_ft.SetElems(vec!["a\nb".into(), "a'\t\r\nb".into(), "a\rb".into()]);
    assert_eq!(enum_ft.String(), "enum('a\\nb','a''\t\\r\\nb','a\\rb')");
    assert!(HasCharset(&enum_ft));

    let mut decimal = NewFieldType(mysql::TypeNewDecimal);
    decimal.SetFlen(10);
    decimal.SetDecimal(3);
    assert!(decimal.IsDecimalValid());
    assert_eq!(decimal.StorageLength(), 6);
    decimal.SetDecimal(mysql::MaxDecimalScale as isize + 1);
    assert!(!decimal.IsDecimalValid());
}

/// 校验 ENUM 元素二进制字面量、Array 视图、JSON 往返与 Equal 语义。
#[test]
fn flags_arrays_json_and_equality_match_go() {
    let mut ft = NewFieldType(mysql::TypeEnum);
    ft.SetElems(vec!["a".into(), "b".into()]);
    ft.SetElemWithIsBinaryLit(1, "bb".into(), true);
    assert_eq!(ft.GetElem(1), "bb");
    assert!(!ft.GetElemIsBinaryLit(0));
    assert!(ft.GetElemIsBinaryLit(1));
    ft.CleanElemIsBinaryLit();
    assert!(!ft.GetElemIsBinaryLit(1));

    ft.SetArray(true);
    assert!(ft.IsArray());
    assert_eq!(ft.GetType(), mysql::TypeJSON);
    assert_eq!(ft.ArrayType().GetType(), mysql::TypeEnum);

    let encoded = ft.MarshalJSON().unwrap();
    let mut decoded = NewFieldType(mysql::TypeNull);
    decoded.UnmarshalJSON(&encoded).unwrap();
    assert!(ft.Equals(&decoded));

    let mut float1 = NewFieldType(mysql::TypeDouble);
    let mut float2 = NewFieldType(mysql::TypeDouble);
    float1.SetFlen(22);
    float2.SetFlen(23);
    assert!(float1.Equal(&float2));
    float1.SetDecimal(5);
    float2.SetDecimal(5);
    assert!(!float1.Equal(&float2));
}

#[test]
/// 通用 serde 入口也必须调用 FieldType 的 Go MarshalJSON/UnmarshalJSON 契约。
fn serde_json_uses_exported_go_field_names() {
    let mut ft = NewFieldType(mysql::TypeVarchar);
    ft.SetFlen(12);
    ft.SetCharset("utf8mb4".into());

    let value = serde_json::to_value(&ft).unwrap();
    assert_eq!(value["Tp"], mysql::TypeVarchar);
    assert_eq!(value["Flen"], 12);
    assert_eq!(value["Charset"], "utf8mb4");
    assert!(value.get("tp").is_none());

    let decoded: FieldType = serde_json::from_value(value).unwrap();
    assert!(ft.Equals(&decoded));
}

/// 校验 Restore 与 FormatAsCastType 输出格式与 Go 一致。
#[test]
fn restore_and_cast_format_match_go() {
    let mut ft = NewFieldType(mysql::TypeVarchar);
    ft.SetFlen(12);
    ft.SetCharset("utf8mb4".into());
    ft.SetCollate("utf8mb4_bin".into());
    ft.AddFlag(mysql::BinaryFlag);

    let mut output = Vec::new();
    let mut ctx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut output);
    ft.Restore(&mut ctx).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "VARCHAR(12) BINARY CHARACTER SET UTF8MB4 COLLATE utf8mb4_bin"
    );

    let mut cast_ft = ft.Clone();
    cast_ft.SetType(mysql::TypeVarString);
    let mut cast = Vec::new();
    cast_ft.FormatAsCastType(&mut cast, true).unwrap();
    assert_eq!(String::from_utf8(cast).unwrap(), "CHAR(12) BINARY");
}

/// 始终失败的 Writer，用于验证 Rust `Restore` 不会掩盖底层写错误。
struct AlwaysFail;
impl Write for AlwaysFail {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::Other, "injected"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Rust 的 `RestoreCtx` 提供可失败写接口，因此应传播底层 I/O 错误。
///
/// Go 的 `RestoreCtx` 写方法没有错误返回值，`FieldType.Restore` 也始终返回 nil；
/// 该场景没有可逐项移植的 Go 断言，属于 Rust API 的额外支撑契约。
#[test]
fn restore_propagates_writer_errors_in_rust_api() {
    let ft = NewFieldType(mysql::TypeLong);
    let mut writer = AlwaysFail;
    let mut ctx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut writer);
    assert_eq!(
        ft.Restore(&mut ctx).unwrap_err().kind(),
        io::ErrorKind::Other
    );
}
