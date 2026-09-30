// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// RU 窗口聚合器：按时间桶收集 TopRU 增量并在上报窗口边界压缩输出。
//
// 将到来的 `RUBatch` 对齐到基础桶（默认 15s），在窗口（默认 60s）结束时
// 取出记录、按用户/SQL Top-N 压缩，并处理迟到数据与版本切换（handover）。
// RU（Request Unit）是 TiDB 资源计量单位。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ru_datamodel::{
    maxPreTopNSQLsPerUser, maxPreTopNUsers, maxTopSQLsPerUser, maxTopUsers,
    newRUCollectingWithCaps, ruCollecting,
};
use crate::{stmtstats, tipb_protobuf as tipb};
use reporter_metrics::reporter_metrics as metrics;

/// 基础时间桶长度（秒），RU 增量按此对齐聚合。
pub const ruBaseBucketSeconds: u64 = 15;
/// 上报窗口长度（秒）；窗口结束才可 take 报告。
pub const ruReportWindowSeconds: u64 = 60;
/// 最终上报保留的 Top 用户数。
pub const ruReportTopNUsers: usize = 100;
/// 每个用户最终上报保留的 Top SQL 数。
pub const ruReportTopNSQLsPerUser: usize = 100;

/// 一批带时间戳与版本的 RU 增量映射。
#[derive(Clone, Debug)]
pub struct RUBatch {
    /// (用户, SQL digest, 计划 digest) → RU 增量。
    pub data: stmtstats::RUIncrementMap,
    /// 批次时间戳（秒）。
    pub timestamp: u64,
    /// RU 版本；与当前版本不一致则丢弃。
    pub version: stmtstats::RUVersion,
}

/// Go 风格类型别名。
pub type ruBatch = RUBatch;

/// 单个时间点桶：活跃收集或已压缩结果。
struct RUPointBucket {
    /// 尚未旋转压缩的收集器。
    collecting: Option<Box<ruCollecting>>,
    /// 旋转后保留的 Top-N 压缩结果。
    compacted: Option<Box<ruCollecting>>,
    /// 桶起始时间（已对齐）。
    start: u64,
}

/// 聚合器可变状态，由 Mutex 保护。
#[derive(Default)]
struct AggregatorState {
    /// 按桶起始时间索引的点桶。
    buckets: HashMap<u64, RUPointBucket>,
    /// 当前接受的 RU 版本。
    currentVersion: stmtstats::RUVersion,
    /// handover 后丢弃此时间之前的数据。
    dropUntilTs: u64,
    /// 上次已上报窗口的结束时间。
    lastReportedEndTs: u64,
}

/// 线程安全的 RU 窗口聚合器。
pub struct RUWindowAggregator {
    state: Mutex<AggregatorState>,
    /// 因迟到且目标桶已关闭而丢弃的键数量。
    droppedLateKeys: AtomicU64,
    /// 丢弃的 RU 总量，以 f64 位模式存于原子整数。
    droppedLateRUBits: AtomicU64,
}

/// Go 风格类型别名。
pub type ruWindowAggregator = RUWindowAggregator;

impl Default for RUWindowAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// 创建堆分配的窗口聚合器。
pub fn newRUWindowAggregator() -> Box<RUWindowAggregator> {
    Box::new(RUWindowAggregator::new())
}

/// 将时间戳向下对齐到 `interval` 边界；`interval==0` 时原样返回。
pub fn alignToInterval(timestamp: u64, interval: u64) -> u64 {
    if interval == 0 {
        timestamp
    } else {
        timestamp - timestamp % interval
    }
}

impl RUWindowAggregator {
    /// 创建空聚合器。
    pub fn new() -> Self {
        Self {
            state: Mutex::new(AggregatorState::default()),
            droppedLateKeys: AtomicU64::new(0),
            droppedLateRUBits: AtomicU64::new(0.0_f64.to_bits()),
        }
    }

    /// 蛇形命名入口，转发到 `addBatch`。
    pub fn add_batch(
        &self,
        timestamp: u64,
        data: stmtstats::RUIncrementMap,
        version: stmtstats::RUVersion,
    ) {
        self.addBatch(RUBatch {
            data,
            timestamp,
            version,
        });
    }

