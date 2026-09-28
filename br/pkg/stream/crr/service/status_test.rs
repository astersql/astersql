// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::status::{StatusSnapshot, StatusStatistic, encode_status_snapshot};

#[test]
fn status_json_matches_go_for_pre_epoch_time_and_sorted_map_keys() {
    let snapshot = StatusSnapshot {
        SyncedByStore: HashMap::from([(10, 10), (2, 2), (1, 1)]),
        Statistic: StatusStatistic {
            PlannedFileSuffixCounts: HashMap::from([
                ("z.log".to_string(), 3),
                ("a.log".to_string(), 1),
                ("m.log".to_string(), 2),
            ]),
            ..Default::default()
        },
        LastSuccessTime: Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
        ..Default::default()
    };

    let encoded = encode_status_snapshot(&snapshot).expect("valid Go-compatible JSON");
    // encoding/json converts integer keys to strings, then sorts lexicographically.
    assert!(encoded.contains("\"synced_by_store\":{\"1\":1,\"10\":10,\"2\":2}"));
    assert!(
        encoded.contains("\"planned_file_suffix_counts\":{\"a.log\":1,\"m.log\":2,\"z.log\":3}")
    );
    assert!(encoded.contains("\"last_success_time\":\"1969-12-31T23:59:59Z\""));
}

#[test]
fn status_json_rejects_times_outside_go_rfc3339_year_range() {
    let snapshot = StatusSnapshot {
        LastSuccessTime: Some(SystemTime::UNIX_EPOCH - Duration::from_secs(62_167_219_201)),
        ..Default::default()
    };

    assert!(encode_status_snapshot(&snapshot).is_err());
}
