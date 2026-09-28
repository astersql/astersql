// Copyright 2026 AsterSQL.

use std::time::Duration;

use crate::locker::Config;

#[test]
fn go_flag_duration_and_double_dash_syntax_are_supported() {
    let cfg = Config::parse_args([
        "br_key_locked",
        "--tidb=127.0.0.1:4000",
        "--pd=127.0.0.1:2379",
        "--db=test",
        "--table=t",
        "--run-timeout=1m30.5s",
        "--lock-ttl=250us",
    ])
    .unwrap();

    assert_eq!(cfg.timeout, Duration::from_millis(90_500));
    assert_eq!(cfg.lock_ttl, Duration::from_micros(250));
}