    /// 写入一批 RU：版本不符或落在 dropUntilTs 之前则丢弃；
    /// 迟到数据尽量挪到当前窗口起点，若目标桶已压缩则计入丢弃指标。
    pub fn addBatch(&self, batch: RUBatch) {
        if batch.data.is_empty() {
            return;
        }
        let mut bucketStart = alignToInterval(batch.timestamp, ruBaseBucketSeconds);
        let mut state = self.state.lock().expect("RU aggregator mutex poisoned");
        if state.currentVersion == 0 {
            state.currentVersion = stmtstats::NormalizeRUVersion(batch.version);
        }
        // 版本切换或仍在 handover 丢弃区间内：直接忽略。
        if batch.version != state.currentVersion
            || (state.dropUntilTs > 0 && bucketStart < state.dropUntilTs)
        {
            return;
        }

        let mut wasLate = false;
        // 已上报窗口之后的迟到数据：挪到 lastReportedEndTs 作为新桶起点。
        if state.lastReportedEndTs > 0 && bucketStart < state.lastReportedEndTs {
            bucketStart = state.lastReportedEndTs;
            wasLate = true;
        }
        rotateBucketsBefore(&mut state, bucketStart);

        let bucket = state
            .buckets
            .entry(bucketStart)
            .or_insert_with(|| RUPointBucket {
                collecting: Some(newRUCollectingWithCaps(
                    maxPreTopNUsers,
                    maxPreTopNSQLsPerUser,
                )),
                compacted: None,
                start: bucketStart,
            });
        if let Some(collecting) = bucket.collecting.as_mut() {
            collecting.addBatch(bucketStart, batch.data);
            return;
        }
        drop(state);

        // 桶已压缩关闭：迟到批次无法写入，统计丢弃键与 RU。
        if wasLate {
            let droppedRU = batch.data.values().map(|increment| increment.TotalRU).sum();
            self.droppedLateKeys
                .fetch_add(batch.data.len() as u64, Ordering::Relaxed);
            addAtomicF64(&self.droppedLateRUBits, droppedRU);
            incrementLateCompactedMetrics(batch.data.len(), droppedRU);
        }
    }

    /// 蛇形命名入口，转发到 `resetForHandover`。
    pub fn reset_for_handover(&self, version: stmtstats::RUVersion, now: u64) {
        self.resetForHandover(version, now);
    }

    /// 故障切换/移交时重置：清空桶，丢弃至下一完整上报窗口边界。
    pub fn resetForHandover(&self, version: stmtstats::RUVersion, now: u64) {
        let mut state = self.state.lock().expect("RU aggregator mutex poisoned");
        state.currentVersion = version;
        state.buckets.clear();
        state.dropUntilTs = alignToInterval(now, ruReportWindowSeconds);
        // 非窗口整点则再跳过当前未完成窗口。
        if now % ruReportWindowSeconds != 0 {
            state.dropUntilTs += ruReportWindowSeconds;
        }
    }

    /// 蛇形命名入口，转发到 `takeReportRecords`。
    pub fn take_report_records(
        &self,
        now: u64,
        itemInterval: u64,
        keyspaceName: Vec<u8>,
    ) -> Vec<tipb::TopRuRecord> {
        self.takeReportRecords(now, itemInterval, keyspaceName)
    }

    /// Drop closed report windows while retaining buckets in the still-open window.
    pub fn dropReportData(&self, now: u64) {
        let window_end = alignToInterval(now, ruReportWindowSeconds);
        let mut state = self.state.lock().expect("RU aggregator mutex poisoned");
        state
            .buckets
            .retain(|timestamp, _| *timestamp >= window_end);
        state.lastReportedEndTs = state.lastReportedEndTs.max(window_end);
    }

    /// 取出上一完整上报窗口的 TopRU 记录；同一窗口只成功 take 一次。
    pub fn takeReportRecords(
        &self,
        now: u64,
        itemInterval: u64,
        keyspaceName: Vec<u8>,
    ) -> Vec<tipb::TopRuRecord> {
        let windowEnd = alignToInterval(now, ruReportWindowSeconds);
        if windowEnd < ruReportWindowSeconds {
            return Vec::new();
        }
        let buckets = {
            let mut state = self.state.lock().expect("RU aggregator mutex poisoned");
            // 窗口尚未前进：避免重复上报。
            if windowEnd <= state.lastReportedEndTs {
                return Vec::new();
            }
            rotateBucketsBefore(&mut state, windowEnd);
            let windowStart = windowEnd - ruReportWindowSeconds;
            let mut taken =
                HashMap::with_capacity((ruReportWindowSeconds / ruBaseBucketSeconds) as usize);
            let mut timestamp = windowStart;
            while timestamp < windowEnd {
                if let Some(bucket) = state.buckets.remove(&timestamp) {
                    taken.insert(timestamp, bucket);
                }
                timestamp += ruBaseBucketSeconds;
            }
            state.lastReportedEndTs = windowEnd;
            // 丢弃窗口起点之前的过期桶。
            state
                .buckets
                .retain(|timestamp, _| *timestamp >= windowStart);
            taken
        };
        buildReportRecords(
            buckets,
            windowEnd - ruReportWindowSeconds,
            windowEnd,
            itemInterval,
            keyspaceName,
        )
    }

