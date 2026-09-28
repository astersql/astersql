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

// 字符串内建向量化内核与 Go 对齐的单元测试。
//
// 通过 `eval_rows` 驱动 `StringBuiltin` 各变体，覆盖 NULL、字符边界、
// max_allowed_packet、binary/UTF-8 定位、编码转换、可变 arity 与 Base64 等。

use crate::string_vec::{EvalConfig, StringBuiltin, Value, eval_rows};

/// 将 &str 包装为 Bytes 列值。
fn text(value: &str) -> Value {
    Value::Bytes(value.as_bytes().to_vec())
}

/// 将可选字符串列表转为 Value 列（None → Null）。
fn texts(values: &[Option<&str>]) -> Vec<Value> {
    values
        .iter()
        .map(|value| value.map_or(Value::Null, text))
        .collect()
}

/// 单行求值并断言结果。
fn assert_one(builtin: StringBuiltin, row: Vec<Value>, expected: Value) {
    let output = eval_rows(&builtin, &[row], &EvalConfig::default()).unwrap();
    assert_eq!(output.values, vec![expected], "builtin: {builtin:?}");
}

#[test]
/// 验证 LOWER/LEFT 等在 NULL 与多字节字符边界上的行为。
fn vector_null_case_and_character_boundaries_match_go() {
    let config = EvalConfig::default();
    let rows = vec![
        vec![text("Straße")],
        vec![text("你好TiDB")],
        vec![Value::Null],
    ];
    assert_eq!(
        eval_rows(&StringBuiltin::LowerUtf8, &rows, &config)
            .unwrap()
            .values,
        texts(&[Some("straße"), Some("你好tidb"), None])
    );

    let rows = vec![
        vec![text("你好TiDB"), Value::Int(3)],
        vec![text("abc"), Value::Int(-1)],
        vec![Value::Null, Value::Int(2)],
    ];
    assert_eq!(
        eval_rows(&StringBuiltin::LeftUtf8, &rows, &config)
            .unwrap()
            .values,
        texts(&[Some("你好T"), Some(""), None])
    );
}

#[test]
/// 验证 max_allowed_packet 截断告警与 CONCAT_WS 的 NULL 规则。
fn vector_packet_limits_and_null_rules_match_go() {
    let config = EvalConfig {
        max_allowed_packet: 5,
        truncate_as_warning: true,
        ..EvalConfig::default()
    };
    let rows = vec![
        vec![text("ab"), Value::Int(3)],
        vec![text("ab"), Value::Int(2)],
        vec![Value::Null, Value::Int(2)],
    ];
    let output = eval_rows(&StringBuiltin::Repeat, &rows, &config).unwrap();
    assert_eq!(output.values, texts(&[None, Some("abab"), None]));
    assert_eq!(output.warnings.len(), 1);

    let rows = vec![
        vec![text(","), text("a"), Value::Null, text("b")],
        vec![Value::Null, text("a"), text("b")],
    ];
    assert_eq!(
        eval_rows(&StringBuiltin::ConcatWs, &rows, &EvalConfig::default())
            .unwrap()
            .values,
        texts(&[Some("a,b"), None])
    );
}

#[test]
/// 验证 LOCATE 等 binary/UTF-8 定位语义。
fn vector_binary_and_utf8_positions_match_go() {
    let config = EvalConfig::default();
    let rows = vec![
        vec![text("好"), text("你好好")],
        vec![text(""), text("abc")],
    ];
    assert_eq!(
        eval_rows(
            &StringBuiltin::Locate2Utf8 {
                collation: "utf8mb4_bin".into()
            },
            &rows,
            &config,
        )
        .unwrap()
        .values,
        vec![Value::Int(2), Value::Int(1)]
    );

    assert_eq!(
        eval_rows(&StringBuiltin::Length, &[vec![text("你好")]], &config)
            .unwrap()
            .values,
        vec![Value::Int(6)]
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::CharLengthUtf8,
            &[vec![text("你好")]],
            &config,
        )
        .unwrap()
        .values,
        vec![Value::Int(2)]
    );
}

