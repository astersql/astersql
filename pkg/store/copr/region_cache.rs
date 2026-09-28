// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Region 缓存与键范围按 location / bucket 拆分。
//
// Region 是 TiKV 键空间的分片单元；PD（Placement Driver）维护其路由。
// 本模块将查询的 KeyRange 按 Region（及可选 bucket）切成可下发的片段，
// 并校验 location 覆盖完整性；bucket 元数据异常时回退到仅按 Region 拆分。

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::batch_coprocessor::{BatchTaskSource, ReplicaReadPolicy};
use crate::batch_request_sender::{
    BatchError, BatchResult, KeyRange, KeyRanges, Peer, RegionFailureHandler, RegionInfo,
    RegionMeta, RegionVerId, RpcContext, Store as RegionStore,
};
use crate::coprocessor::{BatchedCopTask, CopRequest, CopTask, ReplicaReadType};
use crate::range_diagnostics::{RangeIssueStats, range_issues_for_key_ranges};

/// 拆分结果数量不设上限。
pub const UNSPECIFIED_LIMIT: isize = -1;
/// location 列表不足时重新 locate 的最大次数，防止 livelock。
pub const MAX_RELOCATE_ON_OVERFLOW: usize = 64;
/// 覆盖摘要中最多展示的 location 条数。
pub const LOCATION_SUMMARY_MAX_DISPLAY: usize = 5;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 内的 bucket 元数据：有序边界键将 Region 再细分为负载均衡单元。
pub struct Buckets {
    /// bucket 版本号，变更时需刷新缓存。
    pub version: u64,
    /// Ordered boundaries. Each adjacent pair forms one bucket.
    /// 有序边界；相邻一对构成一个 bucket。
    pub keys: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 某个键所属 Region 的定位结果：边界、可选 bucket、store 与 peer。
pub struct KeyLocation {
    /// Region 版本身份（含 conf/version）。
    pub region: RegionVerId,
    /// Region 起始键（含）。
    /// 相关 range 起点。
    pub start_key: Vec<u8>,
    /// Region 结束键（不含）；空表示 +inf。
    /// 相关 range 终点。
    pub end_key: Vec<u8>,
    /// 可选的 bucket 细分信息。
    pub buckets: Option<Buckets>,
    /// 选中的 store。
    pub store: Option<RegionStore>,
    /// 选中的 peer。
    pub peer: Option<Peer>,
}

impl KeyLocation {
    /// 半开区间 [start, end) 是否包含作为 range 起点的 key。
    pub fn contains_start(&self, key: &[u8]) -> bool {
        key >= self.start_key.as_slice()
            && (self.end_key.is_empty() || key < self.end_key.as_slice())
    }

    /// 是否覆盖作为 range 终点的 key（空 end 需双方均为 +inf）。
    pub fn covers_end(&self, key: &[u8]) -> bool {
        if key.is_empty() {
            return self.end_key.is_empty();
        }
        key > self.start_key.as_slice()
            && (self.end_key.is_empty() || key <= self.end_key.as_slice())
    }