    /// 返回因迟到丢弃的键计数。
    pub fn dropped_late_keys(&self) -> u64 {
        self.droppedLateKeys.load(Ordering::Relaxed)
    }

    /// 返回因迟到丢弃的 RU 总量。
    pub fn dropped_late_ru(&self) -> f64 {
        f64::from_bits(self.droppedLateRUBits.load(Ordering::Relaxed))
    }
}

/// 将起始时间 + 桶长不超过 boundary 的活跃桶压缩为 Top-N。
fn rotateBucketsBefore(state: &mut AggregatorState, boundary: u64) {
    for bucket in state.buckets.values_mut() {
        if bucket.start + ruBaseBucketSeconds > boundary || bucket.collecting.is_none() {
            continue;
        }
        let collecting = bucket.collecting.take().expect("collecting checked");
        bucket.compacted = collecting.compactWithLimits(maxTopUsers, maxTopSQLsPerUser);
    }
}

/// 按 itemInterval（15/30/60）重分组桶并压缩为 tipb TopRuRecord。
fn buildReportRecords(
    buckets: HashMap<u64, RUPointBucket>,
    windowStart: u64,
    windowEnd: u64,
    itemInterval: u64,
    keyspaceName: Vec<u8>,
) -> Vec<tipb::TopRuRecord> {
    // 非法间隔回退到 60 秒，与 Go 侧合法集合一致。
    let itemInterval = match itemInterval {
        15 | 30 | 60 => itemInterval,
        _ => 60,
    };
    let singleInterval = windowEnd - windowStart <= itemInterval;
    let bucketsPerInterval =
        ((itemInterval + ruBaseBucketSeconds - 1) / ruBaseBucketSeconds) as usize;
    let intervalPreCapUsers = bucketsPerInterval * maxTopUsers;
    let intervalPreCapSQLs = bucketsPerInterval * maxTopSQLsPerUser;
    let intervalsPerWindow = ((windowEnd - windowStart + itemInterval - 1) / itemInterval) as usize;
    let mut merged = newRUCollectingWithCaps(
        intervalsPerWindow * ruReportTopNUsers,
        intervalsPerWindow * ruReportTopNSQLsPerUser,
    );

    let mut intervalStart = windowStart;
    while intervalStart < windowEnd {
        let mut interval = newRUCollectingWithCaps(intervalPreCapUsers, intervalPreCapSQLs);
        let mut bucketStart = intervalStart;
        while bucketStart < intervalStart + itemInterval {
            if let Some(compacted) = buckets
                .get(&bucketStart)
                .and_then(|bucket| bucket.compacted.as_deref())
            {
                // 合并时把条目时间戳规范到 intervalStart。
                interval.mergeFrom(Some(compacted), intervalStart, true);
            }
            bucketStart += ruBaseBucketSeconds;
        }
        if let Some(mut compacted) =
            interval.compactWithLimits(ruReportTopNUsers, ruReportTopNSQLsPerUser)
        {
            if singleInterval {
                return compacted.toTopRURecords(keyspaceName);
            }
            merged.mergeFrom(Some(&compacted), 0, false);
        }
        intervalStart += itemInterval;
    }
    merged.toTopRURecords(keyspaceName)
}

/// 无锁累加 f64：以位模式 CAS 更新 AtomicU64。
fn addAtomicF64(destination: &AtomicU64, value: f64) {
    let mut current = destination.load(Ordering::Relaxed);
    loop {
        let next = (f64::from_bits(current) + value).to_bits();
        match destination.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// 与 Go 一致，将无法接收的迟到 RU 同步记录到全局 Prometheus 指标。
#[allow(static_mut_refs)]
fn incrementLateCompactedMetrics(droppedKeys: usize, droppedRU: f64) {
    unsafe {
        if let Some(counter) = metrics::IgnoreLateCompactedRUKeysCounter.as_ref() {
            counter.inc_by(droppedKeys as f64);
        }
        if let Some(counter) = metrics::IgnoreLateCompactedRUTotalCounter.as_ref() {
            counter.inc_by(droppedRU);
        }
    }
}
