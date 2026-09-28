// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `lexer.rs` 内部词法扫描行为的单元测试。
//
// 直接构造 `Scanner`，核对单字符/变量 token、字面量、转义、注释、
// 版本与特性扫描、动态字面量值、SQL Mode 及优化器 Hint 位置与 Go 一致。

use super::*;

/// 取扫描器返回的首个 token 与语义值。
fn first_token(input: &str) -> (i32, yySymType) {
    let mut scanner = NewScanner(input.to_owned());
    let mut value = yySymType::default();
    let token = scanner.Lex(&mut value);
    (token, value)
}

/// 断言 `LexLiteral` 返回 Go 测试所要求的具体动态类型和值。
fn assert_literal_value<T>(input: &str, expected: T)
where
    T: std::any::Any + std::fmt::Debug + PartialEq,
{
    let mut scanner = NewScanner(input.to_owned());
    let actual = scanner.LexLiteral();
    assert_eq!(
        actual.downcast_ref::<T>(),
        Some(&expected),
        "{input:?}: unexpected literal type or value"
    );
}

#[test]
/// 单字符运算符与 `@`/`@@` 用户/系统变量 token。
fn lexer_matches_go_single_character_and_variable_tokens() {
    for byte in [
        b'|', b'&', b'-', b'+', b'*', b'/', b'%', b'^', b'~', b'(', b',', b')',
    ] {
        assert_eq!(first_token(&(byte as char).to_string()).0, byte as i32);
    }
    for (input, expected) in [
        ("AT", token::identifier),
        ("?", token::paramMarker),
        ("PLACEHOLDER", token::identifier),
        ("=", token::eq),
        (".", b'.' as i32),
        ("@", token::singleAtIdentifier),
        ("@''", token::singleAtIdentifier),
        ("@1", token::singleAtIdentifier),
        ("@.1_", token::singleAtIdentifier),
        ("@-1.", token::singleAtIdentifier),
        ("@~", token::singleAtIdentifier),
        ("@$", token::singleAtIdentifier),
        ("@a_3cbbc", token::singleAtIdentifier),
        ("@`a_3cbbc`", token::singleAtIdentifier),
        ("@-3cbbc", token::singleAtIdentifier),
        ("@!3cbbc", token::singleAtIdentifier),
        ("@@global.test", token::doubleAtIdentifier),
        ("@@session.test", token::doubleAtIdentifier),
        ("@@local.test", token::doubleAtIdentifier),
        ("@@test", token::doubleAtIdentifier),
        ("@@global.`test`", token::doubleAtIdentifier),
        ("@@session.`test`", token::doubleAtIdentifier),
        ("@@local.`test`", token::doubleAtIdentifier),
        ("@@`test`", token::doubleAtIdentifier),
    ] {
        assert_eq!(first_token(input).0, expected, "{input:?}");
    }
}

