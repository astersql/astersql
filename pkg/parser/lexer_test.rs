// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Native counterparts of lexer_test.go. The integration crate intentionally
// keeps Scanner private, so these tests exercise the same lexer through the
// public Parser and SQL digester entry points.
//
// 对应 Go `lexer_test.go` 的集成测试：通过公开 `Parser` 与 digester
//（SQL 归一化摘要）入口覆盖词法行为。

#[path = "keywords.rs"]
mod keywords;

/// 解析 SQL，失败时返回错误字符串。
fn parse(sql: &str) -> Result<(), String> {
    let mut parser = parser::New();
    parser
        .Parse(sql, "", "")
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 经 digester 归一化 SQL（保留 Hint）。
fn normalize(sql: &str) -> String {
    parser::digester_impl::NormalizeKeepHint(sql)
}

/// 断言 SQL 可被 Parser 成功解析。
fn assert_parse_ok(sql: &str) {
    assert!(
        parse(sql).is_ok(),
        "expected lexer/parser success for {sql:?}: {:?}",
        parse(sql)
    );
}

#[test]
/// 关键字表经 digester 归一化后的形态。
fn test_token_id() {
    assert_eq!(keywords::Keywords.len(), 695);
    // Go TestTokenID iterates tokenMap, not windowFuncTokenMap. The generated
    // Keywords list contains both groups, so disabled window tokens remain
    // identifiers and are quoted by the digester.
    let window_tokens = [
        "CUME_DIST",
        "DENSE_RANK",
        "FIRST_VALUE",
        "GROUPS",
        "LAG",
        "LAST_VALUE",
        "LEAD",
        "NTH_VALUE",
        "NTILE",
        "OVER",
        "PERCENT_RANK",
        "RANK",
        "ROW_NUMBER",
        "WINDOW",
    ];
    for keyword in keywords::Keywords.iter() {
        let normalized = normalize(keyword.Word);
        assert!(
            !normalized.is_empty(),
            "keyword {} was not scanned",
            keyword.Word
        );
        let expected = if keyword.Word == "NULL" {
            "?".to_owned()
        } else if window_tokens.contains(&keyword.Word) {
            format!("`{}`", keyword.Word.to_ascii_lowercase())
        } else {
            keyword.Word.to_ascii_lowercase()
        };
        assert_eq!(normalized, expected, "{}", keyword.Word);
    }
}

#[test]
/// 常见单字符运算符可解析。
fn test_single_char() {
    for sql in [
        "select 1+2",
        "select 2-1",
        "select 2*3",
        "select 4/2",
        "select (1)",
        "select 1,2",
        "select 1%2",
        "select 1=1",
        "select 1<2",
        "select 2>1",
        "select ?",
    ] {
        assert_parse_ok(sql);
    }
}

#[test]
/// 位运算单字符在归一化结果中保留。
fn test_single_char_other() {
    for (sql, symbol) in [
        ("select 1&2", "&"),
        ("select 1|2", "|"),
        ("select 1^2", "^"),
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains(symbol));
    }
}

#[test]
/// 用户变量与系统变量前缀扫描。
fn test_at_leading_identifier() {
    for (sql, expected) in [
        ("select @a", "@a"),
        ("select @@global.a", "@@"),
        ("select @`a b`", "@a b"),
        ("select @'a b'", "@a b"),
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains(expected), "{sql:?}");
    }
}

#[test]
/// 字符集 introducer `_charset` 字面量。
fn test_underscore_cs() {
    for sql in [
        "select _utf8'abc'",
        "select _utf8mb4'abc'",
        "select _binary'abc'",
        "select _latin1'abc'",
        "select _unknown'abc'",
    ] {
        let normalized = normalize(sql);
        assert!(
            normalized.contains("(_charset)") || normalized.contains("_unknown"),
            "{sql}: {normalized}"
        );
    }
}

#[test]
/// 各类字面量可被扫描且归一化非空。
fn test_literal() {
    for sql in [
        "select 0",
        "select 123",
        "select 0.1",
        "select .1",
        "select 1e3",
        "select 1.2e-3",
        "select 'abc'",
        "select x'1a'",
        "select b'10'",
        "select 0x1a",
        "select 0b10",
        r"select \N",
    ] {
        assert!(
            !normalize(sql).is_empty(),
            "literal scanner stalled for {sql:?}"
        );
    }
}

#[test]
/// 不同字面量值归一化为相同占位形状。
fn test_literal_value() {
    let equivalents = [
        ("select 1", "select 2"),
        ("select 1.25", "select 9.75"),
        ("select 'abc'", "select 'xyz'"),
        ("select x'1a'", "select x'ff'"),
        ("select b'10'", "select b'01'"),
    ];
    for (left, right) in equivalents {
        assert_eq!(
            normalize(left),
            normalize(right),
            "literal values must normalize to the same token shape"
        );
    }
}

#[test]
/// 块注释与行注释不改变归一化结果。
fn test_comment() {
    let expected = normalize("select 1");
    for sql in [
        "select /* comment */ 1",
        "select -- comment\n1",
        "select # comment\n1",
    ] {
        assert_eq!(normalize(sql), expected);
        assert_parse_ok(sql);
    }
}

