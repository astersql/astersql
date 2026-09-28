// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `ConcurrentBitmap` 的并发行为单元测试（对应 Go `concurrent_test.go`）。
//
// 覆盖多线程置位正确性、唯一 setter 语义，以及 `Reset` 清零与 `bitLen` 更新。

use super::NewConcurrentBitmap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;

// TestConcurrentBitmapSet 对应 Go 测试：并发设置偶数位，然后用 UnsafeIsSet 检查奇偶位置。
/// 多线程只置偶数位，再用 `UnsafeIsSet` 校验奇偶位置。
#[test]
pub fn TestConcurrentBitmapSet() {
    const LOOP_COUNT: i32 = 1000;
    const INTERVAL: i32 = 2;

    let bm = Arc::new(NewConcurrentBitmap(LOOP_COUNT * INTERVAL));
    let next = Arc::new(AtomicUsize::new(0));
    let workers = thread::available_parallelism()
        .map_or(2, |value| value.get())
        .max(2);
    // 工作线程通过原子计数器分摊任务，各自对偶数下标调用 Set。
    thread::scope(|scope| {
        for _ in 0..workers {
            let bm = Arc::clone(&bm);
            let next = Arc::clone(&next);
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= LOOP_COUNT as usize {
                        break;
                    }
                    bm.Set(i as i32 * INTERVAL);
                }
            });
        }
    });

    for i in 0..LOOP_COUNT {
        assert_eq!(i % INTERVAL == 0, bm.UnsafeIsSet(i));
    }
}

// TestConcurrentBitmapUniqueSetter checks if isSetter is unique everytime
// when a bit is set.
// TestConcurrentBitmapUniqueSetter 对应 Go 测试：反复清零同一 bit，再让多个竞争者并发 Set。
/// 同一 bit 上多竞争者并发 Set：每次清零后恰好一个唯一 setter。
#[test]
pub fn TestConcurrentBitmapUniqueSetter() {
    const LOOP_COUNT: usize = 10000;
    const COMPETITORS_PER_SET: usize = 50;

    let bm = Arc::new(NewConcurrentBitmap(32));
    let setter_counter = Arc::new(AtomicU64::new(0));
    let clear_counter = AtomicU64::new(0);
    let next_task = Arc::new(AtomicUsize::new(0));
    let available_tasks = Arc::new(AtomicUsize::new(0));
    let total_tasks = LOOP_COUNT * COMPETITORS_PER_SET;

    thread::scope(|scope| {
        for _ in 0..COMPETITORS_PER_SET {
            let bm = Arc::clone(&bm);
            let setter_counter = Arc::clone(&setter_counter);
            let next_task = Arc::clone(&next_task);
            let available_tasks = Arc::clone(&available_tasks);
            scope.spawn(move || {
                loop {
                    let task = next_task.fetch_add(1, Ordering::Relaxed);
                    if task >= total_tasks {
                        break;
                    }
                    // 等待主线程放行本轮可用任务后再竞争置位 bit 31。
                    while task >= available_tasks.load(Ordering::Acquire) {
                        thread::yield_now();
                    }
                    if bm.Set(31) {
                        setter_counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }

        // 主线程尝试清零 bit 31（最低位），再放行下一批评测任务。
        for _ in 0..LOOP_COUNT {
            if bm.segments[0]
                .compare_exchange(0x00000001, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                clear_counter.fetch_add(1, Ordering::SeqCst);
            }
            available_tasks.fetch_add(COMPETITORS_PER_SET, Ordering::Release);
        }
    });

    let clear_count = clear_counter.load(Ordering::SeqCst);
    assert!(clear_count < LOOP_COUNT as u64);
    assert_eq!(setter_counter.load(Ordering::SeqCst), clear_count + 1);
}

// TestResetConcurrentBitmap test the reset of concurrentBitmap.
// TestResetConcurrentBitmap 对应 Go 测试：已有置位在 Reset 后被清空，并更新 bitLen。
/// 验证 `Reset` 后已置位被清空且 `bitLen` 更新。
#[test]
pub fn TestResetConcurrentBitmap() {
    let mut bm = NewConcurrentBitmap(32);
    bm.Set(1);
    bm.Set(3);
    bm.Set(7);
    bm.Set(16);

    bm.Reset(8);
    assert_eq!(bm.bitLen, 8);
    assert!(!bm.UnsafeIsSet(1));
    assert!(!bm.UnsafeIsSet(3));
    assert!(!bm.UnsafeIsSet(7));
}
