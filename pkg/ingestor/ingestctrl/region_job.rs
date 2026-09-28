// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Region 作业（RegionJob）的构造、重试队列与按 Store 负载均衡。
//
// Region 是 TiKV 的键空间分片。导入流水线将 KV 数据切成按 Region 的作业，
// 本模块负责：定位区间相交生成作业、ingest 失败后的阶段回退、延时重试堆，
// 以及按 peer 所在 Store 负载挑选作业。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::job_worker::{RegionInfo, RegionJob, RegionJobStage};
use crate::{CancellationToken, Error, KeyRange, KvPair, Result};

/// 默认每批写入的 KV 条数。
pub const defaultKVBatchCount: usize = 512;

/// 已定位到具体 Region 元信息与键范围的区域描述。
#[derive(Clone, Debug)]
pub struct LocatedRegion {
    /// Region 元数据（含 peer/Store 信息）。
    pub region: RegionInfo,
    /// 该 Region 覆盖的键区间。
    pub key_range: KeyRange,
}

/// 构造处于 `RegionScanned` 阶段的单个 Region 作业。
pub fn newRegionJob(
    region: RegionInfo,
    data: Vec<KvPair>,
    jobStart: Vec<u8>,
    jobEnd: Vec<u8>,
    regionSplitSize: i64,
    regionSplitKeys: i64,
) -> RegionJob {
    let mut job = RegionJob::default();
    job.stage = RegionJobStage::RegionScanned;
    job.region = region;
    job.key_range = KeyRange {
        start: jobStart,
        end: jobEnd,
    };
    job.data = data;
    job.region_split_size = regionSplitSize;
    job.region_split_keys = regionSplitKeys;
    job
}

/// 将已排序的 Region 列表与作业键区间相交，生成若干 RegionJob。
///
/// 双指针扫描：跳过完全在作业区间左侧的 Region，截取相交子区间构造作业。
pub fn newRegionJobs(
    sortedRegions: &[LocatedRegion],
    data: &[KvPair],
    sortedJobRanges: &[KeyRange],
    regionSplitSize: i64,
    regionSplitKeys: i64,
) -> Vec<RegionJob> {
    let mut jobs = Vec::with_capacity(sortedRegions.len().max(sortedJobRanges.len()) * 2);
    let mut region_index = 0usize;
    for job_range in sortedJobRanges {
        while let Some(region) = sortedRegions.get(region_index) {
            // Region 完全在作业区间左侧：推进 Region 指针
            if !region.key_range.end.is_empty() && region.key_range.end <= job_range.start {
                region_index += 1;
                continue;
            }
            // Region 完全在作业区间右侧：处理下一作业区间
            if !job_range.end.is_empty() && region.key_range.start >= job_range.end {
                break;
            }
            // 取相交子区间 [max(starts), min(ends))
            let start = if job_range.start > region.key_range.start {
                job_range.start.clone()
            } else {
                region.key_range.start.clone()
            };
            let end = minEnd(&job_range.end, &region.key_range.end);
            if end.is_empty() || start < end {
                jobs.push(newRegionJob(
                    region.region.clone(),
                    data.to_vec(),
                    start,
                    end,
                    regionSplitSize,
                    regionSplitKeys,
                ));
            }
            // 作业区间被当前 Region 覆盖完则跳出；否则推进到下一 Region
            if region.key_range.end.is_empty()
                || (!job_range.end.is_empty() && job_range.end <= region.key_range.end)
            {
                break;
            }
            region_index += 1;
        }
    }
    jobs
}

/// 取两个结束键的较小者；空切片表示正无穷。
fn minEnd(a: &[u8], b: &[u8]) -> Vec<u8> {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Vec::new(),
        (true, false) => b.to_vec(),
        (false, true) => a.to_vec(),
        (false, false) if a <= b => a.to_vec(),
        (false, false) => b.to_vec(),
    }
}

