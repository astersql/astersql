// Copyright 2026 AsterSQL.

use crate::pg_jsonb::PgJsonb;

fn canonical(input: &str, expected: &str) {
    let value: PgJsonb = input.parse().unwrap();
    assert_eq!(value.as_str(), expected, "{input:?}");
    assert_eq!(value.as_bytes(), expected.as_bytes());
    assert_eq!(value.to_string(), expected);
    assert_eq!(PgJsonb::from_bytes(input.as_bytes()).unwrap(), value);
    assert_eq!(expected.parse::<PgJsonb>().unwrap(), value);
}

#[test]
fn pg05_jsonb_object_decoded_duplicates_and_utf8_order() {
    canonical(
        r#" {"zz":0,"a":1,"\u0061":2,"é":3,"b":4,"中":5,"aa":6,"z":7,"":8} "#,
        r#"{"": 8, "a": 2, "b": 4, "z": 7, "aa": 6, "zz": 0, "é": 3, "中": 5}"#,
    );
    canonical(
        r#"{"x":{"b":1,"b":2,"a":0},"x":{"q":3,"q":4},"a":[{"z":1,"z":2},2,2]}"#,
        r#"{"a": [{"z": 2}, 2, 2], "x": {"q": 4}}"#,
    );
    canonical(r#"{"a":0,"\u0061":null}"#, r#"{"a": null}"#);
}

#[test]
fn pg05_jsonb_object_strings_surrogates_and_escaping() {
    canonical(
        r#""\uD83D\uDE00\u4e2d\/\b\f\n\r\t\"\\""#,
        "\"😀中/\\b\\f\\n\\r\\t\\\"\\\\\"",
    );
    canonical(r#"{"\uD83D\uDE00":1,"😀":2}"#, r#"{"😀": 2}"#);
    canonical(r#""\u0001\u001f\u007f""#, "\"\\u0001\\u001f\u{7f}\"");
    canonical(r#""\\u0000""#, r#""\\u0000""#);
}

#[test]
fn pg05_jsonb_object_roots_arrays_and_external_null() {
    for (input, expected) in [
        (" null ", "null"),
        ("true", "true"),
        ("false", "false"),
        (r#""null""#, r#""null""#),
        (" {} ", "{}"),
        (" [] ", "[]"),
        (
            " [1,1,true,false,null,[],{}] ",
            "[1, 1, true, false, null, [], {}]",
        ),
        ("9007199254740993", "9007199254740993"),
        ("1.00", "1.00"),
    ] {
        canonical(input, expected);
    }
    assert_ne!(None::<PgJsonb>, Some("null".parse().unwrap()));
    assert_ne!(
        "null".parse::<PgJsonb>().unwrap(),
        r#""null""#.parse().unwrap()
    );
    assert_ne!("1".parse::<PgJsonb>().unwrap(), "[1]".parse().unwrap());
}

#[test]
fn pg05_jsonb_object_unicode_errors_and_recovery() {
    for (input, state) in [
        (r#""\u0000""#, "22P05"),
        (r#""\ud800""#, "22P02"),
        (r#""\udfff""#, "22P02"),
        (r#""\ud800x""#, "22P02"),
        (r#""\ud800\u0041""#, "22P02"),
        (r#""\ud800\ud800""#, "22P02"),
        (r#"{"\u0000":1}"#, "22P05"),
        (r#"{"a":"\udc00"}"#, "22P02"),
        (r#"{"a":"\ud800","a":"ok"}"#, "22P02"),
    ] {
        assert_eq!(
            input.parse::<PgJsonb>().unwrap_err().sqlstate(),
            state,
            "{input}"
        );
        canonical("null", "null");
    }
}

#[test]
fn pg05_jsonb_object_syntax_and_encoding_errors() {
    for input in [
        "",
        " ",
        "01",
        "+1",
        "1.",
        "NaN",
        "Infinity",
        "[1,]",
        "{a:1}",
        "{}{}",
        "[",
        "{\"a\":}",
        r#""\x""#,
        r#""\u12xz""#,
        r#""\u123""#,
        "\"\n\"",
        "truefalse",
    ] {
        assert_eq!(
            input.parse::<PgJsonb>().unwrap_err().sqlstate(),
            "22P02",
            "{input:?}"
        );
    }
    for bytes in [
        &b"\xff"[..],
        &b"\"\xc0\xaf\""[..],
        &b"null\0"[..],
        &b"\"\0\""[..],
    ] {
        assert_eq!(PgJsonb::from_bytes(bytes).unwrap_err().sqlstate(), "22021");
    }
}

#[test]
fn pg05_jsonb_object_deep_containers_and_wide_duplicates() {
    let input = format!("{}null{}", "[".repeat(4096), "]".repeat(4096));
    canonical(&input, &input);
    let members = (0..1000)
        .map(|i| format!("\"a\":{i}"))
        .collect::<Vec<_>>()
        .join(",");
    canonical(&format!("{{{members}}}"), r#"{"a": 999}"#);
}

#[test]
fn pg05_jsonb_object_all_bmp_escapes_and_surrogate_boundaries() {
    for code in 0..=u16::MAX {
        let input = format!("\"\\u{code:04x}\"");
        if code == 0 || (0xd800..=0xdfff).contains(&code) {
            assert_eq!(
                input.parse::<PgJsonb>().unwrap_err().sqlstate(),
                if code == 0 { "22P05" } else { "22P02" }
            );
        } else {
            let character = char::from_u32(u32::from(code)).unwrap();
            let expected = serde_json::to_string(&character.to_string()).unwrap();
            canonical(&input, &expected);
        }
    }
    for (input, expected) in [
        (r#""\ud800\udc00""#, "\"\u{10000}\""),
        (r#""\udbff\udfff""#, "\"\u{10ffff}\""),
        (
            r#"{"é":0,"e\u0301":1,"é":2}"#,
            "{\"é\": 2, \"e\u{301}\": 1}",
        ),
    ] {
        canonical(input, expected);
    }
}

#[test]
fn pg05_jsonb_object_container_cross_product_and_identity() {
    let roots = [
        "{}", "[]", "null", "true", "false", "\"a\"", "0", "-0", "1.0e+2",
    ];
    for left in roots {
        for right in roots {
            let a = left.parse::<PgJsonb>().unwrap();
            let b = right.parse::<PgJsonb>().unwrap();
            canonical(&format!("[{left},{right}]"), &format!("[{a}, {b}]"));
            canonical(
                &format!("{{\"a\":{left},\"a\":{right}}}"),
                &format!("{{\"a\": {b}}}"),
            );
            canonical(
                &format!("[{{\"a\":{left}}},[{right}]]"),
                &format!("[{{\"a\": {a}}}, [{b}]]"),
            );
        }
    }
    assert_eq!(PgJsonb::OID, 3802);
    let raw: crate::pg_json::PgJson = " {} ".parse().unwrap();
    let normalized: PgJsonb = " {} ".parse().unwrap();
    assert_ne!(raw.as_str(), normalized.as_str());
    // Errors never echo user inputs, including their token values.
    let input = "secret-token-not-json";
    assert!(
        !input
            .parse::<PgJsonb>()
            .unwrap_err()
            .to_string()
            .contains(input)
    );
}

#[test]
fn pg05_jsonb_number_exact_precision_and_display_scale() {
    for (input, expected) in [
        ("18446744073709551617", "18446744073709551617"),
        ("-18446744073709551617", "-18446744073709551617"),
        (
            "123456789012345678901234567890.123456789012345678901234567890",
            "123456789012345678901234567890.123456789012345678901234567890",
        ),
        ("1.230e-5", "0.00001230"),
        ("1.00", "1.00"),
        ("1.230e2", "123.0"),
        ("1.230e3", "1230"),
        ("1.230E+4", "12300"),
        ("0.00000000000000000000100", "0.00000000000000000000100"),
        (
            "100000000000000000000.000000000000000000001",
            "100000000000000000000.000000000000000000001",
        ),
    ] {
        canonical(input, expected);
        canonical(&format!("[{input},null]"), &format!("[{expected}, null]"));
        canonical(
            &format!(r#"{{"n":{input}}}"#),
            &format!(r#"{{"n": {expected}}}"#),
        );
    }
    assert_ne!("1".parse::<PgJsonb>().unwrap(), "1.00".parse().unwrap());
    assert_ne!(None::<PgJsonb>, Some("null".parse().unwrap()));
    assert_eq!(PgJsonb::OID, 3802);
}

#[test]
fn pg05_jsonb_number_negative_zero_preserves_scale() {
    for (input, expected) in [
        ("-0", "0"),
        ("-0.00", "0.00"),
        ("-0.000e2", "0.0"),
        ("-0.00e-3", "0.00000"),
        ("-0e10", "0"),
        ("-0.000e3", "0"),
    ] {
        canonical(input, expected);
    }
}

#[test]
fn pg05_jsonb_number_numeric_domain_boundaries() {
    let largest_integer = "9".repeat(131072);
    let smallest_fraction = format!("0.{}1", "0".repeat(16382));
    for input in [
        largest_integer.clone(),
        format!("-{largest_integer}"),
        smallest_fraction.clone(),
        format!("-{smallest_fraction}"),
        format!("{largest_integer}.{fraction}", fraction = "9".repeat(16383)),
        format!("0.{}", "0".repeat(16383)),
    ] {
        canonical(&input, &input);
    }
    canonical("1e131071", &format!("1{}", "0".repeat(131071)));
    canonical("1e-16383", &smallest_fraction);
    canonical("-0e-16383", &format!("0.{}", "0".repeat(16383)));
    for input in [
        "1e200000".to_owned(),
        "1e131072".to_owned(),
        "1e-16384".to_owned(),
        "-1e200000".to_owned(),
        "9".repeat(131073),
        format!("0.{}1", "0".repeat(16383)),
        "1e9223372036854775807".to_owned(),
        "1e-9223372036854775808".to_owned(),
        "1e999999999999999999999999999999".to_owned(),
    ] {
        for document in [
            input.clone(),
            format!("[{input}]"),
            format!(r#"{{"n":{input}}}"#),
        ] {
            assert_eq!(document.parse::<PgJsonb>().unwrap_err().sqlstate(), "22003");
            canonical("1.00", "1.00");
        }
    }
}

#[test]
fn pg05_jsonb_number_invalid_tokens_and_recovery() {
    for input in [
        "NaN",
        "nan",
        "Infinity",
        "-Infinity",
        "+Infinity",
        "Inf",
        "-NaN",
        "+1",
        "01",
        "-01",
        ".1",
        "1.",
        "1e",
        "1e+",
        "1e-",
        "1e1.0",
        "--1",
        "1_000",
        "0x1",
        "1 2",
        "1.2.3",
    ] {
        for document in [
            input.to_owned(),
            format!("[{input}]"),
            format!(r#"{{"n":{input}}}"#),
        ] {
            let error = document.parse::<PgJsonb>().unwrap_err();
            assert_eq!(error.sqlstate(), "22P02", "{document}");
            assert_eq!(error, crate::pg_jsonb::PgJsonbError::InvalidSyntax);
            canonical("18446744073709551617", "18446744073709551617");
        }
    }
    // Replaced object members must still validate every numeric value.
    for (input, state) in [
        (r#"{"n":1e200000,"n":1}"#, "22003"),
        (r#"{"n":NaN,"n":1}"#, "22P02"),
    ] {
        assert_eq!(input.parse::<PgJsonb>().unwrap_err().sqlstate(), state);
        canonical(r#"{"n":1.230e-5}"#, r#"{"n": 0.00001230}"#);
    }
}

#[test]
fn pg05_jsonb_sql_surrogate_conversion_state() {
    for input in [r#""\ud800""#, r#""\udfff""#, r#"{"a":"\ud800","a":"ok"}"#] {
        assert!(input.parse::<crate::pg_json::PgJson>().is_ok());
        assert_eq!(input.parse::<PgJsonb>().unwrap_err().sqlstate(), "22P02");
    }
}

#[test]
fn pg05_jsonb_input_surrogate_diagnostics() {
    for input in [r#""\ud800""#, r#""\udfff""#, r#"{"a":"\ud800","a":"ok"}"#] {
        assert_eq!(input.parse::<PgJsonb>().unwrap_err().sqlstate(), "22P02");
    }
    assert_eq!(
        r#""\u0000""#.parse::<PgJsonb>().unwrap_err().sqlstate(),
        "22P05"
    );
}
