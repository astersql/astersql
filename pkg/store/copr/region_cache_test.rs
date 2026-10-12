// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Region 缓存与 bucket 拆分相关单元测试。
//
// 可运行用例验证按 bucket 切分键范围与覆盖校验。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::{
    BatchError, BatchResult, Buckets, CopRequest, CopTask, KeyLocation, KeyRange, KeyRanges,
    LocationKeyRanges, Peer, RegionCache, RegionCacheBackend, RegionFailureHandler, RegionInfo,
    RegionMeta, RegionStore, RegionVerId, ReplicaReadType, RpcContext, validate_location_coverage,
};

#[derive(Default)]
struct TestRegionBackend {
    locations: Vec<KeyLocation>,
    rpc_context: Option<RpcContext>,
    send_fail_count: Arc<AtomicUsize>,
}

impl RegionCacheBackend for TestRegionBackend {
    fn batch_locate_key_ranges(
        &self,
        _ranges: &[KeyRange],
        _need_leader: bool,
        _need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>> {
        Ok(self.locations.clone())
    }

    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.locations
            .iter()
            .find(|location| location.contains_start(key))
            .cloned()
            .ok_or_else(|| BatchError::OtherResponse("location not found".to_owned()))
    }

    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.locate_key(key)
    }

    fn locate_region_from_pd(&self, region_id: u64) -> BatchResult<KeyLocation> {
        self.locations
            .iter()
            .find(|location| location.region.id == region_id)
            .cloned()
            .ok_or_else(|| BatchError::OtherResponse("region not found".to_owned()))
    }

    fn invalidate_region(&self, _region: RegionVerId) {}
    fn update_buckets(&self, _region: RegionVerId, _old_version: u64, _new_version: u64) {}
    fn on_send_fail_tiflash(
        &self,
        _store: &RegionStore,
        _region: RegionVerId,
        _meta: &RegionMeta,
        _schedule_reload: bool,
        _error: &BatchError,
    ) {
        self.send_fail_count.fetch_add(1, Ordering::Relaxed);
    }

    fn tikv_rpc_context(
        &self,
        _region: RegionVerId,
        _replica_read: ReplicaReadType,
    ) -> BatchResult<Option<RpcContext>> {
        Ok(self.rpc_context.clone())
    }

    fn tiflash_rpc_context(
        &self,
        _region: RegionVerId,
        _is_mpp: bool,
    ) -> BatchResult<Option<RpcContext>> {
        Ok(None)
    }

    fn all_valid_tiflash_store_ids(
        &self,
        _region: RegionVerId,
        _primary_store_id: u64,
    ) -> Vec<u64> {
        Vec::new()
    }

    fn all_tiflash_stores(&self) -> Vec<RegionStore> {
        Vec::new()
    }

    fn compute_stores(&self) -> BatchResult<Vec<RegionStore>> {
        Ok(Vec::new())
    }

    fn fetch_topology(&self) -> BatchResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn is_store_alive(&self, _address: &str, _ttl: Duration) -> bool {
        false
    }

    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
        Ok(Vec::new())
    }
}

/// 用 UTF-8 字符串边界构造测试用 KeyRange。
fn key_range(start: &str, end: &str) -> KeyRange {
    KeyRange {
        start: start.as_bytes().to_vec(),
        end: end.as_bytes().to_vec(),
    }
}

#[test]
/// 验证按有序 bucket 边界切分后片段与版本保持正确。
fn bucket_split_preserves_range_and_bucket_order() {
    let input = LocationKeyRanges {
        location: KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            buckets: Some(Buckets {
                version: 7,
                keys: vec![b"a".to_vec(), b"m".to_vec(), b"z".to_vec()],
            }),
            ..KeyLocation::default()
        },
        ranges: KeyRanges::new(vec![key_range("b", "y")]),
    };
    let (groups, fallback) = input.split_key_ranges_by_buckets();
    assert!(fallback.is_none());
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].ranges.to_ranges(), vec![key_range("b", "m")]);
    assert_eq!(groups[1].ranges.to_ranges(), vec![key_range("m", "y")]);
    assert!(groups.iter().all(|group| group.bucket_version() == 7));
}

#[test]
/// 连续 location 覆盖通过；出现 gap 时校验失败。
fn coverage_rejects_gaps_and_accepts_contiguous_locations() {
    let ranges = vec![key_range("b", "y")];
    let mut locations = vec![
        KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"a".to_vec(),
            end_key: b"m".to_vec(),
            ..KeyLocation::default()
        },
        KeyLocation {
            region: RegionVerId::new(2, 1, 1),
            start_key: b"m".to_vec(),
            end_key: b"z".to_vec(),
            ..KeyLocation::default()
        },
    ];
    assert!(validate_location_coverage(&ranges, &locations));
    locations[1].start_key = b"n".to_vec();
    assert!(!validate_location_coverage(&ranges, &locations));
}

#[test]
/// stale bucket 不包含 range 起点时必须回退为未拆分 location，不能 panic 或空转。
fn stale_bucket_outside_region_falls_back_without_progress() {
    let input = LocationKeyRanges {
        location: KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"m".to_vec(),
            end_key: b"z".to_vec(),
            buckets: Some(Buckets {
                version: 3,
                keys: vec![b"a".to_vec(), b"f".to_vec(), b"z".to_vec()],
            }),
            ..KeyLocation::default()
        },
        ranges: KeyRanges::new(vec![key_range("b", "c")]),
    };
    let (groups, fallback) = input.split_key_ranges_by_buckets();
    let fallback = fallback.expect("out-of-location start must fall back");
    assert_eq!(fallback.reason, "range_start_outside_location");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].ranges, input.ranges);
}