#[test]
/// 验证 CONVERT、TRANSLATE 与 FORMAT 向量路径。
fn vector_encoding_translation_and_formatting_match_go() {
    let config = EvalConfig::default();
    assert_eq!(
        eval_rows(&StringBuiltin::HexStr, &[vec![text("TiDB")]], &config)
            .unwrap()
            .values,
        texts(&[Some("54694442")])
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::Unhex,
            &[vec![text("F"),], vec![text("GG")]],
            &config,
        )
        .unwrap()
        .values,
        vec![Value::Bytes(vec![0x0f]), Value::Null]
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::TranslateUtf8,
            &[vec![text("世界你好"), text("世界你"), text("Ti")]],
            &config,
        )
        .unwrap()
        .values,
        texts(&[Some("Ti好")])
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::Format,
            &[vec![Value::Real(12345.678), Value::Int(2)]],
            &config,
        )
        .unwrap()
        .values,
        texts(&[Some("12,345.68")])
    );
}

#[test]
/// 验证可变参数函数与 Base64 编解码。
fn vector_variable_arity_and_base64_match_go() {
    let config = EvalConfig::default();
    assert_eq!(
        eval_rows(
            &StringBuiltin::Elt,
            &[
                vec![Value::Int(2), text("a"), text("b")],
                vec![Value::Int(0), text("a"), text("b")],
            ],
            &config,
        )
        .unwrap()
        .values,
        texts(&[Some("b"), None])
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::MakeSet,
            &[vec![Value::Int(5), text("a"), Value::Null, text("c")]],
            &config,
        )
        .unwrap()
        .values,
        texts(&[Some("a,c")])
    );

    let encoded = eval_rows(
        &StringBuiltin::ToBase64,
        &[vec![text(&"x".repeat(58))]],
        &config,
    )
    .unwrap()
    .values
    .remove(0);
    let decoded = eval_rows(&StringBuiltin::FromBase64, &[vec![encoded]], &config)
        .unwrap()
        .values
        .remove(0);
    assert_eq!(decoded, text(&"x".repeat(58)));
}

#[test]
/// 二进制签名矩阵：批量对照 Go 期望值。
fn vector_binary_string_matrix_matches_go() {
    use StringBuiltin::*;

    assert_one(LowerBinary, vec![text("AbC")], text("AbC"));
    assert_one(UpperBinary, vec![text("AbC")], text("AbC"));
    assert_one(StringIsNull, vec![Value::Null], Value::Int(1));
    assert_one(Space, vec![Value::Int(3)], text("   "));
    assert_one(Concat, vec![text("Ti"), text("DB"), text("")], text("TiDB"));
    assert_one(LTrim, vec![text("  x ")], text("x "));
    assert_one(RTrim, vec![text("  x ")], text("  x"));
    assert_one(Trim1, vec![text("  x ")], text("x"));
    assert_one(
        Quote,
        vec![text("a'b\\\0\u{1a}")],
        text("'a\\'b\\\\\\0\\Z'"),
    );
    assert_one(
        InsertBinary,
        vec![
            text("Quadratic"),
            Value::Int(3),
            Value::Int(4),
            text("What"),
        ],
        text("QuWhattic"),
    );
    assert_one(
        SubstringIndex {
            count_unsigned: false,
        },
        vec![text("a,b,c"), text(","), Value::Int(-2)],
        text("b,c"),
    );
    assert_one(Ascii, vec![text("")], Value::Int(0));
    assert_one(
        LpadBinary,
        vec![text("hi"), Value::Int(5), text("xy")],
        text("xyxhi"),
    );
    assert_one(
        RpadBinary,
        vec![text("hi"), Value::Int(5), text("xy")],
        text("hixyx"),
    );
    assert_one(LeftBinary, vec![text("abc"), Value::Int(2)], text("ab"));
    assert_one(RightBinary, vec![text("abc"), Value::Int(2)], text("bc"));
    assert_one(ReverseBinary, vec![text("abc")], text("cba"));
    assert_one(
        Strcmp {
            collation: "utf8mb4_general_ci".into(),
        },
        vec![text("A"), text("a")],
        Value::Int(0),
    );
    assert_one(Locate2Binary, vec![text("bc"), text("abcd")], Value::Int(2));
    assert_one(
        Locate3Binary,
        vec![text("a"), text("banana"), Value::Int(4)],
        Value::Int(4),
    );
    assert_one(
        Substring2Binary,
        vec![text("abcd"), Value::Int(-2)],
        text("cd"),
    );
    assert_one(
        Substring3Binary,
        vec![text("abcd"), Value::Int(2), Value::Int(2)],
        text("bc"),
    );
    assert_one(Trim2, vec![text("xxabcxx"), text("xx")], text("abc"));
    assert_one(
        Trim3,
        vec![text("xxabcxx"), text("xx"), Value::Int(2)],
        text("abcxx"),
    );
    assert_one(
        InstrBinary,
        vec![text("banana"), text("ana")],
        Value::Int(2),
    );
    assert_one(Length, vec![text("你好")], Value::Int(6));
    assert_one(BitLength, vec![text("ab")], Value::Int(16));
    assert_one(
        Replace,
        vec![text("www.mysql.com"), text("mysql"), text("tidb")],
        text("www.tidb.com"),
    );
    assert_one(OctInt, vec![Value::Int(8)], text("10"));
    assert_one(Bin, vec![Value::Int(5)], text("101"));
    assert_one(HexInt, vec![Value::Int(255)], text("FF"));
    assert_one(CharLengthBinary, vec![text("你好")], Value::Int(6));
    assert_one(
        TranslateBinary,
        vec![text("abcdef"), text("ace"), text("XY")],
        text("XbYdf"),
    );
}

