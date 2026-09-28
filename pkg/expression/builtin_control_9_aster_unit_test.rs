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

// 控制流与字符集转换相关的 Aster 单元测试。
//
// 覆盖 CASE/IF/IFNULL 真值与短路径、类型推导（flen/标志/ENUM 提升），
// 以及 GBK/GB18030 编解码、转换告警与 binary literal 包装策略。

#![allow(dead_code, unused_imports)]

#[path = "builtin_control.rs"]
mod builtin_control;
#[path = "builtin_convert_charset.rs"]
mod builtin_convert_charset;

use builtin_control::{
    BINARY_FLAG, FieldKind, FieldType, NOT_NULL_FLAG, SqlValue, UNSIGNED_FLAG, eval_case_when,
    eval_case_when_rows, eval_if, eval_if_null, infer_type_for_control,
};
use builtin_convert_charset::{
    CHARSET_ASCII, CHARSET_BIN, CHARSET_GB18030, CHARSET_GBK, CHARSET_LATIN1, CHARSET_UTF8MB4,
    COLLATION_GBK_CHINESE_CI, DecodeOptions, ExpressionCollation, FunctionProperty, WarningContext,
    WrapperAction, conversion_property, decode_binary, decode_binary_rows, encode_to_binary,
    encode_to_binary_rows, handle_binary_literal, is_legacy_charset,
};

/// 构造整型 SqlValue 测试夹具。
fn int(value: i64) -> SqlValue {
    SqlValue::Int(value)
}

/// CASE WHEN 应对齐 Go 真值表，并短路径跳过未选中分支的错误。
#[test]
fn case_when_matches_go_table_and_short_circuits() {
    let rows = vec![
        (vec![int(1), int(1), int(1), int(2), int(3)], int(1)),
        (vec![int(0), int(1), int(1), int(2), int(3)], int(2)),
        (vec![SqlValue::Null, int(1), int(1), int(2), int(3)], int(2)),
        (vec![int(0), int(1), int(0), int(2), int(3)], int(3)),
        (
            vec![SqlValue::Null, int(1), SqlValue::Null, int(2), int(3)],
            int(3),
        ),
        (vec![int(0), int(1), SqlValue::Null, int(2), int(3)], int(3)),
        (vec![SqlValue::Null, int(1), int(0), int(2), int(3)], int(3)),
        (
            vec![int(1), SqlValue::Json("3".into()), SqlValue::Null],
            SqlValue::Json("3".into()),
        ),
        (
            vec![int(0), SqlValue::Json("3".into()), SqlValue::Null],
            SqlValue::Null,
        ),
        (vec![SqlValue::Real(0.1), int(1), int(2)], int(1)),
        (
            vec![SqlValue::Real(0.0), int(1), SqlValue::Real(0.1), int(2)],
            int(2),
        ),
    ];
    for (args, expected) in rows {
        assert_eq!(eval_case_when(&args).unwrap(), expected);
    }

    assert_eq!(
        // 已匹配的 THEN 之后的 Error 不应被求值。
        eval_case_when(&[int(1), int(7), SqlValue::Error("unreached".into()), int(8)]).unwrap(),
        int(7)
    );
    // 条件本身出错则立即失败。
    assert!(eval_case_when(&[SqlValue::Error("condition".into()), int(1)]).is_err());
    assert_eq!(eval_case_when(&[int(0), int(1)]).unwrap(), SqlValue::Null);
}

/// IF / IFNULL 的真值转换、NULL 与错误传播应对齐 Go。
#[test]
fn if_and_ifnull_match_go_truth_null_and_error_rules() {
    let cases = vec![
        (int(1), int(1)),
        (SqlValue::Null, int(2)),
        (int(0), int(2)),
        (SqlValue::String("abc".into()), int(2)),
        (SqlValue::String("1abc".into()), int(1)),
        (SqlValue::String("0.1".into()), int(1)),
        (SqlValue::String("0.0".into()), int(2)),
        (SqlValue::Decimal("1.2".into()), int(1)),
        (SqlValue::Decimal("0.1".into()), int(1)),
        (SqlValue::Decimal("0.0".into()), int(2)),
        (SqlValue::Time("2026-09-10 12:34:56".into()), int(1)),
        (SqlValue::Duration(1), int(1)),
        (SqlValue::Duration(0), int(2)),
        (SqlValue::Json("1".into()), int(1)),
    ];
    for (condition, expected) in cases {
        assert_eq!(eval_if(&condition, &int(1), &int(2)).unwrap(), expected);
    }
    assert_eq!(
        eval_if(&int(1), &int(9), &SqlValue::Error("unreached".into())).unwrap(),
        int(9)
    );
    assert!(eval_if(&SqlValue::Error("must error".into()), &int(1), &int(2)).is_err());

    assert_eq!(
        eval_if_null(&int(1), &SqlValue::Error("unreached".into())).unwrap(),
        int(1)
    );
    assert_eq!(eval_if_null(&SqlValue::Null, &int(2)).unwrap(), int(2));
    assert_eq!(
        eval_if_null(&SqlValue::Null, &SqlValue::Null).unwrap(),
        SqlValue::Null
    );
    assert!(eval_if_null(&SqlValue::Error("first".into()), &SqlValue::Null).is_err());
}

