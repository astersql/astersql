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

// `zeropool` 与 Go `pool_test.go` 对齐的单元测试与基准辅助。
//
// 覆盖取值正确性、高并发无数据竞争、热路径零分配，以及零值 Pool 可用性；
// 基准函数对比 Mutex+Vec 模拟的 sync.Pool 存值/存指针开销。

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use crate as zeropool;

/// 委托迁移补充测试中的分配计数器，近似 Go `testing.AllocsPerRun`。
fn allocations_per_run(runs: usize, mut operation: impl FnMut()) -> f64 {
    super::migration_aster_unit_test::allocations_per_run(runs, &mut operation)
}

// test_pool corresponds to Go's TestPool and preserves its four subtests in order.
/// 对应 Go `TestPool`：按顺序跑四个子场景（取值、竞态、零分配、零值合法）。
#[test]
fn test_pool() {
    // "provides correct values"：工厂产出定长切片，Put 后再 Get 长度仍正确。
    // "provides correct values"
    {
        let pool = zeropool::New(|| vec![0_u8; 1024]);
        let item1 = pool.Get();
        assert_eq!(1024, item1.len());

        let item2 = pool.Get();
        assert_eq!(1024, item2.len());

        pool.Put(item1);
        pool.Put(item2);

        let item1 = pool.Get();
        assert_eq!(1024, item1.len());

        let item2 = pool.Get();
        assert_eq!(1024, item2.len());
    }

    // "is not racy"：多线程并发 Get/写首字节/Put，检查对象未被交错污染。
    // "is not racy"
    {
        const ITERATIONS: usize = 1_000_000;
        const CONCURRENCY: usize = u8::MAX as usize;

        let pool = Arc::new(zeropool::New(|| vec![0_u8; 1024]));
        let next = Arc::new(AtomicUsize::new(0));
        let counter = Arc::new(AtomicU64::new(0));
        let run = Arc::new(Barrier::new(CONCURRENCY));
        let mut workers = Vec::with_capacity(CONCURRENCY);

        for worker in 0..CONCURRENCY {
            let pool = Arc::clone(&pool);
            let next = Arc::clone(&next);
            let counter = Arc::clone(&counter);
            let run = Arc::clone(&run);
            workers.push(thread::spawn(move || {
                // Barrier 对齐后同时开跑，放大竞态窗口。
                run.wait();
                loop {
                    let iteration = next.fetch_add(1, Ordering::Relaxed);
                    if iteration >= ITERATIONS {
                        break;
                    }

                    let mut item = pool.Get();
                    item[0] = worker as u8;
                    // 计数兼作轻微延迟，贴近 Go 侧增加竞态概率的做法。
                    counter.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(worker as u8, item[0], "wrong value");
                    pool.Put(item);
                }
            }));
        }

        for worker in workers {
            worker.join().expect("pool worker panicked");
        }
        assert_eq!(ITERATIONS as u64, counter.load(Ordering::Relaxed));
        eprintln!("Done {} iterations", counter.load(Ordering::Relaxed));
    }

    // "does not allocate"：预热一次后，热路径 Get/Put 平均分配应低于 1。
    // "does not allocate"
    {
        let pool = zeropool::New(|| vec![0_u8; 1024]);
        let slice = pool.Get();
        pool.Put(slice);

        let allocs = allocations_per_run(1000, || {
            let slice = pool.Get();
            pool.Put(slice);
        });
        assert!(allocs < 1.0, "Should not allocate: {allocs}");
    }

    // "zero value is valid"：Default 构造的 Pool 可直接使用且热路径不分配。
    // "zero value is valid"
    {
        let pool: zeropool::Pool<Vec<u8>> = Default::default();
        let slice = pool.Get();
        pool.Put(slice);

        let allocs = allocations_per_run(1000, || {
            let slice = pool.Get();
            pool.Put(slice);
        });
        assert!(allocs < 1.0, "Should not allocate: {allocs}");
    }
}

// benchmark_zeropool_pool corresponds to Go's BenchmarkZeropoolPool.
/// 对应 Go `BenchmarkZeropoolPool`：预热后循环 Get/Put。
#[allow(dead_code)]
pub fn benchmark_zeropool_pool(iterations: usize) {
    let pool = zeropool::New(|| vec![0_u8; 1024]);
    let item = pool.Get();
    pool.Put(item);

    for _ in 0..iterations {
        let item = black_box(pool.Get());
        pool.Put(item);
    }
}

// benchmark_sync_pool_value corresponds to Go's BenchmarkSyncPoolValue.
/// 对应 Go `BenchmarkSyncPoolValue`：Mutex+Vec 直接存值作为对照。
#[allow(dead_code)]
pub fn benchmark_sync_pool_value(iterations: usize) {
    let pool = Mutex::new(vec![vec![0_u8; 1024]]);
    for _ in 0..iterations {
        let item = pool
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| vec![0_u8; 1024]);
        pool.lock().unwrap().push(black_box(item));
    }
}

// benchmark_sync_pool_new_pointer corresponds to Go's BenchmarkSyncPoolNewPointer.
/// 对应 Go `BenchmarkSyncPoolNewPointer`：每次归还重新 `Box::new`，模拟指针分配开销。
#[allow(dead_code)]
pub fn benchmark_sync_pool_new_pointer(iterations: usize) {
    let pool = Mutex::new(vec![Box::new(vec![0_u8; 1024])]);
    for _ in 0..iterations {
        let item = pool
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Box::new(vec![0_u8; 1024]));
        // 解包再装箱：刻意制造新指针分配，对照 zeropool 复用外壳。
        let buf = *item;
        pool.lock().unwrap().push(black_box(Box::new(buf)));
    }
}

// benchmark_sync_pool_pointer corresponds to Go's BenchmarkSyncPoolPointer.
/// 对应 Go `BenchmarkSyncPoolPointer`：复用同一 `Box` 指针外壳的对照基准。
#[allow(dead_code)]
pub fn benchmark_sync_pool_pointer(iterations: usize) {
    let pool = Mutex::new(vec![Box::new(vec![0_u8; 1024])]);
    for _ in 0..iterations {
        let item = pool
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Box::new(vec![0_u8; 1024]));
        pool.lock().unwrap().push(black_box(item));
    }
}