    /// 返回 bucket 版本；无 bucket 时为 0。
    pub fn bucket_version(&self) -> u64 {
        self.buckets
            .as_ref()
            .map(|buckets| buckets.version)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
/// 同一 location 下绑定的一组 KeyRanges。
pub struct LocationKeyRanges {
    /// 所属 location。
    pub location: KeyLocation,
    /// 落在该 location 内的键范围。
    pub ranges: KeyRanges,
}

impl LocationKeyRanges {
    /// 委托 location 取 bucket 版本。
    pub fn bucket_version(&self) -> u64 {
        self.location.bucket_version()
    }

    /// 按 bucket 边界切分 ranges；失败时返回 fallback 诊断信息。
    pub fn split_key_ranges_by_buckets(
        &self,
    ) -> (Vec<LocationKeyRanges>, Option<BucketSplitFallbackInfo>) {
        let Some(buckets) = &self.location.buckets else {
            return (vec![self.clone()], None);
        };
        if buckets.keys.is_empty() {
            return (vec![self.clone()], None);
        }
        // bucket 边界乱序：放弃细分，交由上层回退。
        if buckets.keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return (
                vec![self.clone()],
                Some(BucketSplitFallbackInfo::new(
                    "bucket_boundaries_not_ordered",
                    self.ranges.ref_at(0),
                    self.ranges.len(),
                )),
            );
        }
        let Some(first) = self.ranges.ref_at(0) else {
            return (Vec::new(), None);
        };
        if !self.location.contains_start(&first.start) {
            return (
                vec![self.clone()],
                Some(BucketSplitFallbackInfo::new(
                    "range_start_outside_location",
                    Some(first),
                    self.ranges.len(),
                )),
            );
        }

        let mut result: Vec<LocationKeyRanges> = Vec::new();
        for range in self.ranges.iter() {
            if !self.location.contains_start(&range.start) {
                return (
                    vec![self.clone()],
                    Some(BucketSplitFallbackInfo::new(
                        "bucket_not_contain_start_no_progress",
                        Some(range),
                        self.ranges.len(),
                    )),
                );
            }
            let mut start = range.start.clone();
            loop {
                let boundary = buckets
                    .keys
                    .iter()
                    .find(|key| key.as_slice() > start.as_slice())
                    .cloned()
                    .unwrap_or_else(|| self.location.end_key.clone());
                let end = if range.end.is_empty() {
                    boundary.clone()
                } else if boundary.is_empty() || range.end < boundary {
                    range.end.clone()
                } else {
                    boundary.clone()
                };
                // 切分未前进，避免死循环，返回诊断信息。
                if !end.is_empty() && start >= end {
                    return (
                        vec![self.clone()],
                        Some(BucketSplitFallbackInfo {
                            reason: "bucket_split_no_progress".to_owned(),
                            start_key: start,
                            end_key: range.end.clone(),
                            bucket_start: Vec::new(),
                            bucket_end: boundary,
                            remaining_range_count: self.ranges.len(),
                        }),
                    );
                }
                let bucket_index = buckets
                    .keys
                    .partition_point(|key| key.as_slice() <= start.as_slice())
                    .saturating_sub(1);
                if result.last().is_none_or(|item| {
                    item.location.bucket_version() != buckets.version
                        || item
                            .ranges
                            .ref_at(0)
                            .map(|first| {
                                buckets
                                    .keys
                                    .partition_point(|key| key.as_slice() <= first.start.as_slice())
                                    .saturating_sub(1)
                                    != bucket_index
                            })
                            .unwrap_or(true)
                }) {
                    result.push(LocationKeyRanges {
                        location: self.location.clone(),
                        ranges: KeyRanges::default(),
                    });
                }
                result
                    .last_mut()
                    .expect("bucket group was inserted")
                    .ranges
                    .0
                    .push(KeyRange {
                        start: start.clone(),
                        end: end.clone(),
                    });
                if end == range.end || (range.end.is_empty() && end.is_empty()) {
                    break;
                }
                start = end;
            }
        }
        (result, None)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// bucket 拆分失败时的诊断信息，供日志与回退决策。
pub struct BucketSplitFallbackInfo {
    /// 失败原因标签。
    pub reason: String,
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    /// 当时的 bucket 起点（若适用）。
    pub bucket_start: Vec<u8>,
    /// 当时的 bucket 终点（若适用）。
    pub bucket_end: Vec<u8>,
    /// 剩余未处理 range 数量。
    pub remaining_range_count: usize,
}

impl BucketSplitFallbackInfo {
    /// 由原因与可选 range 构造诊断信息。
    fn new(reason: &str, range: Option<&KeyRange>, remaining: usize) -> Self {
        Self {
            reason: reason.to_owned(),
            start_key: range.map(|range| range.start.clone()).unwrap_or_default(),
            end_key: range.map(|range| range.end.clone()).unwrap_or_default(),
            remaining_range_count: remaining,
            ..Self::default()
        }
    }
}

/// 比较键边界；空且为 end 视为 +inf，空且为 start 按普通空键比较。
pub fn compare_key_range_boundary(
    left: &[u8],
    right: &[u8],
    left_is_start: bool,
    right_is_start: bool,
) -> Ordering {
    if left.is_empty() && !left_is_start {
        return if right.is_empty() && !right_is_start {
            Ordering::Equal
        } else {
            Ordering::Greater
        };
    }
    if right.is_empty() && !right_is_start {
        return Ordering::Less;
    }
    left.cmp(right)
}

/// 包装 `KeyLocation::contains_start`。
pub fn location_contains_start_key(location: &KeyLocation, key: &[u8]) -> bool {
    location.contains_start(key)
}

/// 包装 `KeyLocation::covers_end`。
pub fn location_covers_end_key(location: &KeyLocation, key: &[u8]) -> bool {
    location.covers_end(key)
}

/// 检查 location 列表起点递增且相邻无重叠（可接壤）。
pub fn check_locations_ordered(locations: &[KeyLocation]) -> bool {
    locations.windows(2).all(|pair| {
        compare_key_range_boundary(&pair[0].start_key, &pair[1].start_key, true, true)
            == Ordering::Less
            && compare_key_range_boundary(&pair[0].end_key, &pair[1].start_key, false, true)
                != Ordering::Greater
    })
}

/// 检查 ranges 是否被连续 location 覆盖；返回各 location 是否被使用及整体是否有效。
pub fn check_ranges_covered(ranges: &[KeyRange], locations: &[KeyLocation]) -> (Vec<bool>, bool) {
    let mut used = vec![false; locations.len()];
    let mut valid = true;
    let mut location_index = 0;
    for range in ranges {
        while location_index < locations.len()
            && !locations[location_index].end_key.is_empty()
            && locations[location_index].end_key <= range.start
        {
            location_index += 1;
        }
        if location_index >= locations.len()
            || !locations[location_index].contains_start(&range.start)
        {
            valid = false;
            continue;
        }
        let mut cover_index = location_index;
        used[cover_index] = true;
        while !locations[cover_index].covers_end(&range.end) {
            let next = cover_index + 1;
            if next >= locations.len()
                || locations[cover_index].end_key != locations[next].start_key
            {
                valid = false;
                break;
            }
            cover_index = next;
            used[cover_index] = true;
        }
    }
    (used, valid)
}

/// 校验 location 有序、完整覆盖 ranges，且无未使用的多余 location。
pub fn validate_location_coverage(ranges: &[KeyRange], locations: &[KeyLocation]) -> bool {
    if ranges.is_empty() {
        return locations.is_empty();
    }
    if locations.is_empty() || !check_locations_ordered(locations) {
        return false;
    }
    let (used, covered) = check_ranges_covered(ranges, locations);
    covered && used.into_iter().all(|used| used)
}

/// 生成 gap/overlap/contiguous 统计与焦点附近 location 摘要字符串。
pub fn location_coverage_summary(locations: &[KeyLocation], focus: usize) -> String {
    if locations.is_empty() {
        return "count=0".to_owned();
    }
    let mut gaps = 0;
    let mut overlaps = 0;
    let mut contiguous = 0;
    for pair in locations.windows(2) {
        match compare_key_range_boundary(&pair[0].end_key, &pair[1].start_key, false, true) {
            Ordering::Less => gaps += 1,
            Ordering::Greater => overlaps += 1,
            Ordering::Equal => contiguous += 1,
        }
    }
    let focus = focus.min(locations.len() - 1);
    let start = focus.saturating_sub(LOCATION_SUMMARY_MAX_DISPLAY / 2);
    let end = (start + LOCATION_SUMMARY_MAX_DISPLAY).min(locations.len());
    let shown = locations[start..end]
        .iter()
        .enumerate()
        .map(|(offset, location)| format!("{}:r{}", start + offset, location.region.id))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "count={} gaps={gaps} overlaps={overlaps} contiguous={contiguous} focus={focus} locs=[{shown}]",
        locations.len()
    )
}

/// Region 缓存后端：批量定位、失效、TiKV/TiFlash RPC 上下文与拓扑查询。
pub trait RegionCacheBackend: Send + Sync + 'static {
    /// 批量定位 key ranges 到 Region。
    fn batch_locate_key_ranges(
        &self,
        ranges: &[KeyRange],
        need_leader: bool,
        need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>>;
    /// 定位单个 key。
    fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation>;
    /// 按排他 end key 定位前一个 Region；语义与 client-go `LocateEndKey` 一致。
    fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation>;
    /// 从 PD 按 region_id 定位。
    fn locate_region_from_pd(&self, region_id: u64) -> BatchResult<KeyLocation>;
    /// 使指定 Region 版本缓存失效。
    fn invalidate_region(&self, region: RegionVerId);
    /// 更新 Region 的 bucket 版本。
    fn update_buckets(&self, region: RegionVerId, old_version: u64, new_version: u64);
    /// TiFlash 发送失败时的回调处理。
    fn on_send_fail_tiflash(
        &self,
        store: &RegionStore,
        region: RegionVerId,
        meta: &RegionMeta,
        schedule_reload: bool,
        error: &BatchError,
    );
    /// 构造 TiKV RPC 上下文。
    fn tikv_rpc_context(
        &self,
        region: RegionVerId,
        replica_read: ReplicaReadType,
    ) -> BatchResult<Option<RpcContext>>;
    /// 构造 TiFlash RPC 上下文。
    fn tiflash_rpc_context(
        &self,
        region: RegionVerId,
        is_mpp: bool,
    ) -> BatchResult<Option<RpcContext>>;
    /// 某 Region 上有效的 TiFlash store ID 列表。
    fn all_valid_tiflash_store_ids(&self, region: RegionVerId, primary_store_id: u64) -> Vec<u64>;
    /// 全部 TiFlash store。
    fn all_tiflash_stores(&self) -> Vec<RegionStore>;
    /// 计算层（disaggregated）store 列表。
    fn compute_stores(&self) -> BatchResult<Vec<RegionStore>>;
    /// 拉取拓扑信息。
    fn fetch_topology(&self) -> BatchResult<Vec<String>>;
    /// 判断 store 在 TTL 内是否存活。
    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool;
    /// TiDB server 地址列表。
    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>>;
}

/// 基于可注入后端的 Region 缓存门面，负责按 location/bucket 拆分。
pub struct RegionCache {
    /// 定位与失效的具体后端。
    backend: Arc<dyn RegionCacheBackend>,
}

impl RegionCache {
    /// 用给定后端构造缓存。
    pub fn new(backend: Arc<dyn RegionCacheBackend>) -> Self {
        Self { backend }
    }

    /// 返回后端引用。
    pub fn backend(&self) -> &Arc<dyn RegionCacheBackend> {
        &self.backend
    }

    /// 定位包含 start key 的 Region。
    pub fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.backend.locate_key(key)
    }

    /// 定位排他 end key 左侧的 Region。
    pub fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
        self.backend.locate_end_key(key)
    }

