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

// local 后端辅助：Region 批量切分/打散、键范围判定、按 Store 写限速与压缩阈值估算。
//
// Region 是 TiKV 的键空间分片；导入前常需按 split key 切分并 scatter 到各 Store。
// 本模块提供粗粒度预切分、令牌桶限速，以及按 Store 的写吞吐限制与 compaction 阈值。

use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::rate_limiter::{TokenBucket, getRateBurst, ratePerSecMultiplier};
use crate::{CancellationToken, Error, KeyRange, Result};

/// 超过该数量的 split key 时先走粗粒度预切分，再做细粒度切分。
pub const coarseGrainedSplitKeysThreshold: usize = 64;

/// 批量 SplitAndScatter 客户端抽象（Region 切分并打散到各 Store）。
pub trait BatchSplitClient: Send + Sync {
    /// 对给定 split keys 执行切分与 scatter。
    fn SplitAndScatter(&self, token: &CancellationToken, keys: &[Vec<u8>]) -> Result<()>;
}

/// 按批次切分并 scatter Region，可选按每秒次数限速。
///
/// 当 split key 数量超过 `coarseGrainedSplitKeysThreshold` 时，先对稀疏子集
/// 做一轮粗切分，再对全量 keys 做细切分，降低单次 Region 过大风险。
pub fn splitAndScatterRegionInBatches(
    client: &dyn BatchSplitClient,
    token: &CancellationToken,
    splitKeys: &[Vec<u8>],
    mut batchCnt: usize,
    maxCntPerSec: f64,
) -> Result<()> {
    if batchCnt == 0 {
        return Err(Error::InvalidArgument(
            "split batch count must be positive".into(),
        ));
    }
    // 正速率时构造令牌桶，并将单批大小限制在 burst 内
    let limiter = if maxCntPerSec > 0.0 {
        batchCnt = batchCnt.min(getRateBurst(maxCntPerSec));
        Some(Mutex::new(TokenBucket::new(
            maxCntPerSec * ratePerSecMultiplier as f64,
            getRateBurst(maxCntPerSec) * ratePerSecMultiplier,
        )))
    } else {
        None
    };
    if splitKeys.len() > coarseGrainedSplitKeysThreshold {
        // 粗粒度预切分：用稀疏 key 先把大 Region 切开
        splitAndScatterRegionInBatchesWithLimiter(
            client,
            token,
            &getCoarseGrainedSplitKeys(splitKeys),
            batchCnt,
            limiter.as_ref(),
        )?;
    }
    // 细粒度：对全部 split keys 再切一轮
    splitAndScatterRegionInBatchesWithLimiter(client, token, splitKeys, batchCnt, limiter.as_ref())
}

/// 从全量 split keys 按约 √n 步长抽样，并保证包含最后一个 key。
pub fn getCoarseGrainedSplitKeys(splitKeys: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if splitKeys.is_empty() {
        return Vec::new();
    }
    let step = (splitKeys.len() as f64).sqrt().floor().max(1.0) as usize;
    let mut keys: Vec<_> = splitKeys.iter().step_by(step).cloned().collect();
    // step_by 未必选中末尾，补上最后一个 key 以覆盖尾部区间
    if keys.last() != splitKeys.last() {
        keys.push(splitKeys.last().expect("non-empty").clone());
    }
    keys
}

/// 按 `batchCnt` 分批调用 SplitAndScatter，并在有 limiter 时等待令牌。
fn splitAndScatterRegionInBatchesWithLimiter(
    client: &dyn BatchSplitClient,
    token: &CancellationToken,
    splitKeys: &[Vec<u8>],
    batchCnt: usize,
    limiter: Option<&Mutex<TokenBucket>>,
) -> Result<()> {
    for batch in splitKeys.chunks(batchCnt) {
        if let Some(limiter) = limiter {
            // 循环预留令牌，短睡等待，并响应取消
            loop {
                token.check()?;
                let delay = limiter
                    .lock()
                    .map_err(|_| Error::Poisoned)?
                    .reserve_delay(batch.len() * ratePerSecMultiplier)?;
                let Some(delay) = delay else { break };
                std::thread::sleep(delay.min(Duration::from_millis(10)));
            }
        }
        client.SplitAndScatter(token, batch)?;
    }
    Ok(())
}

/// 判断 `key` 是否严格在 `end` 之前；空 `end` 表示正无穷。
pub fn beforeEnd(key: &[u8], end: &[u8]) -> bool {
    end.is_empty() || key < end
}

/// 判断 `key` 是否落在 Region 的半开区间 `[start, end)` 内。
pub fn keyInsideRegion(region: &KeyRange, key: &[u8]) -> bool {
    key >= region.start.as_slice() && beforeEnd(key, &region.end)
}

/// 判断若干 meta 区间的起止键是否都落在指定 Region 内。
pub fn insideRegion(region: &KeyRange, metas: &[KeyRange]) -> bool {
    metas
        .iter()
        .all(|range| keyInsideRegion(region, &range.start) && keyInsideRegion(region, &range.end))
}

