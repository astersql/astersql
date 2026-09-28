// Copyright 2026 AsterSQL.

//! `advancer_env.go` parity tests kept separate from production code.

use std::time::Duration;

use crate::advancer_env::{
    GetLogBackupFlushIntervalFromTiKVConfig, parseLogBackupFlushIntervalFromConfig,
};

fn config(duration: &str) -> Vec<u8> {
    format!(r#"{{"log-backup":{{"max-flush-interval":"{duration}"}}}}"#).into_bytes()
}

#[test]
fn parses_go_duration_grammar_used_by_tikv_config() {
    assert_eq!(
        parseLogBackupFlushIntervalFromConfig(&config("1h30m0.5s")).unwrap(),
        Duration::from_secs_f64(5_400.5)
    );
    assert_eq!(
        parseLogBackupFlushIntervalFromConfig(&config("1.25ms")).unwrap(),
        Duration::from_micros(1_250)
    );
    assert_eq!(
        parseLogBackupFlushIntervalFromConfig(&config("2μs")).unwrap(),
        Duration::from_micros(2)
    );
}

#[test]
fn rejects_values_rejected_by_go_config_duration() {
    for value in [" 3s", "3s ", "0", "0s", "-1s", "9223372036854775808ns"] {
        assert!(
            parseLogBackupFlushIntervalFromConfig(&config(value)).is_err(),
            "unexpectedly accepted {value:?}"
        );
    }
}

#[test]
fn aggregates_compound_intervals_using_the_maximum() {
    let configs = vec![config("1m30s"), config("2m0.25s"), config("45s")];
    assert_eq!(
        GetLogBackupFlushIntervalFromTiKVConfig(&configs).unwrap(),
        Duration::from_millis(120_250)
    );
}
