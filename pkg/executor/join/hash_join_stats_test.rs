// Copyright 2026 AsterSQL.

use crate::hash_join_stats::{
    HashJoinRuntimeStats, HashJoinRuntimeStatsV2, HashStatistic, SpillStats, write_bytes_stats,
    write_spilled_partition_num_stats,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

#[test]
fn v1_format_matches_go_runtime_stats() {
    let stats = HashJoinRuntimeStats {
        fetch_and_build: Duration::from_secs(2),
        build_hash_table: Duration::from_millis(100),
        fetch_and_probe: Duration::from_secs(5),
        probe: Duration::from_secs(4),
        probe_collision: 1,
        concurrency: 4,
        max_fetch_and_probe_ns: AtomicI64::new(2_000_000_000),
    };
    assert_eq!(
        stats.to_string(),
        "build_hash_table:{total:2s, fetch:1.9s, build:100ms}, probe:{concurrency:4, total:5s, max:2s, probe:4s, fetch and wait:1s, probe_collision:1}"
    );
}

#[test]
fn hash_statistic_format_matches_go() {
    let stats = HashStatistic {
        probe_collision: 3,
        build_table_elapsed: Duration::from_millis(100),
    };
    assert_eq!(stats.to_string(), "probe_collision:3, build:100ms");
}

#[test]
fn spill_format_matches_go_ratios_and_gib() {
    let mut formatted = String::new();
    write_spilled_partition_num_stats(&mut formatted, 4, &[2, 3]);
    assert_eq!(formatted, "[2/4 3/8]");
    formatted.clear();
    write_bytes_stats(&mut formatted, &[1_073_741_824, 536_870_912]);
    assert_eq!(formatted, "[1.00 0.50]");
}

#[test]
fn v2_clone_and_merge_match_go_selected_fields() {
    let mut left = HashJoinRuntimeStatsV2 {
        concurrency: 2,
        worker_fetch_and_probe: Duration::from_secs(7),
        spill: SpillStats {
            round: 1,
            partition_num: 4,
            total_spill_bytes_per_round: vec![1_073_741_824],
            ..Default::default()
        },
        is_hash_join_ga: true,
        ..Default::default()
    };
    let cloned = left.clone();
    assert_eq!(cloned.worker_fetch_and_probe, Duration::ZERO);
    assert_eq!(cloned.spill, SpillStats::default());
    assert!(!cloned.is_hash_join_ga);

    let right = HashJoinRuntimeStatsV2 {
        concurrency: 8,
        worker_fetch_and_probe: Duration::from_secs(3),
        spill: SpillStats {
            round: 2,
            ..Default::default()
        },
        max_worker_fetch_and_probe_ns: AtomicI64::new(20),
        ..Default::default()
    };
    left.merge(&right);
    assert_eq!(left.concurrency, 2);
    assert_eq!(left.worker_fetch_and_probe, Duration::from_secs(7));
    assert_eq!(left.spill.round, 1);
    assert_eq!(
        left.max_worker_fetch_and_probe_ns.load(Ordering::Acquire),
        20
    );
}

#[test]
fn v2_reset_round_and_format_match_go() {
    let mut stats = HashJoinRuntimeStatsV2 {
        concurrency: 4,
        fetch_and_build: Duration::from_secs(2),
        build_hash_table: Duration::from_millis(400),
        partition_data: Duration::from_millis(300),
        max_build_hash_table: Duration::from_millis(100),
        max_partition_data: Duration::from_millis(200),
        fetch_and_probe: Duration::from_secs(5),
        probe: Duration::from_secs(4),
        max_probe: Duration::from_secs(3),
        worker_fetch_and_probe: Duration::from_secs(7),
        probe_collision: 1,
        max_partition_data_for_current_round: Duration::from_millis(20),
        max_build_hash_table_for_current_round: Duration::from_millis(10),
        max_probe_for_current_round: Duration::from_millis(30),
        max_worker_fetch_and_probe_for_current_round: Duration::from_millis(40),
        max_worker_fetch_and_probe_ns: AtomicI64::new(2_000_000_000),
        spill: SpillStats {
            round: 1,
            partition_num: 4,
            spilled_partition_num_per_round: vec![2],
            total_spill_bytes_per_round: vec![1_073_741_824],
            spill_build_row_table_bytes_per_round: vec![536_870_912],
            spill_build_hash_table_bytes_per_round: vec![268_435_456],
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        stats.to_string(),
        "build_hash_table:{total:2s, fetch:1.7s, build:300ms}, probe:{concurrency:4, total:5s, max:2s, probe:3s, fetch_and_wait:2s, probe_collision:1}, spill:{round:1, spilled_partition_num_per_round:[2/4], total_spill_GiB_per_round:[1.00], build_spill_row_table_GiB_per_round:[0.50], build_spill_hash_table_per_round:[0.25]}"
    );
    stats.reset_round();
    assert_eq!(stats.max_partition_data, Duration::from_millis(220));
    assert_eq!(stats.max_build_hash_table, Duration::from_millis(110));
    assert_eq!(stats.max_probe, Duration::from_millis(3030));
    assert_eq!(
        stats.max_worker_fetch_and_probe_ns.load(Ordering::Acquire),
        2_040_000_000
    );
    assert_eq!(stats.max_probe_for_current_round, Duration::ZERO);

    stats.reset();
    assert_eq!(stats.fetch_and_build, Duration::ZERO);
    assert_eq!(stats.spill.round, 1);
    assert_eq!(stats.concurrency, 4);
}
