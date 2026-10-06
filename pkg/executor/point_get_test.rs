// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Point Get 分区物理 ID 与分区名过滤的单元测试。
//
// 覆盖：无分区索引时回退到逻辑表 ID；有分区索引时取对应物理分区 ID；
// `matchPartitionNames` 按 ASCII 大小写不敏感匹配分区名。

use crate::point_get::{
    GetPhysID, PartitionDefinition, PartitionInfo, TableInfo, matchPartitionNames,
};

/// 验证物理分区 ID 解析与分区名大小写不敏感过滤。
#[test]
fn point_get_resolves_physical_partition_and_case_insensitive_name_filter() {
    // 构造含两个分区定义的 PartitionInfo（分区元数据）。
    let partitions = PartitionInfo {
        definitions: vec![
            PartitionDefinition {
                id: 11,
                name: "pNorth".into(),
            },
            PartitionDefinition {
                id: 12,
                name: "pSouth".into(),
            },
        ],
        ids_in_ddl_to_ignore: Vec::new(),
    };
    let table = TableInfo {
        id: 7,
        name: "orders".into(),
        temporary: false,
        cache_enabled: false,
        pk_is_handle: true,
        is_common_handle: false,
        columns: Vec::new(),
        primary_index: None,
        partition: Some(partitions.clone()),
        table_lock: None,
    };
    // 无分区索引 → 逻辑表 ID；索引 1 → 第二个分区物理 ID。
    assert_eq!(GetPhysID(&table, None), 7);
    assert_eq!(GetPhysID(&table, Some(1)), 12);
    // 分区名比较忽略大小写；不匹配的分区应返回 false。
    assert!(matchPartitionNames(11, &["PNORTH".into()], &partitions));
    assert!(!matchPartitionNames(12, &["PNORTH".into()], &partitions));
}

/// Go `GetPhysID` falls back to the logical table ID when a stale plan still
/// carries a partition index but the current table metadata is no longer
/// partitioned.
#[test]
fn point_get_falls_back_when_partition_metadata_is_absent() {
    let table = TableInfo {
        id: 19,
        name: "orders".into(),
        temporary: false,
        cache_enabled: false,
        pk_is_handle: true,
        is_common_handle: false,
        columns: Vec::new(),
        primary_index: None,
        partition: None,
        table_lock: None,
    };

    assert_eq!(GetPhysID(&table, Some(0)), 19);
}

#[test]
fn read_pool_snapshot_runtime_clone_merge_and_string_preserve_diagnostics() {
    use crate::point_get::{SnapshotRuntimeStats, runtimeStatsWithSnapshot};
    use std::sync::{Arc, Mutex};
    let pool = astersql_kv::PoolTaskDetails {
        TaskCount: 1,
        PollCount: 4,
        MaxPollCount: 4,
        MinPollCount: 4,
        PollWallTime: std::time::Duration::from_millis(12),
        ..Default::default()
    };
    let mut stats = runtimeStatsWithSnapshot {
        snapshot_runtime_stats: Some(Arc::new(Mutex::new(SnapshotRuntimeStats {
            description: "rpc:1".into(),
            read_pool_task_details: Some(pool.clone()),
            ..Default::default()
        }))),
    };
    let cloned = stats.Clone();
    stats.Merge(&cloned);
    assert!(
        cloned
            .String()
            .contains(&format!("read_pool:{}", pool.String()))
    );
    let mut aggregate = pool.clone();
    aggregate.Merge(&pool);
    assert_eq!(
        stats.String(),
        format!("rpc:1, rpc:1, read_pool:{}", aggregate.String())
    );
    let collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    collector.RegisterStatsShared(57, Box::new(stats));
    assert!(
        collector
            .GetRootStatsStringShared(57)
            .contains(&aggregate.String())
    );
}

#[test]
fn point_get_snapshot_runtime_merges_scan_diagnostics() {
    use crate::point_get::{SnapshotRuntimeStats, runtimeStatsWithSnapshot};
    use astersql_util_execdetails::execdetails::util::ScanDetail;
    use std::sync::{Arc, Mutex};

    let mut stats = runtimeStatsWithSnapshot {
        snapshot_runtime_stats: Some(Arc::new(Mutex::new(SnapshotRuntimeStats {
            scan_detail: Some(ScanDetail {
                ProcessedKeys: 1,
                TotalKeys: 2,
                IaCacheHitCount: 7,
                IaRemoteReadSegmentCount: 2,
                IaRemoteReadSegmentBytes: 4096,
                IaRemoteReadSegmentDuration: std::time::Duration::from_millis(6),
                ..Default::default()
            }),
            ..Default::default()
        }))),
    };
    let cloned = stats.Clone();
    stats.Merge(&cloned);
    let guard = stats
        .snapshot_runtime_stats
        .as_ref()
        .unwrap()
        .lock()
        .unwrap();
    let scan = guard.scan_detail.as_ref().unwrap();
    assert_eq!(scan.ProcessedKeys, 2);
    assert_eq!(scan.TotalKeys, 4);
    assert_eq!(scan.IaCacheHitCount, 14);
    assert_eq!(scan.IaRemoteReadSegmentCount, 4);
    assert_eq!(scan.IaRemoteReadSegmentBytes, 8192);
    assert_eq!(
        scan.IaRemoteReadSegmentDuration,
        std::time::Duration::from_millis(12)
    );
}
