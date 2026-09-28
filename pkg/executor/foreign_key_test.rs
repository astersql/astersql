// Copyright 2026 AsterSQL.

use std::time::Duration;

use super::foreign_key::{FKCascadeRuntimeStats, FKCheckRuntimeStats};

#[test]
fn foreign_key_runtime_stats_match_go_duration_precision() {
    let check = FKCheckRuntimeStats {
        Total: Duration::from_nanos(9_412_345),
        Check: Duration::from_nanos(10_412_345),
        Lock: Duration::from_nanos(100_450),
        Keys: 3,
    };
    assert_eq!(
        "total:9.41ms, check:10.4ms, lock:100.5µs, foreign_keys:3",
        check.String()
    );

    let cascade = FKCascadeRuntimeStats {
        Total: Duration::from_millis(5_999),
        Keys: 1,
    };
    assert_eq!("total:6s, foreign_keys:1", cascade.String());
}
