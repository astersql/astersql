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
// See the License for the specific language governing permissions and
// limitations under the License.

// 解析器测试驱动（test_driver）迁移对齐单元测试。
//
// 对照 Go 侧边界：校验整数位数估算、BIT/HEX 字面量、MyDecimal 往返、
// Datum/默认 FieldType，以及 ValueExpr/ParamMarkerExpr 的还原与格式化。

use super::*;
use std::any::Any;

/// 校验 StrLenOfUint64Fast/StrLenOfInt64Fast 与 Go 边界一致。
#[test]
fn helper_lengths_match_go_boundaries() {
    assert_eq!(StrLenOfUint64Fast(0), 1);
    assert_eq!(StrLenOfUint64Fast(9), 1);
    assert_eq!(StrLenOfUint64Fast(10), 2);
    assert_eq!(StrLenOfUint64Fast(u64::MAX), 20);
    assert_eq!(StrLenOfInt64Fast(-1), 2);
    assert_eq!(StrLenOfInt64Fast(i64::MIN), 20);
}

/// 校验 BIT/HEX 字面量解析与二进制字面量格式化遵循 MySQL 语法。
#[test]
fn bit_and_hex_literals_follow_mysql_syntax() {
    assert_eq!(ParseBitStr("b'101'").unwrap().0, vec![5]);
    assert_eq!(ParseBitStr("0b100000000").unwrap().0, vec![1, 0]);
    assert_eq!(ParseBitStr("B''").unwrap().0, Vec::<u8>::new());
    assert!(ParseBitStr("101").is_err());
    assert!(ParseBitStr("b'102'").is_err());

    assert_eq!(ParseHexStr("x'0aFF'").unwrap().0, vec![0x0a, 0xff]);
    assert_eq!(ParseHexStr("0xabc").unwrap().0, vec![0x0a, 0xbc]);
    assert!(ParseHexStr("x'abc'").is_err());
    assert_eq!(BinaryLiteral(vec![0, 15]).String(), "0x000f");
    assert_eq!(
        BinaryLiteral(vec![1]).ToBitLiteralString(false),
        "b'00000001'"
    );
    assert_eq!(BinaryLiteral(vec![1]).ToBitLiteralString(true), "b'1'");
}

/// 校验 MyDecimal 对 Go 测试驱动支持的十进制字符串可往返。
#[test]
fn decimal_round_trips_go_supported_forms() {
    for (input, expected) in [
        ("0", "0"),
        ("-0", "0"),
        ("+12", "12"),
        (".125", "0.125"),
        ("00123.4500", "123.4500"),
        (
            "123456789012345678.000000001",
            "123456789012345678.000000001",
        ),
    ] {
        let mut decimal = MyDecimal::default();
        decimal.FromString(input.as_bytes()).unwrap();
        assert_eq!(decimal.String(), expected, "input={input}");
    }
}

/// 校验 Datum 存取与 DefaultTypeForValue 的 type switch 与 Go 对齐。
#[test]
fn datum_and_default_types_match_go_switches() {
    let mut datum = NewDatum(Box::new(true));
    assert_eq!(datum.Kind(), KindInt64);
    assert_eq!(datum.GetInt64(), 1);

    datum.SetUint64(u64::MAX);
    assert_eq!(datum.GetUint64(), u64::MAX);
    datum.SetFloat32(1.25);
    assert_eq!(datum.GetFloat32(), 1.25);
    datum.SetBytesAsString(vec![0xff, b'a']);
    assert_eq!(datum.GetBytes(), &[0xff, b'a']);

    let mut field_type = types::FieldType::default();
    DefaultTypeForValue(&true, &mut field_type, "utf8mb4", "utf8mb4_bin");
    assert_eq!(field_type.GetType(), mysql::TypeLonglong);
    assert_ne!(field_type.GetFlag() & mysql::IsBooleanFlag, 0);
    assert_eq!(field_type.GetFlen(), 1);

    DefaultTypeForValue(
        &String::from("你好"),
        &mut field_type,
        "utf8mb4",
        "utf8mb4_bin",
    );
    assert_eq!(field_type.GetType(), mysql::TypeVarString);
    assert_eq!(field_type.GetFlen(), 6);
    assert_eq!(field_type.GetCharset(), "utf8mb4");
}

/// 校验 GetValue 对 NULL 与动态 Interface 载荷的保留语义。
#[test]
fn datum_get_value_preserves_null_and_interface_payloads() {
    let null = NewDatum(Box::new(()));
    assert!(matches!(null.GetValue(), DatumValue::Null));

    let dynamic = NewDatum(Box::new(vec![1_i32, 2, 3]));
    let DatumValue::Interface(value) = dynamic.GetValue() else {
        panic!("expected interface payload");
    };
    assert_eq!(value.downcast_ref::<Vec<i32>>().unwrap(), &[1, 2, 3]);
}

