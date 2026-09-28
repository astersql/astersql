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
// `GO_REFERENCE` 保留 Go 侧 location coverage、bucket fallback 等用例骨架；
// 下方可运行用例验证按 bucket 切分键范围与覆盖校验。

/* 对齐 Go `region_cache_test.go` 的历史参考骨架。
// 这段逻辑覆盖 location coverage 校验、bucket split panic 诊断、bucket fallback 和重叠 range 后续拆分。

// kr 对应 Go 文件中的测试辅助闭包：把字符串边界转为 tikv.KeyRange。
fn kr(start: &str, end: &str) -> tikv::KeyRange {
    tikv::KeyRange { StartKey: start.as_bytes().to_vec(), EndKey: end.as_bytes().to_vec() }
}

// kl 对应 Go 文件中的测试辅助闭包：用 regionID 和边界构造 KeyLocation。
fn kl(start: &str, end: &str, region_id: u64) -> tikv::KeyLocation {
    tikv::KeyLocation {
        Region: tikv::NewRegionVerID(region_id, 0, 0),
        StartKey: start.as_bytes().to_vec(),
        EndKey: end.as_bytes().to_vec(),
        ..Default::default()
    }
}

// coverageCase 对应 Go TestValidateLocationCoverage 的表驱动结构。
struct coverageCase {
    name: &'static str,
    ranges: Vec<tikv::KeyRange>,
    locs: Vec<tikv::KeyLocation>,
    want_valid: bool,
}

// test_validate_location_coverage 对应 Go 的 TestValidateLocationCoverage：覆盖精确覆盖、部分覆盖、gap、overlap、空边界和 unused location。
#[test]
fn test_validate_location_coverage() {
    let ctx = context::Background();
    let tests = vec![
        coverageCase { name: "single range, single location - exact match", ranges: vec![kr("a", "z")], locs: vec![kl("a", "z", 1)], want_valid: true },
        coverageCase { name: "single range, single location - location covers more", ranges: vec![kr("b", "y")], locs: vec![kl("a", "z", 1)], want_valid: true },
        coverageCase { name: "single range split across two locations", ranges: vec![kr("a", "z")], locs: vec![kl("a", "m", 1), kl("m", "z", 2)], want_valid: true },
        coverageCase { name: "single range split across three locations", ranges: vec![kr("a", "z")], locs: vec![kl("a", "h", 1), kl("h", "p", 2), kl("p", "z", 3)], want_valid: true },
        coverageCase { name: "multiple ranges, single location covers all", ranges: vec![kr("b", "d"), kr("f", "h")], locs: vec![kl("a", "z", 1)], want_valid: true },
        coverageCase { name: "multiple ranges, multiple locations - aligned", ranges: vec![kr("a", "m"), kr("m", "z")], locs: vec![kl("a", "m", 1), kl("m", "z", 2)], want_valid: true },
        coverageCase { name: "multiple ranges, multiple locations - disjoint ranges don't require covering gaps", ranges: vec![kr("b", "d"), kr("f", "h")], locs: vec![kl("a", "d", 1), kl("f", "i", 2)], want_valid: true },
        coverageCase { name: "multiple ranges, multiple locations - overlapping ranges don't require monotonic loc scan", ranges: vec![kr("a", "z"), kr("b", "c")], locs: vec![kl("a", "m", 1), kl("m", "t", 2), kl("t", "z", 3)], want_valid: true },
        coverageCase { name: "empty start key - location also empty", ranges: vec![kr("", "m")], locs: vec![kl("", "m", 1)], want_valid: true },
        coverageCase { name: "empty start key - location NOT empty", ranges: vec![kr("", "m")], locs: vec![kl("a", "m", 1)], want_valid: false },
        coverageCase { name: "empty end key - location also empty", ranges: vec![kr("m", "")], locs: vec![kl("m", "", 1)], want_valid: true },
        coverageCase { name: "empty end key - location NOT empty", ranges: vec![kr("m", "")], locs: vec![kl("m", "z", 1)], want_valid: false },
        coverageCase { name: "range with empty end - location extends to infinity", ranges: vec![kr("m", "")], locs: vec![kl("a", "", 1)], want_valid: true },
        coverageCase { name: "location doesn't cover range start", ranges: vec![kr("a", "z")], locs: vec![kl("b", "z", 1)], want_valid: false },
        coverageCase { name: "location doesn't cover range end", ranges: vec![kr("a", "z")], locs: vec![kl("a", "y", 1)], want_valid: false },
        coverageCase { name: "gap between locations", ranges: vec![kr("a", "z")], locs: vec![kl("a", "m", 1), kl("n", "z", 2)], want_valid: false },
        coverageCase { name: "discrete ranges with gap between locations - valid", ranges: vec![kr("a", "b"), kr("c", "d")], locs: vec![kl("a", "b", 1), kl("c", "d", 2)], want_valid: true },
        coverageCase { name: "discrete ranges in larger locations with gap - valid", ranges: vec![kr("a", "b"), kr("x", "z")], locs: vec![kl("a", "m", 1), kl("t", "z", 2)], want_valid: true },
        coverageCase { name: "missing range coverage", ranges: vec![kr("a", "m"), kr("m", "z")], locs: vec![kl("a", "m", 1)], want_valid: false },

        // Edge cases：保留 Go 对空 ranges/locs 的三个边界断言。
        coverageCase { name: "empty ranges with locations", ranges: vec![], locs: vec![kl("a", "z", 1)], want_valid: false },
        coverageCase { name: "empty ranges without locations", ranges: vec![], locs: vec![], want_valid: true },
        coverageCase { name: "empty locations", ranges: vec![kr("a", "z")], locs: vec![], want_valid: false },
        coverageCase { name: "exact boundary match", ranges: vec![kr("a", "m"), kr("m", "z")], locs: vec![kl("a", "m", 1), kl("m", "z", 2)], want_valid: true },
        coverageCase { name: "location boundary equals range start", ranges: vec![kr("m", "z")], locs: vec![kl("m", "z", 1)], want_valid: true },

        // Monotonicity violations：location 顺序、重叠和 +inf 后续重叠都必须判无效。
        coverageCase { name: "locations not monotonic", ranges: vec![kr("a", "z")], locs: vec![kl("m", "z", 1), kl("a", "m", 2)], want_valid: false },
        coverageCase { name: "locations overlap", ranges: vec![kr("a", "z")], locs: vec![kl("a", "n", 1), kl("m", "z", 2)], want_valid: false },
        coverageCase { name: "location extends to infinity and overlaps next - invalid", ranges: vec![kr("a", "z")], locs: vec![kl("a", "", 1), kl("m", "z", 2)], want_valid: false },

        // Unused location violations (Property 3)：多余 location 未覆盖任何 range 时应判无效。
        coverageCase { name: "extra location not covering any range", ranges: vec![kr("a", "b")], locs: vec![kl("a", "b", 1), kl("x", "z", 2)], want_valid: false },
        coverageCase { name: "middle location not covering any range", ranges: vec![kr("a", "c")], locs: vec![kl("a", "b", 1), kl("b", "c", 2), kl("x", "z", 3)], want_valid: false },
        coverageCase { name: "current location starts from beginning after non-beginning", ranges: vec![kr("a", "z")], locs: vec![kl("a", "m", 1), kl("", "z", 2)], want_valid: false },
        coverageCase { name: "valid: first location starts from beginning", ranges: vec![kr("", "z")], locs: vec![kl("", "m", 1), kl("m", "z", 2)], want_valid: true },
        coverageCase { name: "valid: last location extends to infinity", ranges: vec![kr("a", "")], locs: vec![kl("a", "m", 1), kl("m", "", 2)], want_valid: true },
    ];

    for tt in tests {
        testing::run(tt.name, || {
            let got = validateLocationCoverage(ctx.clone(), tt.ranges, tt.locs);
            if got != tt.want_valid {
                testing::errorf("validateLocationCoverage() = {:?}, want {:?}", got, tt.want_valid);
            }
        });
    }
}

// test_panic_in_split_key_ranges_by_buckets 对应 Go 测试：通过 failpoint 在第三个 location 触发 panic，并验证 recover 后重新 panic。
#[test]
fn test_panic_in_split_key_ranges_by_buckets() {
    // Go 使用 mock TiKV cluster 建出 4 个 region：nil---g---n---x---nil。
    let (mock_client, cluster, pd_client) = testutils::NewMockTiKV("", None).unwrap();
    defer::defer(|| {
        pd_client.Close();
        require::no_error(mock_client.Close());
    });

    let (_store_id, region_ids, _peer_ids) =
        testutils::BootstrapWithMultiRegions(cluster, vec![b"g".to_vec(), b"n".to_vec(), b"x".to_vec()]);
    // 每个 region 配置 bucket，保持 Go 中 nil/字节字面量边界的布局。
    cluster.SplitRegionBuckets(region_ids[0], vec![vec![], b"c".to_vec(), b"g".to_vec()], region_ids[0]);
    cluster.SplitRegionBuckets(region_ids[1], vec![b"g".to_vec(), b"k".to_vec(), b"n".to_vec()], region_ids[1]);
    cluster.SplitRegionBuckets(region_ids[2], vec![b"n".to_vec(), b"t".to_vec(), b"x".to_vec()], region_ids[2]);
    cluster.SplitRegionBuckets(region_ids[3], vec![b"x".to_vec(), vec![]], region_ids[3]);

    let pd_cli = tikv::NewCodecPDClient(tikv::ModeTxn, pd_client);
    defer::defer(|| pd_cli.Close());
    let cache = NewRegionCache(tikv::NewRegionCache(pd_cli));
    defer::defer(|| cache.Close());

    let bo = backoff::NewBackofferWithVars(context::Background(), 3000, None);
    let ranges = buildCopRanges("a", "z");

    // failpoint 返回 2，表示处理 location index=2 时触发 panic。
    require::no_error(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/store/copr/panicInSplitKeyRangesByBuckets",
        "return(2)",
    ));
    defer::defer(|| {
        require::no_error(failpoint::Disable("github.com/pingcap/tidb/pkg/store/copr/panicInSplitKeyRangesByBuckets"));
    });

    let mut did_panic = false;
    let mut panic_value = String::new();
    let result = panic::catch_unwind(|| {
        let _ = cache.SplitKeyRangesByBuckets(bo, ranges);
        testing::fatal("Expected panic but none occurred");
    });
    if let Err(r) = result {
        did_panic = true;
        panic_value = r.downcast_string();
        testing::logf(format!("Successfully caught panic: {:?}", r));
    }

    require::true_(did_panic, "Expected panic to occur");
    require::equal("failpoint triggered panic in bucket splitting", panic_value);
    testing::log("Test completed successfully - panic was caught, diagnostics logged, and re-panicked as expected");
}

// test_locate_bucket_nil_fallback 对应 Go 测试：首个 range 在 location 外时返回未拆分的原始 LocationKeyRanges。
#[test]
fn test_locate_bucket_nil_fallback() {
    let ctx = context::Background();
    let loc = tikv::KeyLocation {
        Region: tikv::NewRegionVerID(1, 0, 0),
        StartKey: b"a".to_vec(),
        EndKey: b"m".to_vec(),
        Buckets: Some(metapb::Buckets { Keys: vec![b"a".to_vec(), b"f".to_vec(), b"m".to_vec()], Version: 1 }),
    };

    // 这里故意构造 [x,z)，位于 [a,m) 之外；Go bug 场景要求 fallback 而不是 panic。
    let outside_ranges = NewKeyRanges(vec![kv::KeyRange { StartKey: b"x".to_vec(), EndKey: b"z".to_vec() }]);
    let lkr = LocationKeyRanges { Location: loc, Ranges: outside_ranges };

    let (result, fb) = lkr.splitKeyRangesByBuckets(ctx);
    require::not_nil(fb);
    require::len(result, 1, "Expected 1 unsplit LocationKeyRanges");
    require::equal(lkr, result[0], "Expected original LocationKeyRanges to be returned");
    testing::log("StartKey outside location fallback working correctly - returned unsplit ranges instead of panicking");
}

// test_locate_bucket_outside_region_non_nil_fallback 对应 Go 测试：stale bucket 能返回非 nil bucket，但仍不包含 startKey。
#[test]
fn test_locate_bucket_outside_region_non_nil_fallback() {
    let ctx = context::Background();
    let loc = tikv::KeyLocation {
        Region: tikv::NewRegionVerID(1, 0, 0),
        StartKey: b"m".to_vec(),
        EndKey: b"z".to_vec(),
        Buckets: Some(metapb::Buckets { Keys: vec![b"a".to_vec(), b"f".to_vec(), b"z".to_vec()], Version: 1 }),
    };

    let start_key = b"b".to_vec(); // outside [m, z)
    require::false_(loc.Contains(start_key.clone()), "sanity: startKey should be outside location");
    let bucket = loc.LocateBucket(start_key.clone());
    require::not_nil(bucket, "LocateBucket should return a non-nil bucket for this stale metadata");
    require::false_(bucket.Contains(start_key.clone()), "sanity: clamped bucket should not contain the key");

    let lkr = LocationKeyRanges {
        Location: loc,
        Ranges: NewKeyRanges(vec![kv::KeyRange { StartKey: start_key, EndKey: b"c".to_vec() }]),
    };
    let (result, fb) = lkr.splitKeyRangesByBuckets(ctx);
    require::not_nil(fb);
    require::len(result, 1, "Expected unsplit LocationKeyRanges fallback");
    require::equal(lkr, result[0], "Expected original LocationKeyRanges to be returned");
}

// test_overlapping_ranges_can_produce_out_of_order_ranges_after_split 对应 Go 测试：重叠输入拆 region 后可能生成非单调剩余 ranges。
#[test]
fn test_overlapping_ranges_can_produce_out_of_order_ranges_after_split() {
    let ctx = context::Background();

    // Go 注释中的根因链路保留如下：
    // 1. 输入 ranges 重叠或包含，例如 [a,z) 包含 [b,c)。
    // 2. splitKeyRangesByLocation 在 location 边界拆第一段，把 [m,z) 存到 ranges.first。
    // 3. 仍未处理的 [b,c) 留在 ranges.mid，使剩余 KeyRanges 变成 [m,z) 再 [b,c)。
    // 4. 后续 location 收到 start < location.StartKey 的 range，bucket split 必须 fallback，避免 livelock。
    let loc0 = tikv::KeyLocation {
        Region: tikv::NewRegionVerID(1, 0, 0),
        StartKey: b"a".to_vec(),
        EndKey: b"m".to_vec(),
        ..Default::default()
    };
    let loc1 = tikv::KeyLocation {
        Region: tikv::NewRegionVerID(2, 0, 0),
        StartKey: b"m".to_vec(),
        EndKey: b"z".to_vec(),
        Buckets: Some(metapb::Buckets { Keys: vec![b"m".to_vec(), b"z".to_vec()], Version: 1 }),
    };

    let ranges = NewKeyRanges(vec![
        kv::KeyRange { StartKey: b"a".to_vec(), EndKey: b"z".to_vec() },
        kv::KeyRange { StartKey: b"b".to_vec(), EndKey: b"c".to_vec() },
    ]);

    let cache = RegionCache {};
    let res = vec![];
    let (_res, remaining, is_break) = cache.splitKeyRangesByLocation(ctx.clone(), loc0, ranges, res);
    require::false_(is_break, "should not break: more locations remain");
    require::equal(2, remaining.Len(), "expect [m,z) + original contained range to remain");
    require::equal("m", String::from_utf8(remaining.At(0).StartKey));
    require::equal("z", String::from_utf8(remaining.At(0).EndKey));
    require::equal("b", String::from_utf8(remaining.At(1).StartKey));
    require::equal("c", String::from_utf8(remaining.At(1).EndKey));

    // 关键断言：bucket split 观察到 range_start_outside_location 后回退，并返回原 lkr。
    let lkr = LocationKeyRanges { Location: loc1, Ranges: remaining };
    let (result, fb) = lkr.splitKeyRangesByBuckets(ctx);
    require::not_nil(fb);
    require::equal("range_start_outside_location", fb.reason);
    require::len(result, 1);
    require::equal(lkr, result[0]);
}
*/

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
