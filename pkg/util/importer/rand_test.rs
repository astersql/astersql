// Copyright 2026 AsterSQL.

use super::rand::{rand_timestamp, rand_year, seed};

#[test]
fn timestamp_without_maximum_preserves_minimum_clock_like_go() {
    seed(1);

    for _ in 0..32 {
        let value = rand_timestamp("2024-02-29 12:34:56", "").unwrap();
        assert_eq!(&value[11..], "12:34:56");
    }
}

#[test]
fn bounded_year_samples_elapsed_seconds_like_go() {
    // Other tests seed and consume the process-wide RNG. Run this exact-output
    // assertion in its own process so those calls cannot change its sequence.
    const CHILD: &str = "ASTERSQL_IMPORTER_BOUNDED_YEAR_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "rand_test::bounded_year_samples_elapsed_seconds_like_go",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("run bounded year regression in an isolated process");
        assert!(
            output.status.success(),
            "isolated year regression failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }

    seed(1);

    assert_eq!(rand_year("2020", "2022").unwrap(), "2020");
}
