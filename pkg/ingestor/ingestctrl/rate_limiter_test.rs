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

// `ingestLimiter` 单元测试：并发槽、QPS 限速、取消与 Burst/NoLimit 组合。
//
// 覆盖同 Store 阻塞/唤醒、跨 Store 隔离，以及等待期间取消令牌生效的路径。

#![allow(non_snake_case)]

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::rate_limiter::{getEventLimit, newIngestLimiter};
use crate::{CancellationToken, Error};

/// 对应 Go 同名测试：同 Store 并发耗尽、Release 唤醒、不同 Store 互不阻塞。
// TestConcurrencyLimit 对应 Go 同名测试，覆盖同一 store 并发耗尽、Release 唤醒、不同 store 不互相阻塞。
#[test]
pub fn TestConcurrencyLimit() {
    let limiter = Arc::new(newIngestLimiter(CancellationToken::default(), 1, 0.0));
    // 占满唯一并发槽后，另一线程 Acquire 应阻塞
    limiter.Acquire(0, 1).unwrap();
    let (tx, rx) = mpsc::channel();
    let waiter = Arc::clone(&limiter);
    let handle = std::thread::spawn(move || tx.send(waiter.Acquire(0, 1)).unwrap());
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    // Release 后等待者应成功
    limiter.Release(0, 1);
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(()));
    handle.join().unwrap();

    // 不同 storeID 使用独立在途计数，可并行 Acquire
    limiter.Acquire(1, 1).unwrap();
    assert_eq!(limiter.Acquire(2, 1), Ok(()));
    limiter.Release(1, 1);
    limiter.Release(2, 1);
}

/// 对应 Go 同名测试：burst 内即时、耗尽后限速等待、跨 Store 独立 burst。
// TestRateLimit 对应 Go 同名测试，保留 burst 即时、释放后限速等待、不同 store 独立 burst 的断言。
#[test]
pub fn TestRateLimit() {
    let limiter = newIngestLimiter(CancellationToken::default(), 100, 10.0);
    // burst=10：前 10 次应几乎无等待
    let burst_start = Instant::now();
    for _ in 0..10 {
        limiter.Acquire(0, 1).unwrap();
    }
    assert!(burst_start.elapsed() < Duration::from_millis(50));
    for _ in 0..10 {
        limiter.Release(0, 1);
    }
    // 桶空后下一次 Acquire 需按 ~10 QPS 等待
    let limited_start = Instant::now();
    limiter.Acquire(0, 1).unwrap();
    assert!(limited_start.elapsed() >= Duration::from_millis(80));
    limiter.Release(0, 1);

    // 另一 Store 仍有独立满桶
    let independent_start = Instant::now();
    for _ in 0..10 {
        limiter.Acquire(1, 1).unwrap();
    }
    assert!(independent_start.elapsed() < Duration::from_millis(50));
}

/// 对应 Go 同名测试：等待并发槽时取消令牌应返回 `Cancelled`。
// TestContextCancelDuringConcurrencyWait 对应 Go 同名测试，覆盖等待并发令牌时 context 被取消。
#[test]
pub fn TestContextCancelDuringConcurrencyWait() {
    let token = CancellationToken::default();
    let limiter = Arc::new(newIngestLimiter(token.clone(), 1, 0.0));
    limiter.Acquire(0, 1).unwrap();
    let (tx, rx) = mpsc::channel();
    let waiter = Arc::clone(&limiter);
    std::thread::spawn(move || tx.send(waiter.Acquire(0, 1)).unwrap());
    std::thread::sleep(Duration::from_millis(20));
    token.cancel();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(Error::Cancelled)
    );
    limiter.Release(0, 1);
}

/// 对应 Go 同名测试：等待速率令牌时取消令牌应返回 `Cancelled`。
// TestContextCancelDuringRateWait 对应 Go 同名测试，覆盖等待速率令牌时 context 被取消。
#[test]
pub fn TestContextCancelDuringRateWait() {
    let token = CancellationToken::default();
    let limiter = Arc::new(newIngestLimiter(token.clone(), 100, 1.0));
    limiter.Acquire(0, 1).unwrap();
    let (tx, rx) = mpsc::channel();
    let waiter = Arc::clone(&limiter);
    std::thread::spawn(move || tx.send(waiter.Acquire(0, 1)).unwrap());
    std::thread::sleep(Duration::from_millis(20));
    token.cancel();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(Error::Cancelled)
    );
}

/// 对应 Go 同名测试：Burst 取 min(concurrency, rate) 与 NoLimit 真值表。
// TestIngestLimiterBurst 对应 Go 同名测试，保留 Burst 取 min(concurrency, rate) 和 NoLimit 判定表。
#[test]
pub fn TestIngestLimiterBurst() {
    let cases = [
        (0, 0.0, usize::MAX, true),
        (0, 1000.0, 1000, false),
        (1000, 0.0, 1000, false),
        (1000, 1000.0, 1000, false),
        (1000, 567.0, 567, false),
        (567, 1000.0, 567, false),
    ];

    for (concurrency, rate, want_burst, want_no_limit) in cases {
        let limiter = newIngestLimiter(CancellationToken::default(), concurrency, rate);
        assert_eq!(limiter.Burst(), want_burst);
        assert_eq!(limiter.NoLimit(), want_no_limit);
    }
}

/// Go uses `max(1, int(rate * ratePerSecMultiplier))` for every positive rate.
#[test]
fn event_limit_has_the_same_minimum_as_go() {
    assert_eq!(getEventLimit(0.000_5), 1.0);
    assert_eq!(getEventLimit(0.001), 1.0);
    assert_eq!(getEventLimit(1.5), 1500.0);
}

/// `semaphore.Weighted.Acquire` waits for cancellation when the weight exceeds capacity.
#[test]
fn oversized_concurrency_acquire_waits_for_cancellation() {
    let token = CancellationToken::default();
    let limiter = Arc::new(newIngestLimiter(token.clone(), 1, 0.0));
    let (tx, rx) = mpsc::channel();
    let waiter = Arc::clone(&limiter);
    let handle = std::thread::spawn(move || tx.send(waiter.Acquire(0, 2)).unwrap());

    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    token.cancel();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(Error::Cancelled)
    );
    handle.join().unwrap();
}

/// `semaphore.Weighted.Release` panics when more weight is released than held.
#[test]
#[should_panic(expected = "released more slots than held")]
fn over_release_panics_like_go() {
    let limiter = newIngestLimiter(CancellationToken::default(), 1, 0.0);
    limiter.Acquire(0, 1).unwrap();
    limiter.Release(0, 2);
}
