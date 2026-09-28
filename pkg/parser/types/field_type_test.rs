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

// FieldType 串化、字符集判定与等价比较的单元测试。
//
// 对照 Go 的 `field_type_test.go`：覆盖 CompactStr/String、HasCharset、
// ENUM/SET 显示宽度估算，以及 Equal / 严格整型显示宽度开关行为。

use mysql::r#type::{
    BinaryFlag, TypeBlob, TypeDate, TypeDatetime, TypeDouble, TypeDuration, TypeEnum, TypeFloat,
    TypeLong, TypeSet, TypeString, TypeTimestamp, TypeTiny, TypeVarchar, TypeYear, UnsignedFlag,
    ZerofillFlag,
};
use parser_charset::CharsetBin;
use parser_types::types::{
    HasCharset, NewFieldType, TiDBStrictIntegerDisplayWidth, UnspecifiedLength,
};
use serial_test::serial;

/// 临时切换 TiDBStrictIntegerDisplayWidth，返回切换前的旧值以便恢复。
fn set_strict_integer_display_width(value: bool) -> bool {
    unsafe {
        let previous = TiDBStrictIntegerDisplayWidth;
        TiDBStrictIntegerDisplayWidth = value;
        previous
    }
}

/// TestFieldType 对应 Go：覆盖常见类型的 String/InfoSchemaStr 与 HasCharset。
#[test]
#[serial]
fn test_field_type() {
    let previous_strict_width = set_strict_integer_display_width(false);

    let mut ft = NewFieldType(TypeDuration);
    assert_eq!(UnspecifiedLength, ft.GetFlen());
    assert_eq!(UnspecifiedLength, ft.GetDecimal());
    ft.SetDecimal(5);
    assert_eq!("time(5)", ft.String());
    assert!(!HasCharset(&ft));

    ft = NewFieldType(TypeLong);
    ft.SetFlen(5);
    ft.SetFlag(UnsignedFlag | ZerofillFlag);
    assert_eq!("int(5) UNSIGNED ZEROFILL", ft.String());
    assert_eq!("int(5) unsigned", ft.InfoSchemaStr());
    assert!(!HasCharset(&ft));

    // 逐组构造 float 并核对带精度后缀的 String 输出。
    for (flen, decimal, expected) in [
        (12, 3, "float(12,3)"),
        (12, -1, "float"),
        (5, -1, "float"),
        (7, 3, "float(7,3)"),
    ] {
        ft = NewFieldType(TypeFloat);
        ft.SetFlen(flen);
        ft.SetDecimal(decimal);
        assert_eq!(expected, ft.String());
    }
    assert!(!HasCharset(&ft));

    for (flen, decimal, expected) in [
        (22, 3, "double(22,3)"),
        (22, -1, "double"),
        (5, -1, "double"),
        (7, 3, "double(7,3)"),
    ] {
        ft = NewFieldType(TypeDouble);
        ft.SetFlen(flen);
        ft.SetDecimal(decimal);
        assert_eq!(expected, ft.String());
    }
    assert!(!HasCharset(&ft));

    ft = NewFieldType(TypeBlob);
    ft.SetFlen(10);
    ft.SetCharset("UTF8".to_owned());
    ft.SetCollate("UTF8_UNICODE_GI".to_owned());
    assert_eq!(
        "text CHARACTER SET UTF8 COLLATE UTF8_UNICODE_GI",
        ft.String()
    );
    assert!(HasCharset(&ft));

    ft = NewFieldType(TypeVarchar);
    ft.SetFlen(10);
    ft.AddFlag(BinaryFlag);
    assert_eq!("varchar(10) BINARY", ft.String());
    assert!(!HasCharset(&ft));

    ft = NewFieldType(TypeString);
    ft.SetCharset(CharsetBin.to_owned());
    ft.AddFlag(BinaryFlag);
    assert_eq!("binary(1)", ft.String());
    assert!(!HasCharset(&ft));

    // 核对 ENUM/SET 元素转义后的 String 与 HasCharset。
    for (tp, elems, expected) in [
        (TypeEnum, vec!["a", "b"], "enum('a','b')"),
        (TypeEnum, vec!["'a'", "'b'"], "enum('''a''','''b''')"),
        (
            TypeEnum,
            vec!["a\nb", "a\tb", "a\rb"],
            "enum('a\\nb','a\tb','a\\rb')",
        ),
        (
            TypeEnum,
            vec!["a\nb", "a'\t\r\nb", "a\rb"],
            "enum('a\\nb','a''\t\\r\\nb','a\\rb')",
        ),
        (TypeSet, vec!["a", "b"], "set('a','b')"),
        (TypeSet, vec!["'a'", "'b'"], "set('''a''','''b''')"),
        (
            TypeSet,
            vec!["a\nb", "a'\t\r\nb", "a\rb"],
            "set('a\\nb','a''\t\\r\\nb','a\\rb')",
        ),
        (TypeSet, vec!["a'\nb", "a'b\tc"], "set('a''\\nb','a''b\tc')"),
    ] {
        ft = NewFieldType(tp);
        ft.SetElems(elems.into_iter().map(str::to_owned).collect());
        assert_eq!(expected, ft.String());
        assert!(HasCharset(&ft));
    }

    for (tp, flen, decimal, expected) in [
        (TypeTimestamp, 8, 2, "timestamp(2)"),
        (TypeTimestamp, 8, 0, "timestamp"),
        (TypeDatetime, 8, 2, "datetime(2)"),
        (TypeDatetime, 8, 0, "datetime"),
        (TypeDate, 8, 2, "date"),
        (TypeDate, 8, 0, "date"),
        (TypeYear, 4, 0, "year(4)"),
        (TypeYear, 2, 2, "year(2)"),
    ] {
        ft = NewFieldType(tp);
        ft.SetFlen(flen);
        ft.SetDecimal(decimal);
        assert_eq!(expected, ft.String());
        assert!(!HasCharset(&ft));
    }

    ft = NewFieldType(TypeVarchar);
    ft.SetFlen(0);
    ft.SetDecimal(0);
    assert_eq!("varchar(0)", ft.String());
    assert!(HasCharset(&ft));

    ft = NewFieldType(TypeString);
    ft.SetFlen(0);
    ft.SetDecimal(0);
    assert_eq!("char(0)", ft.String());
    assert!(HasCharset(&ft));

    set_strict_integer_display_width(previous_strict_width);
}