    /// 按 Region location 拆分后展平为 KeyRange 列表。
    pub fn split_region_ranges(
        &self,
        ranges: Vec<KeyRange>,
        limit: isize,
    ) -> BatchResult<Vec<KeyRange>> {
        Ok(self
            .split_key_ranges_by_locations(KeyRanges::new(ranges), limit, true, false)?
            .into_iter()
            .flat_map(|location| location.ranges.0)
            .collect())
    }

    /// 将 KeyRanges 按 Region location 切成 LocationKeyRanges；必要时补 locate。
    pub fn split_key_ranges_by_locations(
        &self,
        ranges: KeyRanges,
        limit: isize,
        need_leader: bool,
        buckets: bool,
    ) -> BatchResult<Vec<LocationKeyRanges>> {
        if limit == 0 || ranges.is_empty() {
            return Ok(Vec::new());
        }
        let mut locations =
            self.backend
                .batch_locate_key_ranges(&ranges.to_ranges(), need_leader, buckets)?;
        let mut result: Vec<LocationKeyRanges> = Vec::new();
        let mut relocations = 0usize;

        for original in ranges.iter() {
            let mut remaining = original.clone();
            // Go keeps the unprocessed ranges in a queue. That queue can become
            // non-monotonic when an earlier range contains a later one, so each
            // logical range must search the prefetched locations from the start.
            let mut location_index = 0usize;
            loop {
                if limit != UNSPECIFIED_LIMIT && (limit < 0 || result.len() >= limit as usize) {
                    return Ok(result);
                }
                while location_index < locations.len()
                    && !locations[location_index].end_key.is_empty()
                    && locations[location_index].end_key <= remaining.start
                {
                    location_index += 1;
                }
                if location_index >= locations.len() {
                    // 超过补 locate 预算，避免无限重试。
                    if relocations >= MAX_RELOCATE_ON_OVERFLOW {
                        return Err(BatchError::OtherResponse(format!(
                            "SplitKeyRangesByLocations: re-locate overflow budget exhausted after {relocations} attempts"
                        )));
                    }
                    locations.push(self.backend.locate_key(&remaining.start)?);
                    relocations += 1;
                    continue;
                }
                let location = &locations[location_index];
                if !location.contains_start(&remaining.start) {
                    return Err(BatchError::OtherResponse(
                        "SplitKeyRangesByLocations: remaining ranges start outside location"
                            .to_owned(),
                    ));
                }
                let fragment_end = if remaining.end.is_empty() {
                    location.end_key.clone()
                } else if location.end_key.is_empty() || remaining.end <= location.end_key {
                    remaining.end.clone()
                } else {
                    location.end_key.clone()
                };
                // 片段无进展，返回错误而非死循环。
                if !fragment_end.is_empty() && remaining.start >= fragment_end {
                    return Err(BatchError::OtherResponse(
                        "SplitKeyRangesByLocations made no progress".to_owned(),
                    ));
                }
                let fragment = KeyRange {
                    start: remaining.start.clone(),
                    end: fragment_end.clone(),
                };
                if let Some(last) = result.last_mut()
                    && last.location.region == location.region
                {
                    last.ranges.0.push(fragment);
                } else {
                    result.push(LocationKeyRanges {
                        location: location.clone(),
                        ranges: KeyRanges::new(vec![fragment]),
                    });
                }
                if fragment_end == remaining.end
                    || (fragment_end.is_empty() && remaining.end.is_empty())
                {
                    break;
                }
                remaining.start = fragment_end;
                location_index += 1;
            }
        }
        Ok(result)
    }

