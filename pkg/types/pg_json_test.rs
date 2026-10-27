// Copyright 2026 AsterSQL.

use crate::pg_json::{PgJson, PgJsonError};

#[test]
fn pg_json_test_preserves_every_root_and_original_bytes() {
    for input in [
        " \t\r\n{\"z\":1.00,\"a\":2,\"z\":3}\n ",
        "[]",
        "{}",
        "[true,false,null,{\"中\":[-0,1E+09]}]",
        "9007199254740993",
        "18446744073709551616",
        "1e1000000",
        "-0.000e-1000000",
        "true",
        "false",
        "null",
        "\"中😀\"",
        r#""\u0000\uD800\uDC00\/\b\f\n\r\t\"\\""#,
    ] {
        let value: PgJson = input.parse().unwrap_or_else(|e| panic!("{input:?}: {e}"));
        assert_eq!(value.as_str(), input);
        assert_eq!(value.as_bytes(), input.as_bytes());
        assert_eq!(value.to_string(), input);
        assert_eq!(PgJson::from_bytes(input.as_bytes()).unwrap(), value);
    }
}

#[test]
fn pg_json_test_unicode_escapes_remain_lexical() {
    for input in [
        r#""\uD800""#,
        r#""\udfff""#,
        r#""\u0000""#,
        r#"{"\uD800":"\uDC00"}"#,
        r#""\uD800x\u0000\uD800""#,
    ] {
        assert_eq!(input.parse::<PgJson>().unwrap().as_str(), input);
    }
}

#[test]
fn pg_json_test_invalid_syntax_and_recovery() {
    for input in [
        "",
        " \n\t",
        "01",
        "-01",
        "+1",
        ".1",
        "1.",
        "1e",
        "1e+",
        "-",
        "NaN",
        "Infinity",
        "TRUE",
        "undefined",
        "null null",
        "truefalse",
        "[]{}",
        "[1,]",
        "[,1]",
        "[1 2]",
        "{\"a\":1,}",
        "{a:1}",
        "{\"a\" 1}",
        "{\"a\":}",
        "[",
        "{",
        "[}",
        "{]",
        "/*x*/null",
        "null//x",
        "\u{feff}null",
        "\u{a0}null",
        "\"x\nx\"",
        "\"\u{1f}\"",
        "\"unterminated",
        r#""\x""#,
        r#""\u123""#,
        r#""\u12xz""#,
        "\"x\\",
        "\"x\"tail",
    ] {
        let err = input.parse::<PgJson>().expect_err(input);
        assert_eq!(err, PgJsonError::InvalidSyntax, "{input:?}");
        assert_eq!(err.sqlstate(), "22P02");
        assert_eq!(PgJson::from_bytes(input.as_bytes()).unwrap_err(), err);
        assert_eq!("null".parse::<PgJson>().unwrap().as_str(), "null");
    }
}

#[test]
fn pg_json_test_encoding_errors_are_separate_and_redacted() {
    for input in [
        &b"\xff"[..],
        &b"\"\xc0\xaf\""[..],
        &b"null\0"[..],
        &b"\"\0\""[..],
    ] {
        let err = PgJson::from_bytes(input).unwrap_err();
        assert_eq!(err, PgJsonError::InvalidEncoding);
        assert_eq!(err.sqlstate(), "22021");
    }
    assert_eq!("\0".parse::<PgJson>().unwrap_err().sqlstate(), "22021");
    let secret = "secret-token-not-json";
    assert!(
        !PgJson::from_bytes(secret.as_bytes())
            .unwrap_err()
            .to_string()
            .contains(secret)
    );
}

#[test]
fn pg_json_test_sql_null_is_external_and_spelling_is_identity() {
    let sql_null: Option<PgJson> = None;
    let json_null = Some("null".parse::<PgJson>().unwrap());
    assert_ne!(sql_null, json_null);
    assert_ne!(
        "1".parse::<PgJson>().unwrap(),
        "1.0".parse::<PgJson>().unwrap()
    );
    assert_ne!(
        "{}".parse::<PgJson>().unwrap(),
        " {} ".parse::<PgJson>().unwrap()
    );
}

#[test]
fn pg_json_test_deep_containers_and_long_lexemes() {
    let input = format!("{}0{}", "[".repeat(4096), "]".repeat(4096));
    assert_eq!(input.parse::<PgJson>().unwrap().as_str(), input);
    let number = format!("1{}e+{}", "0".repeat(10000), "9".repeat(10000));
    assert_eq!(number.parse::<PgJson>().unwrap().as_str(), number);
}

#[test]
fn pg_json_test_all_unicode_escapes_and_raw_control_bytes() {
    for code in 0..=u16::MAX {
        let input = format!("\"\\u{code:04x}\"");
        assert_eq!(input.parse::<PgJson>().unwrap().as_str(), input);
    }
    for byte in 0..=0x1f_u8 {
        let input = [b'"', byte, b'"'];
        assert_eq!(
            PgJson::from_bytes(&input).unwrap_err().sqlstate(),
            if byte == 0 { "22021" } else { "22P02" }
        );
    }
    for byte in 0..=127_u8 {
        let input = [b'"', b'\\', byte, b'"'];
        let expected = matches!(byte, b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't');
        assert_eq!(
            PgJson::from_bytes(&input).is_ok(),
            expected,
            "escape {byte}"
        );
    }
}

#[test]
fn pg_json_test_container_continuations_and_numeric_boundaries() {
    let roots = [
        "{}", "[]", "null", "true", "false", "\"a\"", "0", "-0", "1.0e+2",
    ];
    for left in roots {
        for right in roots {
            for input in [
                format!("[{left},{right}]"),
                format!("{{\"a\":{left},\"a\":{right}}}"),
                format!("[{{\"a\":{left}}},[{right}]]"),
            ] {
                assert_eq!(input.parse::<PgJson>().unwrap().as_str(), input);
                for end in 0..input.len() {
                    assert!(
                        input[..end].parse::<PgJson>().is_err(),
                        "prefix: {:?}",
                        &input[..end]
                    );
                }
            }
        }
    }
    for input in [
        "00.1",
        "-00",
        "1.e2",
        "1e+-2",
        "1e--2",
        "1e2.0",
        "--1",
        "1_0",
        "[1e,2]",
        "{\"x\":01}",
        "{\"x\":true false}",
        "[true,]",
        "{\"x\":1,,\"y\":2}",
    ] {
        assert_eq!(
            input.parse::<PgJson>().unwrap_err().sqlstate(),
            "22P02",
            "{input}"
        );
    }
}
