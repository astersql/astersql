// Copyright 2026 AsterSQL.

use super::config::ParseDuration;
use std::time::Duration;

#[test]
fn parse_duration_accepts_go_microsecond_units() {
    assert_eq!(ParseDuration("1µs"), Duration::from_micros(1));
    assert_eq!(ParseDuration("1μs"), Duration::from_micros(1));
    assert_eq!(ParseDuration("1µ"), Duration::from_micros(1));
    assert_eq!(ParseDuration("1μ"), Duration::from_micros(1));
}

#[test]
fn parse_duration_ignores_fraction_digits_beyond_go_precision() {
    assert_eq!(
        ParseDuration("0.123456789012345678901s"),
        Duration::from_nanos(123_456_789)
    );
}

#[test]
fn parse_duration_matches_go_unsigned_accumulation() {
    assert_eq!(
        ParseDuration("9223372036854775808ns9223372036854775808ns"),
        Duration::ZERO
    );
}