    /// 先按 location 再按 bucket 拆分；任一段 fallback 则整批退回仅 Region 拆分。
    pub fn split_key_ranges_by_buckets(
        &self,
        ranges: KeyRanges,
    ) -> BatchResult<Vec<LocationKeyRanges>> {
        let original = ranges.to_ranges();
        let locations =
            self.split_key_ranges_by_locations(ranges.clone(), UNSPECIFIED_LIMIT, false, true)?;
        let coverage_locations: Vec<_> = locations
            .iter()
            .map(|location| location.location.clone())
            .collect();
        let mut result = Vec::new();
        for location in &locations {
            let (split, fallback) = location.split_key_ranges_by_buckets();
            if fallback.is_some() {
                let _coverage_valid = validate_location_coverage(&original, &coverage_locations);
                let _issues: RangeIssueStats = range_issues_for_key_ranges(&ranges);
                // bucket 仅为优化；元数据不一致时回退到纯 Region 拆分。
                // Buckets are an optimization. Inconsistent metadata always falls
                // back to a fresh region-only split.
                return self.split_key_ranges_by_locations(ranges, UNSPECIFIED_LIMIT, false, false);
            }
            result.extend(split);
        }
        Ok(result)
    }

    /// 在 Leader 副本读且 store 空闲阈值允许时，构造可批量发送的 Cop 任务。
    pub fn build_batch_task(
        &self,
        request: &CopRequest,
        task: &CopTask,
        replica_read: ReplicaReadType,
    ) -> BatchResult<Option<BatchedCopTask>> {
        if replica_read != ReplicaReadType::Leader {
            return Ok(None);
        }
        let Some(context) = self.backend.tikv_rpc_context(task.region, replica_read)? else {
            return Ok(None);
        };
        let Some(store) = context.store else {
            return Ok(None);
        };
        let estimated_wait = store
            .labels
            .get("estimated_wait_ms")
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_default();
        if estimated_wait > request.store_busy_threshold {
            return Ok(None);
        }
        Ok(Some(BatchedCopTask {
            task: Box::new(task.clone()),
            store_id: store.id,
            peer: context.peer,
            load_based_replica_retry: replica_read != ReplicaReadType::Leader,
        }))
    }

