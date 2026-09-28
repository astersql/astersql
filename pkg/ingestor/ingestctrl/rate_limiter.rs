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

// Ingest 请求的按 Store 限速器：令牌桶速率限制 + 在途并发上限。
//
// 向 TiKV Store 发起 ingest/写请求时，用 `TokenBucket` 控制 QPS，
// 并用条件变量限制同一 Store 上的在途请求数，避免打满 Store。

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{CancellationToken, Error, Result};

/// 速率换算乘数：将“每秒请求数”放大为令牌粒度（毫秒级）。
pub const ratePerSecMultiplier: usize = 1000;

/// 经典令牌桶：按速率补充令牌，burst 为桶容量。
pub(crate) struct TokenBucket {
    /// 每秒补充的令牌数。
    rate: f64,
    /// 桶容量（突发上限）。
    burst: f64,
    /// 当前可用令牌。
    tokens: f64,
    /// 上次 refill 时间。
    updated: Instant,
}

impl TokenBucket {
    /// 创建满桶的令牌桶。
    pub(crate) fn new(rate: f64, burst: usize) -> Self {
        Self {
            rate,
            burst: burst as f64,
            tokens: burst as f64,
            updated: Instant::now(),
        }
    }

    /// 热更新速率与容量，先 refill 再截断多余令牌。
    pub(crate) fn update(&mut self, rate: f64, burst: usize) {
        self.refill();
        self.rate = rate;
        self.burst = burst as f64;
        self.tokens = self.tokens.min(self.burst);
    }

    /// 按经过时间向桶中补充令牌，不超过 burst。
    fn refill(&mut self) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.updated).as_secs_f64() * self.rate)
            .min(self.burst);
        self.updated = now;
    }

    /// 尝试预留 `count` 个令牌；立即可得返回 `None`，否则返回需等待的时长。
    pub(crate) fn reserve_delay(&mut self, count: usize) -> Result<Option<Duration>> {
        if count as f64 > self.burst {
            return Err(Error::InvalidArgument(format!(
                "requested {count} tokens exceeds burst {}",
                self.burst
            )));
        }
        self.refill();
        if self.tokens >= count as f64 {
            self.tokens -= count as f64;
            return Ok(None);
        }
        if self.rate <= 0.0 {
            return Err(Error::InvalidArgument("rate limiter has zero rate".into()));
        }
        // 令牌不足：返回补齐所需等待时间（调用方短睡重试）
        Ok(Some(Duration::from_secs_f64(
            (count as f64 - self.tokens) / self.rate,
        )))
    }
}

/// 单个 Store 的在途计数与令牌桶。
struct ingestLimiterPerStore {
    /// 当前在途请求占用的并发槽位数。
    in_flight: Mutex<usize>,
    /// 释放槽位时唤醒等待者。
    available: Condvar,
    /// 该 Store 的速率令牌桶。
    limiter: Mutex<TokenBucket>,
}

/// 按 Store 聚合的 ingest 限速器。
pub struct ingestLimiter {
    /// 取消令牌：等待期间可中断。
    token: CancellationToken,
    /// 单 Store 最大在途请求数；0 表示不限制并发。
    maxReqInFlight: usize,
    /// 单 Store 最大 QPS；0 表示不限制速率。
    maxReqPerSec: f64,
    /// storeID → 每 Store 限速状态。
    limiters: Mutex<HashMap<u64, Arc<ingestLimiterPerStore>>>,
}

/// 创建 ingest 限速器；负的并发上限会被钳为 0。
pub fn newIngestLimiter(
    token: CancellationToken,
    maxReqInFlight: i32,
    maxReqPerSec: f64,
) -> ingestLimiter {
    ingestLimiter {
        token,
        maxReqInFlight: maxReqInFlight.max(0) as usize,
        maxReqPerSec: maxReqPerSec.max(0.0),
        limiters: Mutex::new(HashMap::new()),
    }
}

impl ingestLimiter {
    /// 惰性获取或创建指定 Store 的限速状态。
    fn per_store(&self, storeID: u64) -> Result<Arc<ingestLimiterPerStore>> {
        let mut stores = self.limiters.lock().map_err(|_| Error::Poisoned)?;
        Ok(Arc::clone(stores.entry(storeID).or_insert_with(|| {
            Arc::new(ingestLimiterPerStore {
                in_flight: Mutex::new(0),
                available: Condvar::new(),
                limiter: Mutex::new(TokenBucket::new(
                    getEventLimit(self.maxReqPerSec),
                    getRateBurst(self.maxReqPerSec) * ratePerSecMultiplier,
                )),
            })
        })))
    }

    /// 获取 `n` 个速率令牌与并发槽位；可因取消或参数非法失败。
    pub fn Acquire(&self, storeID: u64, n: usize) -> Result<()> {
        if self.NoLimit() || n == 0 {
            return Ok(());
        }
        let per_store = self.per_store(storeID)?;
        // 先按 QPS 等待令牌
        if self.maxReqPerSec > 0.0 {
            loop {
                self.token.check()?;
                let delay = per_store
                    .limiter
                    .lock()
                    .map_err(|_| Error::Poisoned)?
                    .reserve_delay(n.saturating_mul(ratePerSecMultiplier))?;
                let Some(delay) = delay else { break };
                std::thread::sleep(delay.min(Duration::from_millis(10)));
            }
        }
        // 再占在途并发槽
        if self.maxReqInFlight > 0 {
            let mut in_flight = per_store.in_flight.lock().map_err(|_| Error::Poisoned)?;
            while n > self.maxReqInFlight - *in_flight {
                self.token.check()?;
                let (guard, _) = per_store
                    .available
                    .wait_timeout(in_flight, Duration::from_millis(10))
                    .map_err(|_| Error::Poisoned)?;
                in_flight = guard;
            }
            *in_flight += n;
        }
        Ok(())
    }

    /// 释放 `n` 个在途槽位并唤醒等待者。
    pub fn Release(&self, storeID: u64, n: usize) {
        if self.maxReqInFlight == 0 {
            return;
        }
        let Ok(stores) = self.limiters.lock() else {
            return;
        };
        let Some(per_store) = stores.get(&storeID).cloned() else {
            return;
        };
        drop(stores);
        if let Ok(mut in_flight) = per_store.in_flight.lock() {
            *in_flight = in_flight
                .checked_sub(n)
                .expect("released more slots than held");
            per_store.available.notify_all();
        }
    }

    /// 返回有效 burst：并发与速率 burst 的组合（取较小者或无限制）。
    pub fn Burst(&self) -> usize {
        match (self.maxReqInFlight, self.maxReqPerSec > 0.0) {
            (0, false) => usize::MAX,
            (0, true) => getRateBurst(self.maxReqPerSec),
            (concurrency, false) => concurrency,
            (concurrency, true) => concurrency.min(getRateBurst(self.maxReqPerSec)),
        }
    }

    /// 并发与速率均未配置时视为无限速。
    pub fn NoLimit(&self) -> bool {
        self.maxReqInFlight == 0 && self.maxReqPerSec == 0.0
    }
}

/// 将请求速率转换为内部毫秒令牌速率，与 Go 的整数截断及最小值一致。
pub(crate) fn getEventLimit(ratePerSec: f64) -> f64 {
    (ratePerSec * ratePerSecMultiplier as f64).trunc().max(1.0)
}

/// 由每秒速率推导 burst（向上取整，至少为 1）。
pub fn getRateBurst(ratePerSec: f64) -> usize {
    (ratePerSec.ceil() as usize).max(1)
}