/// 校验 ValueExpr/ParamMarkerExpr 对布尔、字符串与占位符的还原与 Format。
#[test]
fn value_expr_restores_and_formats_core_kinds() {
    let boolean = ValueExpr::new(Box::new(true), "utf8mb4", "utf8mb4_bin");
    assert_eq!(
        boolean
            .RestoreToString(format::DefaultRestoreFlags)
            .unwrap(),
        "TRUE"
    );
    let mut formatted = Vec::new();
    boolean.Format(&mut formatted);
    assert_eq!(formatted, b"TRUE");

    let string = ValueExpr::new(Box::new(String::from("a'b")), "utf8mb4", "utf8mb4_bin");
    let flags = format::DefaultRestoreFlags | format::RestoreStringSingleQuotes;
    // 带字符集前缀且单引号转义，对齐 RestoreStringSingleQuotes。
    assert_eq!(string.RestoreToString(flags).unwrap(), "_UTF8MB4'a''b'");
    assert_eq!(string.GetProjectionOffset(), -1);

    let marker = ParamMarkerExpr::new(17);
    assert_eq!(marker.Offset, 17);
    assert_eq!(
        marker.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "?"
    );
}

/// Go newValueExpr 对已有 ValueExpr 直接返回，不重置 datum、类型或 projection offset。
#[test]
fn new_value_expr_reuses_an_existing_expression() {
    let mut original = ValueExpr::new(Box::new(42_i64), "", "");
    original.SetProjectionOffset(9);

    let reused = newValueExpr(Box::new(original), "utf8mb4", "utf8mb4_bin");
    assert_eq!(reused.datum.Kind(), KindInt64);
    assert_eq!(reused.datum.GetInt64(), 42);
    assert_eq!(reused.GetProjectionOffset(), 9);
}

struct ReplacingVisitor {
    skip: bool,
}

impl Visitor for ReplacingVisitor {
    fn Enter(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool) {
        if node.is::<ValueExpr>() {
            let mut replacement = ValueExpr::default();
            replacement.SetProjectionOffset(23);
            (Box::new(replacement), self.skip)
        } else {
            (node, self.skip)
        }
    }

    fn Leave(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool) {
        (node, true)
    }
}

/// Accept 必须保留 Go visitor 的 Enter 替换节点与 Leave 返回值。
#[test]
fn accept_preserves_enter_replacement_like_go() {
    for skip in [false, true] {
        let (node, ok) = Box::new(ValueExpr::default()).Accept(&mut ReplacingVisitor { skip });
        let expression = node.downcast::<ValueExpr>().unwrap();
        assert_eq!(expression.GetProjectionOffset(), 23);
        assert!(ok);
    }
}

/// 校验浮点/十六进制还原格式，以及未实现 kind 返回 not implemented。
#[test]
fn value_expr_keeps_float_binary_and_unsupported_boundaries() {
    let float = ValueExpr::new(Box::new(1.25_f64), "", "");
    assert_eq!(
        float.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "1.25e+00"
    );

    let hex = ValueExpr::new(Box::new(HexLiteral(vec![0x0a, 0xff])), "", "");
    assert_eq!(
        hex.RestoreToString(format::DefaultRestoreFlags).unwrap(),
        "x'0aff'"
    );

    let unsupported = ValueExpr::new(Box::new(vec![1_i32]), "", "");
    let error = unsupported
        .RestoreToString(format::DefaultRestoreFlags)
        .unwrap_err();
    assert_eq!(error.to_string(), "not implemented");
}

/// Go strconv.FormatFloat 会直接输出特殊值，不要求存在科学计数法指数。
#[test]
fn float_special_values_restore_and_size_like_go() {
    let mut finite_f32_type = types::FieldType::default();
    DefaultTypeForValue(&1.2_f32, &mut finite_f32_type, "", "");
    assert_eq!(finite_f32_type.GetFlen(), 3);

    for (value, expected) in [
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
    ] {
        let expression = ValueExpr::new(Box::new(value), "", "");
        assert_eq!(
            expression
                .RestoreToString(format::DefaultRestoreFlags)
                .unwrap(),
            expected
        );

        let mut formatted = Vec::new();
        expression.Format(&mut formatted);
        assert_eq!(String::from_utf8(formatted).unwrap(), expected);

        let mut field_type = types::FieldType::default();
        DefaultTypeForValue(&value, &mut field_type, "", "");
        assert_eq!(field_type.GetFlen(), expected.len() as isize);
    }
}
