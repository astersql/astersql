// Copyright 2026 AsterSQL.

use crate::{NewChunkParser, NewStringReader, Token};

fn lex_once(input: &str) -> (Token, Vec<u8>) {
    let mut parser = NewChunkParser(Box::new(NewStringReader(input)), 1024, None, true);
    crate::parser_generated::lex(&mut parser).unwrap()
}

#[test]
fn longest_unquoted_match_wins_over_keyword_and_integer_prefixes() {
    for input in [
        "valuesx",
        "nullary",
        "trueish",
        "falsehood",
        "123abc",
        "-12e3",
    ] {
        let (token, raw) = lex_once(input);
        assert_eq!(token, Token::Unquoted, "input: {input}");
        assert_eq!(raw, input.as_bytes(), "input: {input}");
    }
}

#[test]
fn longest_unquoted_match_wins_over_based_literal_prefixes() {
    for input in ["0x12g", "0b012"] {
        let (token, raw) = lex_once(input);
        assert_eq!(token, Token::Unquoted, "input: {input}");
        assert_eq!(raw, input.as_bytes(), "input: {input}");
    }
}

#[test]
fn equal_length_specialized_rules_keep_ragel_priority() {
    for (input, expected) in [
        ("values", Token::Values),
        ("null", Token::Null),
        ("123", Token::Integer),
        ("0x12", Token::HexString),
        ("0b01", Token::BinString),
    ] {
        let (token, raw) = lex_once(input);
        assert_eq!(token, expected, "input: {input}");
        assert_eq!(raw, input.as_bytes(), "input: {input}");
    }
}
