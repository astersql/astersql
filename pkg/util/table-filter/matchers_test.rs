// Copyright 2026 AsterSQL.

use crate::{matcher, stringMatcher};

#[test]
fn string_matcher_lowercase_uses_go_simple_case_mapping() {
    let matcher = stringMatcher("\u{130}".to_owned()).toLower();

    assert!(matcher.matchString("i"));
    assert!(!matcher.matchString("i\u{307}"));
}

#[test]
fn regexp_errors_match_go_diagnostics() {
    for (pattern, expected) in [
        ("(?s)^[^]$", "missing closing ]: `[^]$`"),
        ("(?=x)", "invalid or unsupported Perl syntax: `(?=`"),
        ("(?!x)", "invalid or unsupported Perl syntax: `(?!`"),
        ("(?<=x)", "invalid named capture: `(?<=x)`"),
        ("(?<!x)", "invalid named capture: `(?<!x)`"),
        ("a[", "missing closing ]: `[`"),
        ("(abc", "missing closing ): `(abc`"),
        ("abc)", "unexpected ): `abc)`"),
        ("*a", "missing argument to repetition operator: `*`"),
        ("[z-a]", "invalid character class range: `z-a`"),
        (r"\q", r"invalid escape sequence: `\q`"),
        ("abc\\", "trailing backslash at end of expression: ``"),
        ("a{3,2}", "invalid repeat count: `{3,2}`"),
    ] {
        let error = crate::newRegexpMatcher(pattern).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("error parsing regexp: {expected}"),
            "{pattern}"
        );
    }
}