/// 发往 TiKV 的写请求元信息封装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteRequest {
    /// 序列化后的写元数据。
    pub meta: Vec<u8>,
    /// 资源组名称（Resource Control）。
    pub resource_group_name: String,
    /// 任务类型标识。
    pub task_type: String,
    /// 请求来源（固定 `internal_lightning:{taskType}`）。
    pub request_source: String,
    /// 事务来源标记。
    pub txn_source: u64,
}

/// 构造 Lightning 内部写请求，`txn_source` 固定为 1。
pub fn newWriteRequest(meta: Vec<u8>, resourceGroupName: &str, taskType: &str) -> WriteRequest {
    WriteRequest {
        meta,
        resource_group_name: resourceGroupName.to_owned(),
        task_type: taskType.to_owned(),
        request_source: format!("internal_lightning:{taskType}"),
        txn_source: 1,
    }
}

/// 根据 ingest 错误信息决定作业应回退到的阶段。
///
/// - `KVIngestFailed` → 从扫描阶段重来；
/// - `ServerIsBusy` / `RequestTooNew` / 可重试且与 epoch/region 无关 → 从 Wrote 重试；
/// - 其余 → 需要重新扫描 Region（`NeedRescan`）。
pub fn getNextStageOnIngestError(error: &Error) -> RegionJobStage {
    let message = error.to_string();
    if message.contains("KVIngestFailed") {
        RegionJobStage::RegionScanned
    } else if message.contains("ServerIsBusy") || message.contains("RequestTooNew") {
        RegionJobStage::Wrote
    } else if matches!(error, Error::Retryable(_) | Error::Timeout)
        && !message.contains("epoch")
        && !message.contains("region")
    {
        RegionJobStage::Wrote
    } else {
        RegionJobStage::NeedRescan
    }
}

/// 延时重试堆中的条目：按 `wait_until` 最早优先（小顶堆语义）。
#[derive(Debug)]
struct RetryEntry {
    wait_until: Instant,
    sequence: u64,
    job: RegionJob,
}

impl PartialEq for RetryEntry {
    fn eq(&self, other: &Self) -> bool {
        self.wait_until == other.wait_until && self.sequence == other.sequence
    }
}
impl Eq for RetryEntry {}
impl PartialOrd for RetryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RetryEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap 是大顶堆：反转比较使最早到期的在堆顶
        other
            .wait_until
            .cmp(&self.wait_until)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

/// Region 作业的延时重试队列：按到期时间弹出，支持关闭与清理。
pub struct regionJobRetryer {
    queue: Mutex<BinaryHeap<RetryEntry>>,
    reload: Condvar,
    closed: AtomicBool,
    sequence: AtomicU64,
}

impl Default for regionJobRetryer {
    fn default() -> Self {
        Self {
            queue: Mutex::new(BinaryHeap::new()),
            reload: Condvar::new(),
            closed: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
        }
    }
}

impl regionJobRetryer {
    /// 将作业推入延时队列；已关闭时返回 false。
    pub fn push(&self, job: RegionJob, waitUntil: Instant) -> bool {
        if self.closed.load(AtomicOrdering::Acquire) {
            return false;
        }
        let Ok(mut queue) = self.queue.lock() else {
            return false;
        };
        // 持锁后再检 closed，避免关闭窗口丢作业却仍入队
        if self.closed.load(AtomicOrdering::Acquire) {
            return false;
        }
        queue.push(RetryEntry {
            wait_until: waitUntil,
            sequence: self.sequence.fetch_add(1, AtomicOrdering::Relaxed),
            job,
        });
        self.reload.notify_one();
        true
    }