#[test]
/// 非单调 bucket 边界触发整体回退并保留诊断原因。
fn unordered_bucket_boundaries_fall_back() {
    let input = LocationKeyRanges {
        location: KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            buckets: Some(Buckets {
                version: 3,
                keys: vec![b"a".to_vec(), b"m".to_vec(), b"f".to_vec(), b"z".to_vec()],
            }),
            ..KeyLocation::default()
        },
        ranges: KeyRanges::new(vec![key_range("b", "y")]),
    };
    let (groups, fallback) = input.split_key_ranges_by_buckets();
    assert_eq!(
        fallback.expect("unordered metadata must fall back").reason,
        "bucket_boundaries_not_ordered"
    );
    assert_eq!(groups[0].ranges, input.ranges);
}

#[test]
/// 空边界、离散 ranges 与多余 location 的覆盖判定与 Go 表驱动用例一致。
fn location_coverage_matches_go_edge_case_matrix() {
    let location = |id, start: &str, end: &str| KeyLocation {
        region: RegionVerId::new(id, 1, 1),
        start_key: start.as_bytes().to_vec(),
        end_key: end.as_bytes().to_vec(),
        ..KeyLocation::default()
    };
    assert!(validate_location_coverage(
        &[key_range("", "z")],
        &[location(1, "", "m"), location(2, "m", "z")]
    ));
    assert!(validate_location_coverage(
        &[key_range("a", "")],
        &[location(1, "a", "m"), location(2, "m", "")]
    ));
    assert!(validate_location_coverage(
        &[key_range("a", "b"), key_range("x", "z")],
        &[location(1, "a", "m"), location(2, "t", "z")]
    ));
    assert!(!validate_location_coverage(
        &[key_range("a", "b")],
        &[location(1, "a", "b"), location(2, "x", "z")]
    ));
    assert!(!validate_location_coverage(
        &[key_range("a", "z")],
        &[location(1, "a", "n"), location(2, "m", "z")]
    ));
}

#[test]
fn overlapping_ranges_restart_location_search_like_go() {
    let locations = vec![
        KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"a".to_vec(),
            end_key: b"m".to_vec(),
            ..KeyLocation::default()
        },
        KeyLocation {
            region: RegionVerId::new(2, 1, 1),
            start_key: b"m".to_vec(),
            end_key: b"z".to_vec(),
            ..KeyLocation::default()
        },
    ];
    let cache = RegionCache::new(Arc::new(TestRegionBackend {
        locations,
        ..TestRegionBackend::default()
    }));

    let split = cache
        .split_key_ranges_by_locations(
            KeyRanges::new(vec![key_range("a", "z"), key_range("b", "c")]),
            -1,
            false,
            false,
        )
        .expect("Go accepts contained ranges and preserves every fragment");

    assert_eq!(split.len(), 3);
    assert_eq!(split[0].ranges.to_ranges(), vec![key_range("a", "m")]);
    assert_eq!(split[1].ranges.to_ranges(), vec![key_range("m", "z")]);
    assert_eq!(split[2].ranges.to_ranges(), vec![key_range("b", "c")]);
}

#[test]
fn build_batch_task_uses_request_busy_threshold_like_go() {
    let mut labels = HashMap::new();
    labels.insert("estimated_wait_ms".to_owned(), "20".to_owned());
    let backend = TestRegionBackend {
        rpc_context: Some(RpcContext {
            store: Some(RegionStore {
                id: 42,
                labels,
                ..RegionStore::default()
            }),
            peer: Some(Peer {
                id: 7,
                store_id: 42,
            }),
            ..RpcContext::default()
        }),
        ..TestRegionBackend::default()
    };
    let cache = RegionCache::new(Arc::new(backend));
    let request = CopRequest {
        store_busy_threshold: Duration::from_millis(10),
        ..CopRequest::default()
    };
    let task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        busy_threshold: Duration::from_millis(100),
        ..CopTask::default()
    };

    assert!(
        cache
            .build_batch_task(&request, &task, ReplicaReadType::Leader)
            .expect("RPC context lookup succeeds")
            .is_none(),
        "Go compares EstimatedWaitTime with req.StoreBusyThreshold"
    );
}

#[test]
fn negative_limit_other_than_unspecified_returns_no_locations_like_go() {
    let cache = RegionCache::new(Arc::new(TestRegionBackend {
        locations: vec![KeyLocation {
            region: RegionVerId::new(1, 1, 1),
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            ..KeyLocation::default()
        }],
        ..TestRegionBackend::default()
    }));

    let split = cache
        .split_key_ranges_by_locations(KeyRanges::new(vec![key_range("a", "z")]), -2, false, false)
        .unwrap();
    assert!(split.is_empty());
}

#[test]
fn send_failure_callback_only_accepts_explicit_tiflash_store() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = RegionCache::new(Arc::new(TestRegionBackend {
        send_fail_count: Arc::clone(&calls),
        ..TestRegionBackend::default()
    }));
    let region = RegionInfo {
        Region: RegionVerId::new(1, 1, 1),
        Meta: Some(RegionMeta {
            id: 1,
            peers: vec![7],
        }),
        ..RegionInfo::default()
    };
    let error = BatchError::Transport("send failed".to_owned());

    cache.on_send_fail_for_batch_regions(
        Some(&RegionStore::default()),
        std::slice::from_ref(&region),
        false,
        &error,
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let mut labels = HashMap::new();
    labels.insert("engine".to_owned(), "tiflash".to_owned());
    cache.on_send_fail_for_batch_regions(
        Some(&RegionStore {
            labels,
            ..RegionStore::default()
        }),
        &[region],
        false,
        &error,
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
