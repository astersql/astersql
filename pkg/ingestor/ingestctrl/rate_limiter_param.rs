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

// Ingest 限速相关系统参数的默认值、持久化加载与全局原子缓存。
//
// 从参数存储读取批次切分大小、切分速率、ingest 并发与 QPS；
// 缺失时写入默认值，并同步到进程内原子变量供运行时读取。

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use crate::Result;

/// 单次批量 split ranges 的默认上限。
const DEFAULT_MAX_BATCH_SPLIT_RANGES: i32 = 2048;
/// 每秒 split ranges 的默认速率（0 表示不限速）。
const DEFAULT_SPLIT_RANGES_PER_SECOND: f64 = 0.0;
/// ingest 默认最大在途并发（0 表示不限制）。
const DEFAULT_MAX_INGEST_IN_FLIGHT: i32 = 0;
/// ingest 默认最大 QPS（0 表示不限制）。
const DEFAULT_MAX_INGEST_PER_SECOND: f64 = 0.0;

/// 当前批量 split ranges 上限的全局缓存。
pub static CurrentMaxBatchSplitRanges: AtomicI32 = AtomicI32::new(0);
/// 当前每秒 split ranges 上限的全局缓存（`f64` 按 bits 存储）。
pub static CurrentMaxSplitRangesPerSec: AtomicU64 = AtomicU64::new(0);
/// 当前 ingest 最大在途并发的全局缓存。
pub static CurrentMaxIngestInflight: AtomicI32 = AtomicI32::new(0);
/// 当前 ingest 最大 QPS 的全局缓存（`f64` 按 bits 存储）。
pub static CurrentMaxIngestPerSec: AtomicU64 = AtomicU64::new(0);

/// 限速参数的持久化存储抽象（读写 ingest 相关系统变量）。
pub trait RateLimiterParamStore {
    /// 读取批量 split ranges 上限；`None` 表示尚未配置。
    fn GetIngestMaxBatchSplitRanges(&mut self) -> Result<Option<i32>>;
    /// 写入批量 split ranges 上限。
    fn SetIngestMaxBatchSplitRanges(&mut self, value: i32) -> Result<()>;
    /// 读取每秒 split ranges 上限。
    fn GetIngestMaxSplitRangesPerSec(&mut self) -> Result<Option<f64>>;
    /// 写入每秒 split ranges 上限。
    fn SetIngestMaxSplitRangesPerSec(&mut self, value: f64) -> Result<()>;
    /// 读取 ingest 最大在途并发。
    fn GetIngestMaxInflight(&mut self) -> Result<Option<i32>>;
    /// 写入 ingest 最大在途并发。
    fn SetIngestMaxInflight(&mut self, value: i32) -> Result<()>;
    /// 读取 ingest 最大 QPS。
    fn GetIngestMaxPerSec(&mut self) -> Result<Option<f64>>;
    /// 写入 ingest 最大 QPS。
    fn SetIngestMaxPerSec(&mut self, value: f64) -> Result<()>;
}

/// 从存储初始化限速参数：缺失则写默认，并刷新全局原子缓存。
pub fn InitializeRateLimiterParam(store: &mut dyn RateLimiterParamStore) -> Result<()> {
    let mut batch = store.GetIngestMaxBatchSplitRanges()?;
    if batch.is_none() {
        store.SetIngestMaxBatchSplitRanges(DEFAULT_MAX_BATCH_SPLIT_RANGES)?;
        batch = Some(DEFAULT_MAX_BATCH_SPLIT_RANGES);
    }
    // 存盘值为 0 时回退到默认批次上限
    CurrentMaxBatchSplitRanges.store(
        batch
            .filter(|value| *value != 0)
            .unwrap_or(DEFAULT_MAX_BATCH_SPLIT_RANGES),
        Ordering::Release,
    );

    let mut split = store.GetIngestMaxSplitRangesPerSec()?;
    if split.is_none() {
        store.SetIngestMaxSplitRangesPerSec(DEFAULT_SPLIT_RANGES_PER_SECOND)?;
        split = Some(DEFAULT_SPLIT_RANGES_PER_SECOND);
    }
    CurrentMaxSplitRangesPerSec.store(split.unwrap_or_default().to_bits(), Ordering::Release);

    let mut in_flight = store.GetIngestMaxInflight()?;
    if in_flight.is_none() {
        store.SetIngestMaxInflight(DEFAULT_MAX_INGEST_IN_FLIGHT)?;
        in_flight = Some(DEFAULT_MAX_INGEST_IN_FLIGHT);
    }
    CurrentMaxIngestInflight.store(in_flight.unwrap_or_default(), Ordering::Release);

    let mut ingest = store.GetIngestMaxPerSec()?;
    if ingest.is_none() {
        store.SetIngestMaxPerSec(DEFAULT_MAX_INGEST_PER_SECOND)?;
        ingest = Some(DEFAULT_MAX_INGEST_PER_SECOND);
    }
    CurrentMaxIngestPerSec.store(ingest.unwrap_or_default().to_bits(), Ordering::Release);
    Ok(())
}

/// 读取批量 split ranges 上限；缓存为 0 时返回默认值。
pub fn GetMaxBatchSplitRanges() -> i32 {
    match CurrentMaxBatchSplitRanges.load(Ordering::Acquire) {
        0 => DEFAULT_MAX_BATCH_SPLIT_RANGES,
        value => value,
    }
}
/// 读取每秒 split ranges 上限。
pub fn GetMaxSplitRangePerSec() -> f64 {
    f64::from_bits(CurrentMaxSplitRangesPerSec.load(Ordering::Acquire))
}
/// 读取 ingest 最大在途并发。
pub fn GetMaxIngestConcurrency() -> i32 {
    CurrentMaxIngestInflight.load(Ordering::Acquire)
}
/// 读取 ingest 最大 QPS。
pub fn GetMaxIngestPerSec() -> f64 {
    f64::from_bits(CurrentMaxIngestPerSec.load(Ordering::Acquire))
}
