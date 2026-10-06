// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::hash_join_stats::{
    HashJoinRuntimeStats, HashJoinRuntimeStatsV2, SpillStats, write_bytes_stats,
    write_spilled_partition_num_stats,
};
use crate::index_lookup_join::{IndexLookUpJoinRuntimeStats, InnerWorkerRuntimeStats};
use astersql_executor_internal_exec::adaptive_limit_controller::AdaptiveLimitSnapshot;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

/// 校验 v1 统计：Clone 文本一致，Merge 累加耗时与 probe_collision，concurrency 保持原值。
#[test]
fn hash_join_runtime_stats_clone_merge_and_format_preserve_all_counters() {
    let mut stats = HashJoinRuntimeStats {
        fetch_and_build: Duration::from_secs(2),
        build_hash_table: Duration::from_millis(100),
        fetch_and_probe: Duration::from_secs(5),
        probe: Duration::from_secs(4),
        probe_collision: 1,
        concurrency: 4,
        max_fetch_and_probe_ns: AtomicI64::new(2_000_000_000),
        ..Default::default()
    };
    assert_eq!(
        stats.to_string(),
        "build_hash_table:{total:2s, fetch:1.9s, build:100ms}, probe:{concurrency:4, total:5s, max:2s, probe:4s, fetch and wait:1s, probe_collision:1}"
    );
    let cloned = stats.clone();
    assert_eq!(cloned.to_string(), stats.to_string());
    stats.merge(&cloned);
    assert_eq!(
        stats.to_string(),
        "build_hash_table:{total:4s, fetch:3.8s, build:200ms}, probe:{concurrency:4, total:10s, max:2s, probe:8s, fetch and wait:2s, probe_collision:2}"
    );
}

/// 精确复现 Go `TestIndexJoinRuntimeStats` 的 String、Clone 和 Merge 契约。
#[test]
fn index_lookup_join_runtime_stats_clone_merge_and_format_match_go() {
    let mut stats = IndexLookUpJoinRuntimeStats {
        concurrency: 5,
        probe: Duration::from_secs(1),
        inner_worker: InnerWorkerRuntimeStats {
            total_time: Duration::from_secs(5),
            tasks: 16,
            construct: Duration::from_millis(100),
            fetch: Duration::from_millis(300),
            build: Duration::from_millis(250),
            join: Duration::from_millis(150),
        },
        adaptive_limit_snapshot: None,
    };
    assert_eq!(
        stats.to_string(),
        "inner:{total:5s, concurrency:5, task:16, construct:100ms, fetch:300ms, build:250ms, join:150ms}, probe:1s"
    );
    let cloned = stats.clone();
    assert_eq!(cloned.to_string(), stats.to_string());
    stats.merge(&cloned);
    assert_eq!(
        stats.to_string(),
        "inner:{total:10s, concurrency:5, task:32, construct:200ms, fetch:600ms, build:500ms, join:300ms}, probe:2s"
    );
}

#[test]
fn index_lookup_join_runtime_stats_render_and_retain_adaptive_snapshot() {
    let mut stats = IndexLookUpJoinRuntimeStats {
        adaptive_limit_snapshot: Some(AdaptiveLimitSnapshot {
            outer_fetched: 1413,
            outer_consumed: 1000,
            lookup_handles: 1256,
            lookup_rows: 1000,
            outer_outstanding_at_stop: 413,
            lookup_outstanding_at_stop: 256,
            ..AdaptiveLimitSnapshot::default()
        }),
        ..IndexLookUpJoinRuntimeStats::default()
    };
    assert!(
        stats
            .to_string()
            .contains("adaptive:{outer:1413/1000, lookup:1256/1000, outstanding:413/256")
    );
    let other = IndexLookUpJoinRuntimeStats {
        adaptive_limit_snapshot: Some(AdaptiveLimitSnapshot {
            outer_fetched: 999,
            ..AdaptiveLimitSnapshot::default()
        }),
        ..IndexLookUpJoinRuntimeStats::default()
    };
    stats.merge(&other);
    assert_eq!(stats.adaptive_limit_snapshot.unwrap().outer_fetched, 1413);
}

/// 校验 v2 合并取耗时最大值、不合并 spill 向量，并格式化溢写统计。
#[test]
fn v2_spill_stats_merge_rounds_and_keep_max_worker_time() {
    let mut left = HashJoinRuntimeStatsV2 {
        max_build_hash_table: Duration::from_secs(1),
        spill: SpillStats {
            spilled_partition_num: vec![2, 1],
            spilled_bytes: vec![1024, 2048],
            restored_bytes: vec![512],
            ..Default::default()
        },
        concurrency: 2,
        ..Default::default()
    };
    left.set_max_worker_fetch_and_probe(Duration::from_millis(10));
    let right = HashJoinRuntimeStatsV2 {
        max_build_hash_table: Duration::from_secs(2),
        spill: SpillStats {
            spilled_partition_num: vec![1, 3, 4],
            spilled_bytes: vec![2048],
            restored_bytes: vec![256, 128],
            ..Default::default()
        },
        concurrency: 8,
        ..Default::default()
    };
    right.set_max_worker_fetch_and_probe(Duration::from_millis(20));
    left.merge(&right);
    assert_eq!(left.max_build_hash_table, Duration::from_secs(2));
    assert_eq!(left.spill.spilled_partition_num, [2, 1]);
    assert_eq!(left.spill.spilled_bytes, [1024, 2048]);
    assert_eq!(left.concurrency, 2);
    assert_eq!(
        left.max_worker_fetch_and_probe_ns.load(Ordering::Acquire),
        20_000_000
    );

    let mut formatted = String::new();
    write_spilled_partition_num_stats(&mut formatted, 4, &[3, 4]);
    write_bytes_stats(&mut formatted, &[1024, 2 * 1024 * 1024]);
    assert_eq!(formatted, "[3/4 4/12][0.00 0.00]");
}