/// 按行 CASE 应保持行序，并正确处理 NULL 与未达错误。
#[test]
fn vector_case_preserves_row_order_nulls_and_errors() {
    let rows = vec![
        vec![int(0), int(1), int(1), int(2), int(3)],
        vec![SqlValue::Null, int(1), int(0), int(2)],
        vec![int(1), int(4), SqlValue::Error("unreached".into()), int(5)],
    ];
    assert_eq!(
        eval_case_when_rows(&rows).unwrap(),
        vec![int(2), SqlValue::Null, int(4)]
    );
}

/// 控制函数类型推导应保留 Go 的宽度、标志与 ENUM→VARCHAR 提升。
#[test]
fn control_type_inference_keeps_go_lengths_flags_and_enum_fixups() {
    let signed = FieldType::new(FieldKind::Longlong)
        .with_flen(20)
        .with_decimal(0)
        .with_flags(NOT_NULL_FLAG);
    let unsigned = FieldType::new(FieldKind::Longlong)
        .with_flen(20)
        .with_decimal(0)
        .with_flags(NOT_NULL_FLAG | UNSIGNED_FLAG);
    let null = FieldType::new(FieldKind::Null).with_flags(NOT_NULL_FLAG);

    let integer = infer_type_for_control("if", &[signed.clone(), unsigned, null]).unwrap();
    // 混有 NULL 参数时清除 NOT NULL；有符号/无符号混比保留 Longlong。
    assert_eq!(integer.kind, FieldKind::Longlong);
    assert_eq!(integer.decimal, 0);
    assert_eq!(integer.flags & NOT_NULL_FLAG, 0);
    assert!(integer.flen >= 20);

    let enum_string = FieldType::new(FieldKind::Enum)
        .with_flen(12)
        .with_charset("utf8mb4", "utf8mb4_bin");
    let varchar = FieldType::new(FieldKind::Varchar)
        .with_flen(8)
        .with_charset("utf8mb4", "utf8mb4_general_ci");
    let inferred = infer_type_for_control("case", &[enum_string, varchar]).unwrap();
    // ENUM 与 VARCHAR 混用提升为 VARCHAR，flen 取较大者。
    assert_eq!(inferred.kind, FieldKind::Varchar);
    assert_eq!(inferred.decimal, -1);
    assert_eq!(inferred.flen, 12);

    let binary = FieldType::new(FieldKind::VarString)
        .with_flen(6)
        .with_charset("binary", "binary");
    let mixed = infer_type_for_control("coalesce", &[binary, signed]).unwrap();
    assert_ne!(mixed.flags & BINARY_FLAG, 0);
}

/// 标量/向量编解码应使用真实 GBK、GB18030 编解码器。
#[test]
fn charset_scalar_and_vector_paths_use_real_gbk_and_gb18030_codecs() {
    let gbk = encode_to_binary("中文", CHARSET_GBK).unwrap();
    assert_eq!(gbk, vec![0xd6, 0xd0, 0xce, 0xc4]);
    let mut warnings = WarningContext::default();
    assert_eq!(
        decode_binary(&gbk, CHARSET_GBK, DecodeOptions::default(), &mut warnings)
            .unwrap()
            .unwrap(),
        "中文".as_bytes()
    );
    assert!(warnings.warnings.is_empty());

    let emoji = encode_to_binary("😂", CHARSET_GB18030).unwrap();
    assert_eq!(
        decode_binary(
            &emoji,
            CHARSET_GB18030,
            DecodeOptions::default(),
            &mut warnings
        )
        .unwrap()
        .unwrap(),
        "😂".as_bytes()
    );

    let encoded = encode_to_binary_rows(
        &[Some("中文".to_owned()), None, Some("ASCII".to_owned())],
        CHARSET_GBK,
    )
    .unwrap();
    assert_eq!(encoded[1], None);
    let decoded = decode_binary_rows(
        &encoded,
        CHARSET_GBK,
        DecodeOptions::default(),
        &mut warnings,
    )
    .unwrap();
    assert_eq!(decoded[0].as_deref(), Some("中文".as_bytes()));
    assert_eq!(decoded[1], None);
    assert_eq!(decoded[2].as_deref(), Some(b"ASCII".as_slice()));
}