    /// 弹出一个已到期的作业；未到期则短超时等待，可被取消或关闭中断。
    pub fn popReady(&self, token: &CancellationToken) -> Result<Option<RegionJob>> {
        let mut queue = self.queue.lock().map_err(|_| Error::Poisoned)?;
        loop {
            token.check()?;
            if self.closed.load(AtomicOrdering::Acquire) {
                return Ok(None);
            }
            let Some(next) = queue.peek() else {
                return Ok(None);
            };
            let now = Instant::now();
            if next.wait_until <= now {
                return Ok(queue.pop().map(|entry| entry.job));
            }
            let timeout = next.wait_until.saturating_duration_since(now);
            let (next_queue, _) = self
                .reload
                .wait_timeout(queue, timeout.min(Duration::from_millis(10)))
                .map_err(|_| Error::Poisoned)?;
            queue = next_queue;
        }
    }

    /// 关闭队列并唤醒所有等待者。
    pub fn close(&self) {
        self.closed.store(true, AtomicOrdering::Release);
        self.reload.notify_all();
    }

    /// 关闭并取出所有尚未处理的作业。
    pub fn cleanupUnprocessedJobs(&self) -> Vec<RegionJob> {
        self.closed.store(true, AtomicOrdering::Release);
        let Ok(mut queue) = self.queue.lock() else {
            return Vec::new();
        };
        queue.drain().map(|entry| entry.job).collect()
    }
}

/// 按 Region peer 所在 Store 的当前负载挑选作业的均衡器。
#[derive(Default)]
pub struct storeBalancer {
    /// 待调度作业列表（带入队序号）。
    jobs: Mutex<Vec<(u64, RegionJob)>>,
    /// storeID → 当前负载（进行中作业数）。
    storeLoadMap: Mutex<HashMap<u64, usize>>,
    /// 入队序号生成器。
    nextJobIndex: AtomicU64,
}

impl storeBalancer {
    /// 将作业加入待调度队列。
    pub fn push(&self, job: RegionJob) -> Result<()> {
        let index = self.nextJobIndex.fetch_add(1, AtomicOrdering::Relaxed);
        self.jobs
            .lock()
            .map_err(|_| Error::Poisoned)?
            .push((index, job));
        Ok(())
    }

    /// 返回待调度作业数量。
    pub fn jobLen(&self) -> usize {
        self.jobs.lock().map(|jobs| jobs.len()).unwrap_or_default()
    }

    /// 选出 peer Store 负载之和最小的作业，并增加对应 Store 负载。
    pub fn pickJob(&self) -> Result<Option<RegionJob>> {
        let mut jobs = self.jobs.lock().map_err(|_| Error::Poisoned)?;
        let mut loads = self.storeLoadMap.lock().map_err(|_| Error::Poisoned)?;
        let Some((best_position, _)) = jobs.iter().enumerate().min_by_key(|(_, (_, job))| {
            job.region
                .peer_store_ids
                .iter()
                .map(|store| loads.get(store).copied().unwrap_or_default())
                .sum::<usize>()
        }) else {
            return Ok(None);
        };
        let (_, job) = jobs.swap_remove(best_position);
        for store in &job.region.peer_store_ids {
            *loads.entry(*store).or_default() += 1;
        }
        Ok(Some(job))
    }

    /// 作业完成后减少相关 Store 的负载计数。
    pub fn releaseStoreLoad(&self, peers: &[u64]) -> Result<()> {
        let mut loads = self.storeLoadMap.lock().map_err(|_| Error::Poisoned)?;
        for store in peers {
            let Some(load) = loads.get_mut(store) else {
                // Go records the invariant violation and continues so a stale or
                // malformed peer cannot prevent the remaining loads from being
                // released. Keep the same best-effort cleanup contract here.
                continue;
            };
            *load = load.saturating_sub(1);
        }
        Ok(())
    }

    /// 查询指定 Store 的当前负载。
    pub fn storeLoad(&self, storeID: u64) -> usize {
        self.storeLoadMap
            .lock()
            .ok()
            .and_then(|loads| loads.get(&storeID).copied())
            .unwrap_or_default()
    }
}
