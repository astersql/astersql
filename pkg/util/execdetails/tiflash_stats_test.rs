// Copyright 2026 AsterSQL.

use execdetails_ruv2::{TiFlashNetworkTrafficSummary, TiFlashScanContext};
use std::collections::HashMap;

#[test]
fn region_balance_uses_go_fixed_point_format() {
    let stats = TiFlashScanContext {
        regionsOfInstance: HashMap::from([("a".to_string(), 4), ("b".to_string(), 9)]),
        ..Default::default()
    };

    assert!(
        stats
            .String()
            .contains("region_balance:{instance_num: 2, max/min: 9/4=2.250000}")
    );
}

#[test]
fn network_string_matches_go_int64_conversion() {
    let stats = TiFlashNetworkTrafficSummary {
        innerZoneSendBytes: u64::MAX,
        ..Default::default()
    };

    assert_eq!(
        stats.String(),
        "tiflash_network: {inner_zone_send_bytes: -1}"
    );
}
