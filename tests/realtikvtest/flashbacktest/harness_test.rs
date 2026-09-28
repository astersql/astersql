// Copyright 2026 AsterSQL.

use std::time::{Duration, UNIX_EPOCH};

use crate::harness::{oracle, tikvutil};

#[test]
fn time_formats_match_go_utc_contract() {
    assert_eq!(
        oracle::format_fsp(UNIX_EPOCH + Duration::from_micros(123_456)),
        "1970-01-01 00:00:00.123456"
    );
    assert_eq!(tikvutil::format_gc(UNIX_EPOCH), "19700101-00:00:00 +0000");
}

#[test]
fn fsp_parser_uses_unix_epoch_and_preserves_milliseconds() {
    let ts = oracle::parse_fsp("1970-01-02 03:04:05.123456").unwrap();
    assert_eq!(ts >> 18, 97_445_123);
    assert_eq!(
        oracle::format_fsp(oracle::GetTimeFromTS(ts)),
        "1970-01-02 03:04:05.123000"
    );
}
