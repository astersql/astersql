// Copyright 2026 AsterSQL.

use crate::statistics::RegionStats;

/// Go `int` is 64 bits on the supported server platforms, so PD statistics
/// must not reject values outside the Rust `i32` range.
#[test]
fn region_statistics_preserve_go_int_range() {
    let beyond_i32 = i64::from(i32::MAX) + 1;
    let stats: RegionStats = serde_json::from_value(serde_json::json!({
        "count": beyond_i32,
        "empty_count": beyond_i32,
        "store_leader_count": {"42": beyond_i32},
        "store_peer_count": {"42": beyond_i32}
    }))
    .expect("Go int statistics in the supported 64-bit range must deserialize");

    assert_eq!(i64::from(stats.Count), beyond_i32);
    assert_eq!(i64::from(stats.EmptyCount), beyond_i32);
    assert_eq!(i64::from(stats.StoreLeaderCount[&42]), beyond_i32);
    assert_eq!(i64::from(stats.StorePeerCount[&42]), beyond_i32);
}