/// 返回两个起始键中较大的一个（字典序）。
pub fn largerStartKey<'a>(a: &'a [u8], b: &'a [u8]) -> &'a [u8] {
    if a > b { a } else { b }
}

/// 按 Store 维度的写入限速接口。
pub trait StoreWriteLimiter: Send + Sync {
    /// 在指定 Store 上等待获取 `n` 个写令牌。
    fn WaitN(&self, token: &CancellationToken, storeID: u64, n: isize) -> Result<()>;
    /// 当前每秒写限制（0 表示不限速）。
    fn Limit(&self) -> isize;
    /// 热更新写限制；会同步刷新已有 Store 的令牌桶。
    fn UpdateLimit(&self, limit: isize);
}

/// 按 Store ID 维护独立令牌桶的写限速器。
pub struct storeWriteLimiter {
    /// storeID → 令牌桶。
    limiters: RwLock<HashMap<u64, Arc<Mutex<TokenBucket>>>>,
    /// 全局速率（令牌/秒）。
    limit: AtomicIsize,
    /// 突发容量（约为 limit 的 1.2 倍）。
    burst: AtomicIsize,
}

/// 按写限制创建按 Store 的写限速器。
pub fn newStoreWriteLimiter(limit: isize) -> storeWriteLimiter {
    let (limit, burst) = calculateLimitAndBurst(limit);
    storeWriteLimiter {
        limiters: RwLock::new(HashMap::new()),
        limit: AtomicIsize::new(limit),
        burst: AtomicIsize::new(burst),
    }
}

/// 由写限制推导速率与 burst（burst = limit + limit/5）；非正限制视为不限速。
pub fn calculateLimitAndBurst(writeLimit: isize) -> (isize, isize) {
    if writeLimit <= 0 {
        return (0, 0);
    }
    (writeLimit, writeLimit.saturating_add(writeLimit / 5))
}

impl storeWriteLimiter {
    /// 惰性创建并返回指定 Store 的令牌桶；limit 为 0 时返回 None。
    fn getLimiter(&self, storeID: u64) -> Result<Option<Arc<Mutex<TokenBucket>>>> {
        let limit = self.limit.load(Ordering::Acquire);
        if limit == 0 {
            return Ok(None);
        }
        if let Some(limiter) = self
            .limiters
            .read()
            .map_err(|_| Error::Poisoned)?
            .get(&storeID)
        {
            return Ok(Some(Arc::clone(limiter)));
        }
        // 双检：写锁下插入缺失的 Store 桶
        let mut limiters = self.limiters.write().map_err(|_| Error::Poisoned)?;
        Ok(Some(Arc::clone(limiters.entry(storeID).or_insert_with(
            || {
                Arc::new(Mutex::new(TokenBucket::new(
                    limit as f64,
                    self.burst.load(Ordering::Acquire) as usize,
                )))
            },
        ))))
    }
}

impl StoreWriteLimiter for storeWriteLimiter {
    fn WaitN(&self, token: &CancellationToken, storeID: u64, mut n: isize) -> Result<()> {
        let Some(limiter) = self.getLimiter(storeID)? else {
            return Ok(());
        };
        // 每次最多取 burst 个令牌，循环直到凑满 n
        while n > 0 {
            token.check()?;
            let burst = self.burst.load(Ordering::Acquire).max(1);
            let count = n.min(burst) as usize;
            loop {
                let delay = limiter
                    .lock()
                    .map_err(|_| Error::Poisoned)?
                    .reserve_delay(count)?;
                let Some(delay) = delay else { break };
                token.check()?;
                std::thread::sleep(delay.min(Duration::from_millis(10)));
            }
            n -= count as isize;
        }
        Ok(())
    }

    fn Limit(&self) -> isize {
        self.limit.load(Ordering::Acquire)
    }

    fn UpdateLimit(&self, newLimit: isize) {
        let (limit, burst) = calculateLimitAndBurst(newLimit);
        // 速率未变则跳过刷新
        if self.limit.swap(limit, Ordering::AcqRel) == limit {
            return;
        }
        self.burst.store(burst, Ordering::Release);
        let Ok(mut limiters) = self.limiters.write() else {
            return;
        };
        if limit == 0 {
            // 关闭限速时清空各 Store 桶
            limiters.clear();
            return;
        }
        for limiter in limiters.values() {
            if let Ok(mut limiter) = limiter.lock() {
                limiter.update(limit as f64, burst as usize);
            }
        }
    }
}

/// 压缩（compaction）阈值下界：512 MiB。
pub const CompactionLowerThreshold: i64 = 512 * 1024 * 1024;
/// 压缩阈值上界：32 GiB。
pub const CompactionUpperThreshold: i64 = 32 * 1024 * 1024 * 1024;

/// 按原始文件总大小估算 compaction 阈值，落在上下界之间且取最近 2 的幂。
pub fn EstimateCompactionThreshold2(totalRawFileSize: i64) -> i64 {
    let base = (totalRawFileSize / 512).max(1) as u64;
    let threshold = base.checked_next_power_of_two().unwrap_or(u64::MAX) as i64;
    threshold.clamp(CompactionLowerThreshold, CompactionUpperThreshold)
}