#[test]
/// UTF-8 与可变 arity 矩阵：批量对照 Go 期望值。
fn vector_utf8_and_variable_arity_matrix_matches_go() {
    use StringBuiltin::*;

    assert_one(UpperUtf8, vec![text("straße")], text("STRAßE"));
    assert_one(ReverseUtf8, vec![text("你a好")], text("好a你"));
    assert_one(
        Locate3Utf8 {
            collation: "utf8mb4_general_ci".into(),
        },
        vec![text("A"), text("你aA"), Value::Int(2)],
        Value::Int(2),
    );
    assert_one(
        Convert {
            source_charset: "utf8mb4".into(),
            target_charset: "utf8mb4".into(),
        },
        vec![text("你好")],
        text("你好"),
    );
    assert_one(
        FindInSet {
            collation: "utf8mb4_general_ci".into(),
        },
        vec![text("B"), text("a,b,c")],
        Value::Int(2),
    );
    assert_one(
        LpadUtf8,
        vec![text("好"), Value::Int(3), text("世界")],
        text("世界好"),
    );
    assert_one(
        RpadUtf8,
        vec![text("好"), Value::Int(3), text("世界")],
        text("好世界"),
    );
    assert_one(
        Substring2Utf8,
        vec![text("你好世界"), Value::Int(-2)],
        text("世界"),
    );
    assert_one(
        Substring3Utf8,
        vec![text("你好世界"), Value::Int(2), Value::Int(2)],
        text("好世"),
    );
    assert_one(
        InstrUtf8 {
            collation: "utf8mb4_bin".into(),
        },
        vec![text("你好好"), text("好")],
        Value::Int(2),
    );
    assert_one(
        InsertUtf8,
        vec![text("你好世界"), Value::Int(2), Value::Int(2), text("TiDB")],
        text("你TiDB界"),
    );
    assert_one(Ord, vec![text("你")], Value::Int(14_990_752));
    assert_one(
        Locate2Utf8 {
            collation: "utf8mb4_bin".into(),
        },
        vec![text("好"), text("你好")],
        Value::Int(2),
    );
    assert_one(
        Char {
            charset: "ascii".into(),
        },
        vec![Value::Int(65), Value::Null, Value::Int(66)],
        text("AB"),
    );
    assert_one(CharLengthUtf8, vec![text("你好")], Value::Int(2));
    assert_one(
        ExportSet5,
        vec![
            Value::Int(5),
            text("Y"),
            text("N"),
            text(","),
            Value::Int(3),
        ],
        text("Y,N,Y"),
    );
    assert_one(
        ExportSet4,
        vec![Value::Int(0), text("Y"), text("N"), text("|")],
        text(&vec!["N"; 64].join("|")),
    );
    assert_one(
        ExportSet3,
        vec![Value::Int(-1), text("Y"), text("N")],
        text(&vec!["Y"; 64].join(",")),
    );
    assert_one(OctString, vec![text("8xyz")], text("10"));
    assert_one(
        TranslateUtf8,
        vec![text("世界你好"), text("世界你"), text("Ti")],
        text("Ti好"),
    );
    assert_one(
        FormatWithLocale,
        vec![
            Value::Decimal("1234.555".into()),
            Value::Int(2),
            text("en_US"),
        ],
        text("1,234.56"),
    );
}

/// 供向量化字符串 Go 同名入口复用的完整回归集合。
pub(crate) fn run_string_vector_parity_suite() {
    vector_null_case_and_character_boundaries_match_go();
    vector_packet_limits_and_null_rules_match_go();
    vector_binary_and_utf8_positions_match_go();
    vector_encoding_translation_and_formatting_match_go();
    vector_variable_arity_and_base64_match_go();
    vector_binary_string_matrix_matches_go();
    vector_utf8_and_variable_arity_matrix_matches_go();
}