#[test]
/// 整型/浮点/十六进制/位串/字符串等字面量分类。
fn lexer_matches_all_go_literal_token_cases() {
    let nul_identifier = format!("t1{}", '\0');
    let replacement_identifier = format!("123{}xxx", char::REPLACEMENT_CHARACTER);
    let cases: Vec<(String, i32)> = vec![
        ("'''a'''".into(), token::stringLit),
        ("''a''".into(), token::stringLit),
        ("\"\"a\"\"".into(), token::stringLit),
        (r"\'a\'".into(), b'\\' as i32),
        (r#"\"a\""#.into(), b'\\' as i32),
        ("0.2314".into(), decLit),
        ("1234567890123456789012345678901234567890".into(), decLit),
        ("132.313".into(), decLit),
        ("132.3e231".into(), floatLit),
        ("132.3e-231".into(), floatLit),
        ("001e-12".into(), floatLit),
        ("23416".into(), intLit),
        ("123test".into(), token::identifier),
        (replacement_identifier, token::identifier),
        ("0".into(), intLit),
        ("0x3c26".into(), hexLit),
        ("x'13181C76734725455A'".into(), hexLit),
        ("0b01".into(), bitLit),
        (nul_identifier, token::identifier),
        ("N'some text'".into(), token::underscoreCS),
        ("n'some text'".into(), token::underscoreCS),
        (r"\N".into(), token::null),
        (".*".into(), b'.' as i32),
        (".1_t_1_x".into(), decLit),
        ("9e9e".into(), floatLit),
        (".1e".into(), invalid),
        (".1e23".into(), floatLit),
        (".123".into(), decLit),
        (".1*23".into(), decLit),
        (".1,23".into(), decLit),
        (".1 23".into(), decLit),
        (".1$23".into(), decLit),
        (".1a23".into(), decLit),
        (".1e23$23".into(), floatLit),
        (".1e23a23".into(), floatLit),
        (".1C23".into(), decLit),
        (".1\u{0081}".into(), decLit),
        (".1Ｔ".into(), decLit),
        ("b''".into(), bitLit),
        ("b'0101'".into(), bitLit),
        ("0b0101".into(), bitLit),
    ];
    for (input, expected) in cases {
        assert_eq!(first_token(&input).0, expected, "{input:?}");
    }
}

#[test]
/// 字符串转义与成对引号还原结果。
fn lexer_matches_go_string_escape_cases() {
    let cases = [
        (r"' \n\tTest String'", " \n\tTest String"),
        (r"'\x\B'", "xB"),
        (r#"'\0\'\"\b\n\r\t\\'"#, "\0'\"\u{0008}\n\r\t\\"),
        (r"'\Z'", "\u{001a}"),
        (r"'\%\_'", r"\%\_"),
        ("'hello'", "hello"),
        (r#"'"hello"'"#, r#""hello""#),
        (r#"'""hello""'"#, r#"""hello"""#),
        ("'hel''lo'", "hel'lo"),
        (r"'\'hello'", "'hello"),
        (r#""hello""#, "hello"),
        (r#""'hello'""#, "'hello'"),
        (r#""''hello''""#, "''hello''"),
        (r#""hel""lo""#, "hel\"lo"),
        (r#""\"hello""#, "\"hello"),
        (r"'disappearing\ backslash'", "disappearing backslash"),
        (
            "'한국의中文UTF8およびテキストトラック'",
            "한국의中文UTF8およびテキストトラック",
        ),
    ];
    for (input, expected) in cases {
        let mut scanner = NewScanner(input.to_owned());
        let (token, position, literal) = scanner.scan();
        assert_eq!(position.Offset, 0, "{input:?}");
        assert_eq!(token, token::stringLit, "{input:?}");
        assert_eq!(literal, expected, "{input:?}");
    }
}

#[test]
/// 字符集 introducer、引号标识符与 Go 一样产生连续且带精确值的 token。
fn lexer_matches_go_introducer_and_quoted_identifier_values() {
    let mut scanner = NewScanner(r#"_utf8"string""#.to_owned());
    let mut value = yySymType::default();
    assert_eq!(scanner.Lex(&mut value), token::underscoreCS);
    assert_eq!(value.ident, "utf8");
    assert_eq!(scanner.Lex(&mut value), token::stringLit);
    assert_eq!(value.ident, "string");

    scanner.reset("N'string'".to_owned());
    assert_eq!(scanner.Lex(&mut value), token::underscoreCS);
    assert_eq!(value.ident, "utf8");
    assert_eq!(scanner.Lex(&mut value), token::stringLit);
    assert_eq!(value.ident, "string");

    scanner.reset("`fk`".to_owned());
    let (actual_token, position, literal) = scanner.scan();
    assert_eq!(actual_token, quotedIdentifier);
    assert_eq!(position.Offset, 0);
    assert_eq!(literal, "fk");
}

#[test]
/// 客户端多字节编码下不得把合法 UTF-8 输入切裂。
fn lexer_client_encoding_never_splits_rust_utf8_input() {
    for encoding in ["gbk", "gb18030"] {
        let mut scanner = NewScanner("'啊'".to_owned());
        scanner.client = charset::encoding::FindEncoding(encoding);
        let (actual_token, position, literal) = scanner.scan();
        assert_eq!(position.Offset, 0, "{encoding}");
        assert_eq!(actual_token, token::stringLit, "{encoding}: {literal:?}");
        assert!(
            scanner.r.eof(),
            "{encoding}: scanner did not consume the literal"
        );
    }
}

#[test]
/// 注释、标识符与非法未闭合字面量。
fn lexer_matches_go_comment_identifier_and_illegal_cases() {
    for (input, expected) in [
        ("-- select --\n1", intLit),
        ("/*!40101 SET character_set_client = utf8 */;", token::set),
        ("/* SET character_set_client = utf8 */;", b';' as i32),
        ("/* some comments */ SELECT ", token::selectKwd),
        (
            "-- comment continues to the end of line\nSELECT",
            token::selectKwd,
        ),
        (
            "# comment continues to the end of line\nSELECT",
            token::selectKwd,
        ),
        ("#comment\n123", intLit),
        ("--5", b'-' as i32),
        ("--\nSELECT", token::selectKwd),
        ("--\tSELECT", 0),
        ("--\r\nSELECT", token::selectKwd),
        ("--", 0),
        ("/*T![unsupported] '*/0 -- ' */", intLit),
        ("/*T![auto_rand] '*/0 -- ' */", token::stringLit),
    ] {
        assert_eq!(first_token(input).0, expected, "{input:?}");
    }

    for (input, expected) in [
        ("哈哈", "哈哈"),
        ("`numeric`", "numeric"),
        ("\r\n \r \n \tthere\t \n", "there"),
        ("5number", "5number"),
        ("1_x", "1_x"),
        ("0_x", "0_x"),
        ("9e", "9e"),
        ("0b", "0b"),
        ("0b123", "0b123"),
        ("0b1ab", "0b1ab"),
        ("0B01", "0B01"),
        ("0x", "0x"),
        ("0x7fz3", "0x7fz3"),
        ("023a4", "023a4"),
        ("9eTSs", "9eTSs"),
    ] {
        let (actual, value) = first_token(input);
        assert_eq!(actual, token::identifier, "{input:?}");
        assert_eq!(value.ident, expected, "{input:?}");
    }

    for input in [
        "'",
        "'fu",
        "'\\n",
        "'\\",
        "\0",
        "`",
        "\"",
        "@`",
        "@'",
        "@\"",
        "@@`",
        "@@global.`",
    ] {
        assert_eq!(first_token(input).0, invalid, "{input:?}");
    }
}

#[test]
/// `scanVersionDigits` 与 `scanFeatureIDs` 边界。
fn lexer_matches_go_version_and_feature_scanners() {
    let version_cases = [
        ("12345", 5, 5, 0),
        ("12345xyz", 5, 5, b'x'),
        ("1234xyz", 5, 5, b'1'),
        ("123456", 5, 5, b'6'),
        ("1234", 5, 5, b'1'),
        ("", 5, 5, 0),
        ("1234567xyz", 5, 6, b'7'),
        ("12345xyz", 5, 6, b'x'),
        ("12345", 5, 6, 0),
        ("1234xyz", 5, 6, b'1'),
    ];
    for (input, min, max, expected) in version_cases {
        let mut scanner = NewScanner(input.to_owned());
        scanner.scanVersionDigits(min, max);
        assert_eq!(scanner.r.readByte(), expected, "{input:?}");
    }

    let feature_cases: &[(&str, Option<&[&str]>, u8)] = &[
        ("[feature]", Some(&["feature"]), 0),
        ("[feature] xx", Some(&["feature"]), b' '),
        ("[feature1,feature2]", Some(&["feature1", "feature2"]), 0),
        (
            "[feature1,feature2,feature3]",
            Some(&["feature1", "feature2", "feature3"]),
            0,
        ),
        ("[id_en_ti_fier]", Some(&["id_en_ti_fier"]), 0),
        ("[invalid,    whitespace]", None, b'['),
        ("[unclosed_brac", None, b'['),
        ("unclosed_brac]", None, b'u'),
        ("[invalid_comma,]", None, b'['),
        ("[,]", None, b'['),
        ("[]", None, b'['),
    ];
    for (input, expected, next) in feature_cases {
        let mut scanner = NewScanner((*input).to_owned());
        let actual = scanner.scanFeatureIDs();
        let expected = expected.map(|items| items.iter().map(|item| (*item).to_owned()).collect());
        assert_eq!(actual, expected, "{input:?}");
        assert_eq!(scanner.r.readByte(), *next, "{input:?}");
    }
}

#[test]
/// `LexLiteral` 保留具体数值类型而非一律字符串化。
fn lexer_preserves_go_dynamic_literal_values() {
    for (input, expected) in [
        ("01000001783", 1_000_001_783_i64),
        ("00001783", 1_783),
        ("0", 0),
        ("0000", 0),
        ("01", 1),
        ("10", 10),
    ] {
        let mut scanner = NewScanner(input.to_owned());
        let value = scanner.LexLiteral();
        if let Some(value) = value.downcast_ref::<i64>() {
            assert_eq!(*value, expected, "{input:?}");
        } else {
            assert_eq!(
                *value.downcast_ref::<u64>().expect("integer literal"),
                expected as u64,
                "{input:?}"
            );
        }
    }

    let mut scanner = NewScanner("132.3e231".to_owned());
    assert_eq!(*scanner.LexLiteral().downcast::<f64>().unwrap(), 1.323e233);
    scanner.reset("001e-12".to_owned());
    assert_eq!(*scanner.LexLiteral().downcast::<f64>().unwrap(), 1e-12);
    scanner.reset("0.2314".to_owned());
    assert_eq!(
        scanner
            .LexLiteral()
            .downcast_ref::<parser_test_driver::MyDecimal>()
            .unwrap()
            .String(),
        "0.2314"
    );

    assert_literal_value("'''a'''", "'a'".to_owned());
    assert_literal_value("''a''", String::new());
    assert_literal_value("123test", "123test".to_owned());
    for (input, expected) in [
        ("0x3c26", vec![60_u8, 38]),
        (
            "x'13181C76734725455A'",
            vec![19_u8, 24, 28, 118, 115, 71, 37, 69, 90],
        ),
    ] {
        let mut scanner = NewScanner(input.to_owned());
        assert_eq!(
            scanner
                .LexLiteral()
                .downcast_ref::<parser_test_driver::HexLiteral>()
                .expect("hex literal")
                .0,
            expected,
            "{input:?}"
        );
    }
    for (input, expected) in [("0b01", vec![1_u8]), ("b'0101'", vec![5_u8])] {
        let mut scanner = NewScanner(input.to_owned());
        assert_eq!(
            scanner
                .LexLiteral()
                .downcast_ref::<parser_test_driver::BitLiteral>()
                .expect("bit literal")
                .0,
            expected,
            "{input:?}"
        );
    }
    assert_literal_value(r"\N", r"\N".to_owned());

    let mut scanner = NewScanner(".1_t_1_x".to_owned());
    assert_eq!(
        scanner
            .LexLiteral()
            .downcast_ref::<parser_test_driver::MyDecimal>()
            .unwrap()
            .String(),
        "0.1"
    );
}

#[test]
/// 特殊注释、特性注释和优化器 Hint 保留 Go 的 token 文本与字节位置。
fn lexer_matches_go_comment_and_hint_positions() {
    let mut scanner = NewScanner("/*!40101 select\n5*/".to_owned());
    let (actual_token, position, literal) = scanner.scan();
    assert_eq!(
        (actual_token, literal.as_str()),
        (token::identifier, "select")
    );
    assert_eq!((position.Line, position.Col, position.Offset), (1, 9, 9));
    let (actual_token, position, literal) = scanner.scan();
    assert_eq!((actual_token, literal.as_str()), (intLit, "5"));
    assert_eq!((position.Line, position.Col, position.Offset), (2, 1, 16));

    scanner.reset("/*T![auto_rand] auto_random(5) */".to_owned());
    let (actual_token, position, literal) = scanner.scan();
    assert_eq!(
        (actual_token, literal.as_str()),
        (token::identifier, "auto_random")
    );
    assert_eq!((position.Line, position.Col, position.Offset), (1, 16, 16));
    assert_eq!(scanner.scan().0, b'(' as i32);
    let (_, position, literal) = scanner.scan();
    assert_eq!(literal, "5");
    assert_eq!((position.Line, position.Col, position.Offset), (1, 28, 28));
    assert_eq!(scanner.scan().0, b')' as i32);

    scanner.reset("SELECT /*+ BKA(t1) */ 0;".to_owned());
    let expected = [
        (token::selectKwd, "SELECT", 0),
        (token::hintComment, "/*+ BKA(t1) */", 7),
        (intLit, "0", 22),
        (b';' as i32, ";", 23),
    ];
    let mut value = yySymType::default();
    for (expected_token, expected_ident, expected_offset) in expected {
        assert_eq!(scanner.Lex(&mut value), expected_token);
        assert_eq!(value.ident, expected_ident);
        assert_eq!(value.offset, expected_offset);
    }
}

#[test]
/// ANSI_QUOTES 与 NO_BACKSLASH_ESCAPES 对词法的影响。
fn lexer_matches_go_sql_modes() {
    let ansi_cases = [
        (r#""identifier""#, token::identifier, "identifier"),
        ("`identifier`", token::identifier, "identifier"),
        (r#""identifier""and""#, token::identifier, "identifier\"and"),
        ("'string''string'", token::stringLit, "string'string"),
        (r#""identifier"'and'"#, token::identifier, "identifier"),
        (r#"'string'"identifier""#, token::stringLit, "string"),
    ];
    let mut scanner = NewScanner(String::new());
    scanner.SetSQLMode(mysql::ModeANSIQuotes);
    for (input, expected_token, expected_ident) in ansi_cases {
        scanner.reset(input.to_owned());
        let mut value = yySymType::default();
        assert_eq!(scanner.Lex(&mut value), expected_token, "{input:?}");
        assert_eq!(value.ident, expected_ident, "{input:?}");
    }

    let no_backslash_cases = [
        (r"' \n\tTest String'", r" \n\tTest String"),
        (r"'\x\B'", r"\x\B"),
        (r"'\Z'", r"\Z"),
        (r"'\%\_'", r"\%\_"),
        (r#"'\0\\''"\b\n\r\t\'"#, r#"\0\\'"\b\n\r\t\"#),
        ("'hello'", "hello"),
        (r#"'"hello"'"#, r#""hello""#),
        (r#"'""hello""'"#, r#"""hello"""#),
        ("'hel''lo'", "hel'lo"),
        (r"'\'hello'", r"\"),
        (r#""hello""#, "hello"),
        (r#""'hello'""#, "'hello'"),
        (r#""''hello''""#, "''hello''"),
        (r#""hel""lo""#, "hel\"lo"),
        (r#""\"hello""#, r"\"),
        (
            "'한국의中文UTF8およびテキストトラック'",
            "한국의中文UTF8およびテキストトラック",
        ),
    ];
    scanner.SetSQLMode(mysql::ModeNoBackslashEscapes);
    for (input, expected) in no_backslash_cases {
        scanner.reset(input.to_owned());
        let (actual_token, position, literal) = scanner.scan();
        assert_eq!(position.Offset, 0, "{input:?}");
        assert_eq!(actual_token, token::stringLit, "{input:?}");
        assert_eq!(literal, expected, "{input:?}");
    }

    scanner.SetSQLMode(mysql::ModeANSIQuotes);
    scanner.reset("'string' 'string'".to_owned());
    let mut value = yySymType::default();
    assert_eq!(scanner.Lex(&mut value), token::stringLit);
    assert_eq!(value.ident, "string");
    assert_eq!(scanner.Lex(&mut value), token::stringLit);
    assert_eq!(value.ident, "string");
}

#[test]
/// 优化器 Hint 仅在允许关键字后被识别。
fn lexer_matches_go_optimizer_hint_positions() {
    let cases: &[(&str, &[i32])] = &[
        (
            "SELECT /*+ hint */ *",
            &[token::selectKwd, token::hintComment, b'*' as i32, 0],
        ),
        (
            "UPDATE /*+ hint */",
            &[token::update, token::hintComment, 0],
        ),
        (
            "INSERT /*+ hint */",
            &[token::insert, token::hintComment, 0],
        ),
        (
            "REPLACE /*+ hint */",
            &[token::replace, token::hintComment, 0],
        ),
        (
            "DELETE /*+ hint */",
            &[token::deleteKwd, token::hintComment, 0],
        ),
        (
            "CREATE /*+ hint */",
            &[token::create, token::hintComment, 0],
        ),
        ("/*+ hint */ SELECT *", &[token::selectKwd, b'*' as i32, 0]),
        (
            "SELECT /* comment */ /*+ hint */ *",
            &[token::selectKwd, token::hintComment, b'*' as i32, 0],
        ),
        ("SELECT * /*+ hint */", &[token::selectKwd, b'*' as i32, 0]),
        (
            "SELECT /*T![auto_rand] * */ /*+ hint */",
            &[token::selectKwd, b'*' as i32, 0],
        ),
        (
            "SELECT /*T![unsupported] * */ /*+ hint */",
            &[token::selectKwd, token::hintComment, 0],
        ),
        (
            "SELECT /*+ hint1 */ /*+ hint2 */ *",
            &[token::selectKwd, token::hintComment, b'*' as i32, 0],
        ),
        (
            "SELECT * FROM /*+ hint */",
            &[token::selectKwd, b'*' as i32, token::from, 0],
        ),
        ("`SELECT` /*+ hint */", &[token::identifier, 0]),
        ("'SELECT' /*+ hint */", &[token::stringLit, 0]),
    ];
    for (input, expected) in cases {
        let mut scanner = NewScanner((*input).to_owned());
        let mut value = yySymType::default();
        let actual: Vec<i32> = std::iter::from_fn(|| {
            let token = scanner.Lex(&mut value);
            Some(token).filter(|_| token != 0)
        })
        .chain(std::iter::once(0))
        .collect();
        assert_eq!(&actual, expected, "{input:?}");
    }
}
