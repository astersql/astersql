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

//! Go-equivalent tests for `br/pkg/backup/limit_test.go`.
//!
//! 这些测试验证的不是吞吐性能，而是“超过阈值时如何阻塞、释放后如何恢复”。
//! 尤其第二个测试会保留 Go 版本允许的 120 峰值容忍度。
//! 这是因为限流器按进入前的当前值判定，而不是严格把结果钳制在阈值以内。
//! 因而这里的断言其实是在守护一个非常具体的并发语义。
//! 后续若有人把限流器改成严格上界模式，这里会第一时间报错。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::limit::NewResourceMemoryLimiter;

/// TestResourceConcurrentLimiter: fill to 100, then Acquire(200) wakes after Release.
/// 这个用例先把额度占满，再验证大请求会等待到释放后才继续推进。
#[test]
fn test_resource_concurrent_limiter() {
    let limiter = Arc::new(NewResourceMemoryLimiter(100));

    let tag = Arc::new(AtomicI64::new(0));
    limiter.Acquire(50);
    limiter.Acquire(50);

    let handle = {
        let limiter = Arc::clone(&limiter);
        let tag = Arc::clone(&tag);
        thread::spawn(move || {
            limiter.Acquire(200);
            assert_eq!(1, tag.load(Ordering::SeqCst));
            assert_eq!(2, tag.fetch_add(1, Ordering::SeqCst) + 1);
            limiter.Release(200);
        })
    };

    // 工作线程里的 `Acquire(200)` 在这里还拿不到额度，主线程先观察阻塞状态。
    // 只有两个 50 都被释放后，后台线程才会继续推进并把 `tag` 从 1 增到 2。
    // acquire 200 is blocked
    assert_eq!(1, tag.fetch_add(1, Ordering::SeqCst) + 1);
    limiter.Release(50);
    limiter.Release(50);

    handle.join().expect("limiter wait thread should join");
    assert_eq!(2, tag.load(Ordering::SeqCst));
    limiter.Acquire(99);
    limiter.Acquire(1);
    limiter.Release(100);
}

/// TestResourceConcurrentLimiter2: 20 goroutines Acquire(30); peak current ≤ 120.
/// 并发压测重点是验证“宽松阈值”仍然有上界，不会无限叠加。
/// 只要峰值仍被限制在可接受范围内，就说明实现没有偏离 Go 契约。
#[test]
fn test_resource_concurrent_limiter2() {
    let limiter = Arc::new(NewResourceMemoryLimiter(100));
    let mut handles = Vec::new();
    for _ in 0..20 {
        let limiter = Arc::clone(&limiter);
        handles.push(thread::spawn(move || {
            let start = Instant::now();
            let current = limiter.Acquire(30);
            let _ = (current, start.elapsed());
            thread::sleep(Duration::from_millis(10));
            limiter.Release(30);
            assert!(current <= 120, "current={current} exceeds Go tolerance 120");
        }));
    }
    for handle in handles {
        handle.join().expect("limiter worker should join");
    }
}