    /// 使指定 Region 的缓存条目失效。
    pub fn invalidate_region(&self, region: RegionVerId) {
        self.backend.invalidate_region(region);
    }

    /// 在版本匹配时更新 Region 的 bucket 缓存。
    pub fn update_buckets(&self, region: RegionVerId, old_version: u64, new_version: u64) {
        self.backend
            .update_buckets(region, old_version, new_version);
    }
}

/// TiFlash 批量发送失败时，按 Region 通知后端做失效/重载。
impl RegionFailureHandler for RegionCache {
    /// 批量 Region 发送失败时的失效处理。
    fn on_send_fail_for_batch_regions(
        &self,
        store: Option<&RegionStore>,
        regions: &[RegionInfo],
        schedule_reload: bool,
        error: &BatchError,
    ) {
        let Some(store) = store else {
            return;
        };
        if store
            .labels
            .get("engine")
            .is_none_or(|engine| engine != "tiflash")
        {
            return;
        }
        for region in regions.iter().filter(|region| region.Meta.is_some()) {
            self.backend.on_send_fail_tiflash(
                store,
                region.Region,
                region.Meta.as_ref().expect("filtered metadata"),
                schedule_reload,
                error,
            );
        }
    }
}

/// 作为 batch cop / MPP 的任务源：按 TiFlash 上下文拆分键范围。
impl BatchTaskSource for RegionCache {
    /// BatchTaskSource：按策略切分 key ranges。
    fn split_key_ranges(
        &self,
        ranges: &KeyRanges,
        partition_index: i64,
    ) -> BatchResult<Vec<RegionInfo>> {
        self.split_key_ranges_by_locations(ranges.clone(), UNSPECIFIED_LIMIT, false, false)?
            .into_iter()
            .map(|location| {
                let context = self
                    .backend
                    .tiflash_rpc_context(location.location.region, true)?;
                let primary = context
                    .as_ref()
                    .and_then(|context| context.store.as_ref())
                    .map(|store| store.id)
                    .unwrap_or_default();
                Ok(RegionInfo {
                    Region: location.location.region,
                    Meta: Some(RegionMeta {
                        id: location.location.region.id,
                        peers: location.location.peer.iter().map(|peer| peer.id).collect(),
                    }),
                    Ranges: location.ranges,
                    AllStores: self
                        .backend
                        .all_valid_tiflash_store_ids(location.location.region, primary),
                    PartitionIndex: partition_index,
                })
            })
            .collect()
    }

