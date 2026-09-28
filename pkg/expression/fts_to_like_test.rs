// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// FTS→ILIKE 内核的专项单元测试。
//
// 覆盖搜索串校验矩阵、布尔词解析、LIKE 转义、从 builtin 构建表达式，
// 以及非默认 FTS 修饰符对 TiFlash 下推的拒绝逻辑。

use crate::fts_to_like_kernel::*;
use crate::infer_pushdown_kernel::{PushDownContext, StoreType, can_expr_push_down};

/// 构造测试用 VARCHAR 列表达式。
fn col(id: i64, index: usize) -> Expression {
    Expression::column(id, index, FieldType::varchar())
}

/// 自然语言/布尔模式下合法与非法搜索串的完整对照表。
#[test]
fn test_validate_fts_search_string_for_like_fallback() {
    let cases = [
        ("", FulltextSearchModifier::NaturalLanguage, false),
        (" \t\n ", FulltextSearchModifier::NaturalLanguage, false),
        ("MySQL", FulltextSearchModifier::NaturalLanguage, false),
        (
            "MySQL tutorial PostgreSQL",
            FulltextSearchModifier::NaturalLanguage,
            false,
        ),
        (
            "abc123 mysql8",
            FulltextSearchModifier::NaturalLanguage,
            false,
        ),
        ("x-x", FulltextSearchModifier::NaturalLanguage, true),
        ("MySQL,", FulltextSearchModifier::NaturalLanguage, true),
        ("+word", FulltextSearchModifier::NaturalLanguage, true),
        ("-word", FulltextSearchModifier::NaturalLanguage, true),
        (r#""phrase""#, FulltextSearchModifier::NaturalLanguage, true),
        ("word*", FulltextSearchModifier::NaturalLanguage, true),
        ("100%", FulltextSearchModifier::NaturalLanguage, true),
        ("test_file", FulltextSearchModifier::NaturalLanguage, true),
        ("", FulltextSearchModifier::Boolean, false),
        ("MySQL", FulltextSearchModifier::Boolean, false),
        ("+MySQL", FulltextSearchModifier::Boolean, false),
        ("-MySQL", FulltextSearchModifier::Boolean, false),
        ("+apple -cherry pie", FulltextSearchModifier::Boolean, false),
        ("xx-yy", FulltextSearchModifier::Boolean, true),
        ("+", FulltextSearchModifier::Boolean, true),
        ("-", FulltextSearchModifier::Boolean, true),
        ("x+y", FulltextSearchModifier::Boolean, true),
        ("word*", FulltextSearchModifier::Boolean, true),
        ("+word*", FulltextSearchModifier::Boolean, true),
        (">word", FulltextSearchModifier::Boolean, true),
        ("<word", FulltextSearchModifier::Boolean, true),
        ("~word", FulltextSearchModifier::Boolean, true),
        (r#""exact phrase""#, FulltextSearchModifier::Boolean, true),
        (
            r#"+"required phrase""#,
            FulltextSearchModifier::Boolean,
            true,
        ),
        ("(word)", FulltextSearchModifier::Boolean, true),
        ("+100%", FulltextSearchModifier::Boolean, true),
        ("你好", FulltextSearchModifier::NaturalLanguage, false),
        ("+你好", FulltextSearchModifier::Boolean, false),
    ];
    for (text, modifier, want_error) in cases {
        assert_eq!(
            validate_fts_search_string_for_like_fallback(text, modifier).is_err(),
            want_error,
            "text={text:?}, modifier={modifier:?}",
        );
    }
}

/// 布尔模式空白切分与 +/- 前缀解析。
#[test]
fn test_parse_fts_boolean_search_string() {
    let cases = [
        (
            "+apple +pie",
            vec![
                FtsSearchTerm::required("apple"),
                FtsSearchTerm::required("pie"),
            ],
        ),
        (
            "+apple -cherry",
            vec![
                FtsSearchTerm::required("apple"),
                FtsSearchTerm::excluded("cherry"),
            ],
        ),
        (
            "word1 word2 word3",
            vec![
                FtsSearchTerm::optional("word1"),
                FtsSearchTerm::optional("word2"),
                FtsSearchTerm::optional("word3"),
            ],
        ),
        (
            "word1\t\nword2",
            vec![
                FtsSearchTerm::optional("word1"),
                FtsSearchTerm::optional("word2"),
            ],
        ),
        ("", vec![]),
        ("   \t\n  ", vec![]),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_fts_boolean_search_string(input),
            expected,
            "{input:?}"
        );
    }
}

/// 单个检索词前缀：`+`/`-`/无前缀及空串边界。
#[test]
fn test_parse_fts_search_term() {
    let cases = [
        ("+word", FtsSearchTerm::required("word")),
        ("-word", FtsSearchTerm::excluded("word")),
        ("word", FtsSearchTerm::optional("word")),
        ("", FtsSearchTerm::optional("")),
        ("+", FtsSearchTerm::required("")),
        ("-", FtsSearchTerm::excluded("")),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_fts_search_term(input), expected);
    }
}