/// 转换失败应遵循 Go 的告警与严格模式路径。
#[test]
fn conversion_errors_follow_go_warning_and_strict_mode_paths() {
    let invalid_gbk = [0xff, b'a'];
    let mut warnings = WarningContext::default();
    let error = decode_binary(
        &invalid_gbk,
        CHARSET_GBK,
        DecodeOptions::default(),
        &mut warnings,
    )
    .unwrap_err();
    assert!(error.to_string().contains("FF61"));
    assert!(warnings.warnings.is_empty());

    let non_strict = DecodeOptions {
        cannot_convert_as_warning: true,
        strict_mode: false,
    };
    assert!(
        decode_binary(&invalid_gbk, CHARSET_GBK, non_strict, &mut warnings)
            .unwrap()
            .is_some()
    );
    assert_eq!(warnings.warnings.len(), 1);

    let strict = DecodeOptions {
        cannot_convert_as_warning: true,
        strict_mode: true,
    };
    assert_eq!(
        decode_binary(&invalid_gbk, CHARSET_GBK, strict, &mut warnings).unwrap(),
        None
    );
    assert_eq!(warnings.warnings.len(), 2);
}

/// binary literal 包装应对齐 Go 的函数属性表。
#[test]
fn binary_literal_wrapping_matches_go_function_property_table() {
    assert_eq!(conversion_property("char_length"), FunctionProperty::None);
    assert_eq!(conversion_property("sha2"), FunctionProperty::BinaryAware);
    assert_eq!(conversion_property("concat"), FunctionProperty::Auto);
    assert_eq!(conversion_property("unknown"), FunctionProperty::None);
    assert!(is_legacy_charset(CHARSET_UTF8MB4));
    assert!(is_legacy_charset(CHARSET_ASCII));
    assert!(is_legacy_charset(CHARSET_LATIN1));
    assert!(is_legacy_charset(CHARSET_BIN));
    assert!(!is_legacy_charset(CHARSET_GBK));

    let gbk = ExpressionCollation::new(CHARSET_GBK, COLLATION_GBK_CHINESE_CI);
    let binary = ExpressionCollation::new(CHARSET_BIN, CHARSET_BIN);
    assert_eq!(
        handle_binary_literal(CHARSET_GBK, &binary, "sha2", false, false),
        WrapperAction::ToBinary
    );
    assert_eq!(
        handle_binary_literal(CHARSET_UTF8MB4, &binary, "sha2", false, false),
        WrapperAction::Unchanged
    );
    assert_eq!(
        handle_binary_literal(CHARSET_BIN, &gbk, "concat", false, true),
        WrapperAction::FromBinary {
            target: gbk.clone(),
            cannot_convert_as_warning: true
        }
    );
    assert_eq!(
        handle_binary_literal(CHARSET_BIN, &gbk, "concat", true, true),
        WrapperAction::Unchanged
    );
    assert_eq!(
        handle_binary_literal(CHARSET_GBK, &binary, "concat", false, false),
        WrapperAction::ToBinary
    );
    assert_eq!(
        handle_binary_literal(CHARSET_UTF8MB4, &binary, "concat", false, false),
        WrapperAction::Unchanged
    );
}

/// 向量解码须保留 Go 特有的非严格失败单元，而严格模式将其置 NULL。
#[test]
fn vector_decode_error_paths_match_go_row_behavior() {
    let rows = vec![Some(vec![0xff, b'a']), None, Some(vec![0xd6, 0xd0])];
    let mut warnings = WarningContext::default();
    let non_strict = DecodeOptions {
        cannot_convert_as_warning: true,
        strict_mode: false,
    };
    assert_eq!(
        decode_binary_rows(&rows, CHARSET_GBK, non_strict, &mut warnings).unwrap(),
        vec![Some(vec![0xff, b'a']), None, Some("中".as_bytes().to_vec())]
    );
    assert_eq!(warnings.warnings.len(), 1);

    let strict = DecodeOptions {
        cannot_convert_as_warning: true,
        strict_mode: true,
    };
    assert_eq!(
        decode_binary_rows(&rows, CHARSET_GBK, strict, &mut warnings).unwrap(),
        vec![None, None, Some("中".as_bytes().to_vec())]
    );
    assert_eq!(warnings.warnings.len(), 2);
}