    fn fetch_topology(&self) -> BatchResult<Vec<String>> {
        self.backend.fetch_topology()
    }

    fn compute_stores(&self) -> BatchResult<Vec<RegionStore>> {
        self.backend.compute_stores()
    }

    /// 委托 backend 探活。
    fn is_store_alive(&self, address: &str, ttl: Duration) -> bool {
        self.backend.is_store_alive(address, ttl)
    }

    /// 按是否 MPP 选择 TiKV/TiFlash RPC 上下文。
    fn rpc_context(&self, region: RegionVerId, is_mpp: bool) -> BatchResult<Option<RpcContext>> {
        self.backend.tiflash_rpc_context(region, is_mpp)
    }

    /// 有效 store ID（MPP 场景）。
    fn all_valid_store_ids(&self, region: RegionVerId, primary_store_id: u64) -> Vec<u64> {
        self.backend
            .all_valid_tiflash_store_ids(region, primary_store_id)
    }

    fn all_tiflash_stores(&self) -> Vec<RegionStore> {
        self.backend.all_tiflash_stores()
    }

    /// BatchTaskSource：失效 Region。
    fn invalidate_region(&self, region: RegionVerId) {
        self.backend.invalidate_region(region);
    }
}

#[allow(non_camel_case_types)]
/// Go 风格类型别名。
pub type bucketSplitFallbackInfo = BucketSplitFallbackInfo;
#[allow(non_upper_case_globals)]
/// Go 风格常量别名。
pub const UnspecifiedLimit: isize = UNSPECIFIED_LIMIT;

#[allow(non_snake_case)]
/// Go 风格构造：返回堆上 RegionCache。
pub fn NewRegionCache(backend: Arc<dyn RegionCacheBackend>) -> Box<RegionCache> {
    Box::new(RegionCache::new(backend))
}

#[allow(dead_code)]
/// 编译期保留对 ReplicaReadPolicy 的引用，避免未使用告警。
fn _assert_replica_policy(_: ReplicaReadPolicy) {}