/// LIKE 特殊字符 `\ % _` 的转义结果。
#[test]
fn test_escape_fts_like_pattern() {
    for (input, expected) in [
        ("normal text", "normal text"),
        ("100%", r"100\%"),
        ("test_file", r"test\_file"),
        (r"path\to\file", r"path\\to\\file"),
        ("mix_%_all", r"mix\_\%\_all"),
        (r"\%_", r"\\\%\_"),
        ("", ""),
    ] {
        assert_eq!(escape_fts_like_pattern(input), expected, "{input:?}");
    }
}

/// 便捷构造指定列数的 `fts_mysql_match_against` builtin。
fn fts_builtin(search: Datum, columns: usize, modifier: FulltextSearchModifier) -> ScalarFunction {
    ScalarFunction::fts(
        Expression::constant(search),
        (0..columns)
            .map(|index| col(index as i64 + 1, index))
            .collect(),
        modifier,
    )
}

/// 从 builtin 构建：错误函数名、单列成功、多列拒绝、NULL 直通、非法词拒绝。
#[test]
fn test_build_fts_to_i_like_expression_from_builtin() {
    let wrong = ScalarFunction::new(
        "length",
        Signature::Generic("Length".into()),
        vec![col(1, 0)],
        FieldType::integer(),
    );
    assert!(build_fts_to_ilike_expression_from_builtin(&wrong).is_err());

    let mut wrong_signature = ScalarFunction::new(
        "fts_mysql_match_against",
        Signature::Generic("Length".into()),
        vec![
            Expression::constant(Datum::String("mysql".into())),
            col(1, 0),
        ],
        FieldType::new(FieldKind::Real),
    );
    wrong_signature.modifier = Some(FulltextSearchModifier::NaturalLanguage);
    assert!(
        build_fts_to_ilike_expression_from_builtin(&wrong_signature).is_err(),
        "the Go implementation rejects a non-FTS builtin signature even when the function name matches",
    );

    let single = fts_builtin(
        Datum::String("mysql".into()),
        1,
        FulltextSearchModifier::NaturalLanguage,
    );
    let expression = build_fts_to_ilike_expression_from_builtin(&single).unwrap();
    assert!(matches!(expression, Expression::ScalarFunction(_)));

    let multi = fts_builtin(
        Datum::String("mysql".into()),
        2,
        FulltextSearchModifier::NaturalLanguage,
    );
    assert!(build_fts_to_ilike_expression_from_builtin(&multi).is_err());

    let null = fts_builtin(Datum::Null, 1, FulltextSearchModifier::NaturalLanguage);
    assert_eq!(
        build_fts_to_ilike_expression_from_builtin(&null).unwrap(),
        Expression::constant(Datum::Null),
    );

    let unsupported = fts_builtin(
        Datum::String("xx-yy".into()),
        1,
        FulltextSearchModifier::NaturalLanguage,
    );
    assert!(build_fts_to_ilike_expression_from_builtin(&unsupported).is_err());
}

/// TiFlash 仅允许默认自然语言修饰符的 FTS；布尔/查询扩展不可下推。
#[test]
fn test_scalar_expr_supported_by_flash_rejects_non_default_fts_modifier() {
    let context = PushDownContext::new(false, None, None, 0);
    for (modifier, expected) in [
        (FulltextSearchModifier::NaturalLanguage, true),
        (FulltextSearchModifier::Boolean, false),
        (
            FulltextSearchModifier::NaturalLanguageWithQueryExpansion,
            false,
        ),
    ] {
        let expression =
            Expression::ScalarFunction(fts_builtin(Datum::String("mysql".into()), 1, modifier));
        assert_eq!(
            can_expr_push_down(&context, &expression, StoreType::TiFlash, true),
            expected
        );
    }
}
