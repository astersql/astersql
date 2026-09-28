// Copyright 2026 AsterSQL.

use super::long_from_args;

#[test]
fn long_stops_at_the_first_positional_argument() {
    assert!(!long_from_args(["test-binary", "case-name", "-long"]));
}

#[test]
fn long_stops_at_the_double_dash_terminator() {
    assert!(!long_from_args(["test-binary", "--", "-long"]));
}

#[test]
fn long_stops_after_an_unknown_flag() {
    assert!(!long_from_args(["test-binary", "--unknown", "-long"]));
}

#[test]
fn invalid_boolean_resets_long_and_stops_parsing() {
    assert!(!long_from_args([
        "test-binary",
        "-long",
        "--long=invalid",
        "-long",
    ]));
}