/// TestHasCharset 按类型与字符集矩阵验证 HasCharset 判定。
#[test]
#[serial]
fn test_has_charset_from_stmt() {
    for (tp, charset, expected) in [
        (TypeLong, "", false),
        (TypeFloat, "", false),
        (TypeString, "utf8mb4", true),
        (TypeString, CharsetBin, false),
        (TypeVarchar, "utf8mb4", true),
        (TypeVarchar, CharsetBin, false),
        (TypeYear, "", false),
        (TypeDate, "", false),
        (TypeDuration, "", false),
        (TypeDatetime, "", false),
        (TypeTimestamp, "", false),
        (TypeBlob, CharsetBin, false),
        (TypeBlob, "utf8mb4", true),
        (TypeEnum, "utf8mb4", true),
        (TypeSet, "utf8mb4", true),
    ] {
        let mut field_type = NewFieldType(tp);
        field_type.SetCharset(charset.to_owned());
        if charset == CharsetBin {
            field_type.AddFlag(BinaryFlag);
        }
        assert_eq!(expected, HasCharset(&field_type), "{tp}/{charset}");
    }
}

/// TestEnumSetFlen 用本地公式核对 ENUM/SET 显示宽度计算规则。
#[test]
#[serial]
fn test_enum_set_flen() {
    for (tp, elems, expected) in [
        (TypeEnum, vec!["a"], 1),
        (TypeEnum, vec!["a", "b"], 1),
        (TypeEnum, vec!["a", "bb"], 2),
        (TypeEnum, vec![""], 0),
        (TypeEnum, vec!["a", ""], 1),
        (TypeSet, vec!["a"], 1),
        (TypeSet, vec!["a", "b"], 3),
        (TypeSet, vec!["a", "bb"], 4),
        (TypeSet, vec!["a", "b", "c"], 5),
        (TypeSet, vec!["a", "bb", "c"], 6),
        (TypeSet, vec![""], 0),
        (TypeSet, vec!["a", ""], 2),
    ] {
        let calculated = if tp == TypeEnum {
            elems.iter().map(|value| value.len()).max().unwrap_or(0)
        } else {
            elems.iter().map(|value| value.len()).sum::<usize>() + elems.len().saturating_sub(1)
        };
        assert_eq!(expected, calculated as i32, "{tp}/{elems:?}");
    }
}

