// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// membuf 迁移对齐测试：对照 Go 行为验证 Pool/Buffer 复用、对齐、限流与 FIFO。

use super::buffer::{
    AllocatedBytes, Allocator, GetAlignedSize, NewPool, WithAllocator, WithBlockNum, WithBlockSize,
    WithBufferMemoryLimit, WithPoolMemoryLimiter,
};
use super::limiter::{ErrCannotAcquireMemory, NewLimiter};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

/// 计数分配器：统计 Alloc/Free 次数，用于断言块复用与溢出释放。
#[derive(Default)]
struct CountingAllocator {
    allocations: AtomicUsize,
    frees: AtomicUsize,
}

impl Allocator for CountingAllocator {
    fn Alloc(&self, n: usize) -> Vec<u8> {
        self.allocations.fetch_add(1, Ordering::SeqCst);
        vec![0; n]
    }

    fn Free(&self, bytes: Vec<u8>) {
        self.frees.fetch_add(1, Ordering::SeqCst);
        drop(bytes);
    }
}

/// 将 `AllocatedBytes` 展开为可变字节切片，便于读写断言。
fn bytes<'a, 'b>(value: &'a mut AllocatedBytes<'b>) -> &'a mut [u8] {
    match value {
        AllocatedBytes::Owned(value) => value,
        AllocatedBytes::Borrowed(value) => value,
    }
}

/// 池内块可复用；超块大小走独立分配；Destroy 后溢出块才 Free，对齐 Go。
#[test]
fn migration_pool_reuses_blocks_and_frees_overflow_like_go() {
    let allocator = Arc::new(CountingAllocator::default());
    let pool = NewPool(vec![
        WithBlockNum(2),
        WithAllocator(allocator.clone()),
        WithBlockSize(1024),
    ]);

    let mut buffer = pool.NewBuffer(vec![]);
    assert!(buffer.AllocBytes(256).is_some());
    assert!(buffer.AllocBytes(512).is_some());
    assert!(buffer.AllocBytes(257).is_some());
    assert!(buffer.AllocBytes(767).is_some());
    assert_eq!(allocator.allocations.load(Ordering::SeqCst), 2);
    let mut large = buffer.AllocBytes(1025).unwrap();
    assert_eq!(bytes(&mut large).len(), 1025);
    assert_eq!(allocator.allocations.load(Ordering::SeqCst), 2);
    drop(large);
    buffer.Destroy();
    assert_eq!(allocator.frees.load(Ordering::SeqCst), 0);

    let mut second = pool.NewBuffer(vec![]);
    for _ in 0..6 {
        assert!(second.AllocBytes(512).is_some());
    }
    second.Destroy();
    assert_eq!(allocator.allocations.load(Ordering::SeqCst), 3);
    assert_eq!(allocator.frees.load(Ordering::SeqCst), 1);
    pool.Destroy();
}

/// 零长度分配：尚无块时返回 None（对齐 Go nil）；有块后返回空切片。
#[test]
fn migration_zero_length_allocation_matches_go_nil_until_a_block_exists() {
    let pool = NewPool(vec![WithBlockSize(8)]);
    let mut buffer = pool.NewBuffer(vec![]);

    assert!(buffer.AllocBytes(0).is_none());
    assert!(buffer.TryAllocBytes(0).unwrap().is_none());
    assert!(buffer.AllocBytesWithSliceLocation(0).0.is_none());

    assert!(buffer.AllocBytes(1).is_some());
    assert_eq!(bytes(&mut buffer.AllocBytes(0).unwrap()).len(), 0);
    assert_eq!(
        bytes(&mut buffer.TryAllocBytes(0).unwrap().unwrap()).len(),
        0
    );
    assert_eq!(buffer.AllocBytesWithSliceLocation(0).0.unwrap().len(), 0);
}