#[test]
/// 反引号标识符扫描与转义。
fn test_scan_quoted_ident() {
    for (sql, ident) in [
        ("select `fk`", "`fk`"),
        ("select `a``b`", "`a`b`"),
        ("select `select`", "`select`"),
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains(ident), "{sql:?}");
    }
}

#[test]
/// 字符串字面量归一化为 `?`。
fn test_scan_string() {
    for sql in [
        r"select 'abc'",
        r"select 'a\nb'",
        r"select 'a''b'",
        r#"select "abc""#,
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains('?'));
    }
}

#[test]
/// NO_BACKSLASH_ESCAPES 下反斜杠不作转义。
fn test_scan_string_with_no_backslash_escapes_mode() {
    let mut p = parser::New();
    p.SetSQLMode(parser::mysql::ModeNoBackslashEscapes);
    assert!(p.Parse(r"select 'a\nb'", "", "").is_ok());
    assert!(p.Parse("select 'a''b'", "", "").is_ok());
}

#[test]
/// 普通与 Unicode 标识符可解析。
fn test_identifier() {
    for sql in [
        "select abc",
        "select a_b",
        "select 中文",
        "select 🥳",
        "select a.b from t as a",
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains('`'));
    }
}

#[test]
/// MySQL 版本条件注释展开。
fn test_special_comment() {
    let sql = "/*!40101 select\n5*/";
    assert_parse_ok(sql);
    let normalized = normalize(sql);
    assert!(
        normalized.contains("select") && normalized.contains('?'),
        "{normalized}"
    );
}

#[test]
/// TiDB 特性注释按特性开关展开或丢弃。
fn test_feature_ids_comment() {
    // Go's TestFeatureIDsComment exercises Scanner directly: enabled feature
    // comments expose their body as tokens even when that body is not a full
    // SQL statement. Direct token/offset assertions live in the internal
    // lexer_5 test; this public integration test verifies the same expansion
    // through the digester without incorrectly requiring statement parsing.
    assert_eq!(
        normalize("/*T![auto_rand] auto_random(5) */"),
        "auto_random ( ? )"
    );
    assert_eq!(
        normalize("/*T![unsupported_feature] unsupported(123) */"),
        ""
    );
}

#[test]
/// 优化器 Hint 在归一化中保留。
fn test_optimizer_hint() {
    let sql = "SELECT /*+ BKA(t1) */ 0;";
    assert_parse_ok(sql);
    let normalized = normalize(sql);
    assert!(normalized.contains("/*+ BKA(t1) */"));
}

#[test]
/// Hint 在 SELECT/UPDATE 等关键字后合法。
fn test_optimizer_hint_after_certain_keyword_only() {
    for sql in [
        "select /*+ HASH_JOIN(t1,t2) */ * from t1 join t2",
        "update /*+ INL_JOIN(t1,t2) */ t1 join t2 set t1.a=1",
        "delete /*+ HASH_JOIN(t1,t2) */ t1 from t1 join t2",
        "insert /*+ MEMORY_QUOTA(1 MB) */ into t values (1)",
    ] {
        assert_parse_ok(sql);
        assert!(normalize(sql).contains("/*+"));
    }
}

#[test]
/// 整型字面量（含超大无）归一化为 `?`。
fn test_int() {
    for value in [
        "0",
        "1",
        "9223372036854775807",
        "9223372036854775808",
        "18446744073709551615",
    ] {
        let sql = format!("select {value}");
        assert_parse_ok(&sql);
        assert_eq!(normalize(&sql), "select ?");
    }
}

#[test]
/// ANSI_QUOTES 下双引号作标识符。
fn test_sql_mode_ansi_quotes() {
    let mut p = parser::New();
    p.SetSQLMode(parser::mysql::ModeANSIQuotes);
    assert!(p.Parse(r#"select "identifier" from t"#, "", "").is_ok());
    assert!(normalize(r#"select "string""#).contains('?'));
}

#[test]
/// 非法未闭合 token 应失败或截断。
fn test_illegal() {
    for sql in [
        "select 'unterminated",
        "select `unterminated",
        "select @`unterminated",
        "select @'unterminated",
        "select @@global.`unterminated",
    ] {
        assert!(
            parse(sql).is_err() || normalize(sql).trim_end().ends_with("select"),
            "illegal token unexpectedly survived: {sql:?}"
        );
    }
}

#[test]
/// 版本注释前缀不泄漏到归一化结果。
fn test_version_digits() {
    for sql in [
        "/*!40101 SELECT 1 */",
        "/*!99999 SELECT 2 */",
        "/*! SELECT 3 */",
    ] {
        let normalized = normalize(sql);
        assert!(
            !normalized.contains("/*!"),
            "version prefix leaked: {normalized}"
        );
    }
}

#[test]
/// 特性 ID 列表支持与否影响正文是否展开。
fn test_feature_ids() {
    let cases = [
        ("/*T![auto_rand] auto_random(5) */", true),
        ("/*T![auto_rand, clustered_index] auto_random(5) */", true),
        ("/*T![unsupported_feature] unsupported(123) */", false),
        ("/*T! auto_random(5) */", true),
    ];
    for (sql, supported) in cases {
        let normalized = normalize(sql);
        assert_eq!(!normalized.is_empty(), supported, "{sql:?}: {normalized:?}");
    }
}