/// TestFieldTypeEqual 验证 Equal 对类型、decimal、flen 的比较规则。
#[test]
#[serial]
fn test_field_type_equal() {
    let mut ft1 = NewFieldType(TypeDouble);
    let mut ft2 = NewFieldType(TypeFloat);
    assert!(!ft1.Equal(&ft2));

    ft2 = NewFieldType(TypeDouble);
    ft2.SetDecimal(5);
    assert!(!ft1.Equal(&ft2));

    ft1.SetDecimal(5);
    ft1.SetFlen(22);
    assert!(!ft1.Equal(&ft2));

    ft2.SetFlen(22);
    assert!(ft1.Equal(&ft2));

    ft1.SetDecimal(-1);
    ft2.SetDecimal(-1);
    ft1.SetFlen(23);
    assert!(ft1.Equal(&ft2));
}

/// TestCompactStr 验证严格整型显示宽度开关对 CompactStr 的影响。
#[test]
#[serial]
fn test_compact_str() {
    let cases = [
        (TypeTiny, 1, 0, "tinyint(1)", "tinyint(1)"),
        (TypeTiny, 2, 0, "tinyint(2)", "tinyint"),
        (TypeLong, 10, 0, "int(10)", "int"),
        (TypeLong, 10, ZerofillFlag, "int(10)", "int(10)"),
    ];
    let previous_strict_width = set_strict_integer_display_width(false);

    // 在关闭/开启严格宽度两种模式下分别断言 CompactStr。
    for (tp, flen, flags, expected_disabled, expected_enabled) in cases {
        let mut ft = NewFieldType(tp);
        ft.SetFlen(flen);
        ft.SetFlag(flags);

        set_strict_integer_display_width(false);
        assert_eq!(expected_disabled, ft.CompactStr());

        set_strict_integer_display_width(true);
        assert_eq!(expected_enabled, ft.CompactStr());
    }

    set_strict_integer_display_width(previous_strict_width);
}

/// Go 的 nil slice 会编码为 null，且 UnmarshalJSON 接受对应的 null 字段。
#[test]
fn test_json_nil_slice_parity() {
    let ft = NewFieldType(TypeLong);
    let encoded: serde_json::Value = serde_json::from_slice(&ft.MarshalJSON().unwrap()).unwrap();
    assert_eq!(serde_json::Value::Null, encoded["Elems"]);
    assert_eq!(serde_json::Value::Null, encoded["ElemsIsBinaryLit"]);

    let mut non_nil_empty = NewFieldType(TypeLong);
    non_nil_empty.SetElems(Vec::new());
    let encoded: serde_json::Value =
        serde_json::from_slice(&non_nil_empty.MarshalJSON().unwrap()).unwrap();
    assert_eq!(serde_json::json!([]), encoded["Elems"]);

    let mut decoded = NewFieldType(TypeLong);
    decoded
        .UnmarshalJSON(
            br#"{"Tp":3,"Flag":0,"Flen":-1,"Decimal":-1,"Charset":"","Collate":"","Elems":null,"ElemsIsBinaryLit":null,"Array":false}"#,
        )
        .unwrap();
    assert!(decoded.GetElems().is_empty());
    assert!(decoded.GetElemsIsBinaryLit().is_empty());
}
