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

// AsterSQL 迁移补充的 zeropool 行为回归测试。
//
// 覆盖工厂复用、零值 Pool 合法性、热路径分配次数，以及高并发 Get/Put。
// 通过自定义 `GlobalAlloc` 统计分配次数，近似 Go `testing.AllocsPerRun`。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::{New, Pool};

/// 在开启计数时累加堆分配次数的全局分配器包装。
struct CountingAllocator;

thread_local! {
    /// 是否处于被计量的运行区间。
    static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    /// 当前线程累计分配次数。
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 仅在计量窗口内递增计数，避免干扰未测代码路径。
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 测量闭包平均每次运行的堆分配次数（先 warmup 一次不计入）。
pub(super) fn allocations_per_run(runs: usize, mut f: impl FnMut()) -> f64 {
    // testing.AllocsPerRun performs one unmeasured warmup call first.
    f();
    ALLOCATION_COUNT.with(|count| count.set(0));
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    for _ in 0..runs {
        f();
    }
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    ALLOCATION_COUNT.with(Cell::get) as f64 / runs as f64
}

/// 工厂创建与 Put 后再 Get 应复用，不再增加工厂调用次数。
#[test]
fn migration_pool_provides_factory_and_reused_values() {
    let calls = Arc::new(AtomicUsize::new(0));
    let factory_calls = Arc::clone(&calls);
    let pool = New(move || {
        factory_calls.fetch_add(1, Ordering::SeqCst);
        vec![0_u8; 1024]
    });

    let first = pool.Get();
    let second = pool.Get();
    assert_eq!(first.len(), 1024);
    assert_eq!(second.len(), 1024);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    pool.Put(first);
    pool.Put(second);
    assert_eq!(pool.Get().len(), 1024);
    assert_eq!(pool.Get().len(), 1024);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// 零值 Pool：空时返回空 Vec，Put 后可取回原值。
#[test]
fn migration_pool_zero_value_is_valid() {
    let pool = Pool::<Vec<u8>>::default();

    assert!(pool.Get().is_empty());
    pool.Put(vec![1, 2, 3]);
    assert_eq!(pool.Get(), vec![1, 2, 3]);
    assert!(pool.Get().is_empty());
}

/// 热身后 Get/Put 循环平均分配应小于 1。
#[test]
fn migration_pool_reuses_allocations_after_warmup() {
    let pool = New(|| vec![0_u8; 1024]);
    let item = pool.Get();
    pool.Put(item);

    let allocations = allocations_per_run(1_000, || {
        let item = pool.Get();
        pool.Put(item);
    });
    assert!(
        allocations < 1.0,
        "expected less than one allocation per run"
    );
}

/// 零值 Pool 热身后同样应接近零分配。
#[test]
fn migration_zero_value_pool_reuses_allocations_after_warmup() {
    let pool = Pool::<Vec<u8>>::default();
    let item = pool.Get();
    pool.Put(item);

    let allocations = allocations_per_run(1_000, || {
        let item = pool.Get();
        pool.Put(item);
    });
    assert!(
        allocations < 1.0,
        "expected less than one allocation per run"
    );
}

/// 多线程并发 Get/Put，校验无数据竞争且完成次数正确。
#[test]
fn migration_pool_is_safe_for_concurrent_get_and_put() {
    const CONCURRENCY: usize = u8::MAX as usize;
    const ITERATIONS: usize = 1_000_000;

    let pool = Arc::new(New(|| vec![0_u8; 1024]));
    let next = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(CONCURRENCY));
    let mut workers = Vec::with_capacity(CONCURRENCY);
    for worker in 0..CONCURRENCY {
        let pool = Arc::clone(&pool);
        let next = Arc::clone(&next);
        let completed = Arc::clone(&completed);
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            // 屏障对齐后同时开跑，放大竞态窗口。
            start.wait();
            loop {
                let iteration = next.fetch_add(1, Ordering::Relaxed);
                if iteration >= ITERATIONS {
                    break;
                }
                let mut item = pool.Get();
                item[0] = worker as u8;
                completed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(item[0], worker as u8);
                pool.Put(item);
            }
        }));
    }

    for worker in workers {
        worker.join().expect("pool worker must not panic");
    }
    assert_eq!(completed.load(Ordering::SeqCst), ITERATIONS);
}