/// 对齐计算、Buffer 内存上限、SliceLocation 读写、Reset 复用与切片隔离对齐 Go。
#[test]
fn migration_buffer_limit_locations_reset_and_isolation_match_go() {
    assert_eq!(GetAlignedSize(10, 16), 16);
    assert_eq!(GetAlignedSize(17, 16), 32);

    let pool = NewPool(vec![WithBlockSize(10)]);
    let mut buffer = pool.NewBuffer(vec![WithBufferMemoryLimit(20)]);
    let (first, first_location) = buffer.AllocBytesWithSliceLocation(9);
    first.unwrap().copy_from_slice(b"123456789");
    let (second, second_location) = buffer.AllocBytesWithSliceLocation(9);
    second.unwrap().copy_from_slice(b"abcdefghi");
    assert!(buffer.AllocBytesWithSliceLocation(2).0.is_none());
    assert_eq!(buffer.GetSlice(&first_location), b"123456789");
    assert_eq!(buffer.GetSlice(&second_location), b"abcdefghi");

    buffer.Reset();
    let (reused, _) = buffer.AllocBytesWithSliceLocation(9);
    reused.unwrap().copy_from_slice(b"987654321");
    assert_eq!(buffer.GetSlice(&first_location), b"987654321");
    buffer.Destroy();

    let pool = NewPool(vec![WithBlockSize(1024)]);
    let mut buffer = pool.NewBuffer(vec![]);
    let (left, left_location) = buffer.AllocBytesWithSliceLocation(16);
    left.unwrap().fill(1);
    let (right, right_location) = buffer.AllocBytesWithSliceLocation(16);
    right.unwrap().fill(2);
    buffer.GetSlice(&left_location)[0] = 3;
    assert_eq!(buffer.GetSlice(&right_location)[0], 2);
}

/// TryAlloc/TryAdd 不阻塞；失败不改动 TotalSize；配额释放后可再成功。
#[test]
fn migration_try_allocation_is_nonblocking_and_failure_is_atomic() {
    const BLOCK_SIZE: usize = 1024;
    let limiter = NewLimiter(BLOCK_SIZE);
    let pool = NewPool(vec![
        WithBlockNum(0),
        WithBlockSize(BLOCK_SIZE),
        WithPoolMemoryLimiter(limiter.clone()),
    ]);
    let mut buffer = pool.NewBuffer(vec![]);

    let error = match buffer.TryAllocBytes(1) {
        Err(error) => error,
        Ok(_) => panic!("allocation unexpectedly succeeded"),
    };
    assert_eq!(error, ErrCannotAcquireMemory);
    assert_eq!(buffer.TotalSize(), 0);
    assert!(limiter.TryAcquire(BLOCK_SIZE));
    limiter.Release(BLOCK_SIZE);

    let limiter = NewLimiter(BLOCK_SIZE + 256 * 1024);
    let pool = NewPool(vec![
        WithBlockNum(0),
        WithBlockSize(BLOCK_SIZE),
        WithPoolMemoryLimiter(limiter),
    ]);
    let mut first = pool.NewBuffer(vec![]);
    assert!(first.TryAddBytes(b"a").unwrap().is_some());
    let mut second = pool.NewBuffer(vec![]);
    let error = match second.TryAddBytes(b"b") {
        Err(error) => error,
        Ok(_) => panic!("allocation unexpectedly succeeded"),
    };
    assert_eq!(error, ErrCannotAcquireMemory);
    first.Destroy();
    assert!(second.TryAddBytes(b"b").unwrap().is_some());
}

/// 已有阻塞等待者时 TryAcquire 失败（含 n=0），保证 FIFO；Release 后再可 TryAcquire。
#[test]
fn migration_limiter_blocks_and_preserves_fifo_against_try_acquire() {
    let limiter = NewLimiter(2);
    limiter.Acquire(2);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let waiter_limiter = limiter.clone();
    let waiter = thread::spawn(move || {
        started_tx.send(()).unwrap();
        waiter_limiter.Acquire(2);
        finished_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    // 给等待线程一点时间进入 Condvar 等待
    thread::sleep(Duration::from_millis(20));

    // 即使请求 0 也不应插队越过 FIFO 队首
    assert!(!limiter.TryAcquire(0));
    assert!(finished_rx.try_recv().is_err());
    limiter.Release(2);
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    waiter.join().unwrap();
    limiter.Release(2);
    assert!(limiter.TryAcquire(2));
}
