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

// membuf Buffer/Pool 单元测试与基准辅助例程。
//
// 覆盖块复用、内存限制阻塞/非阻塞、切片隔离、块数上限，以及 SliceLocation 相对裸切片的分配路径。

use super::super::limiter::{ErrCannotAcquireMemory, NewLimiter};
use super::*;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

/// 统计 Alloc/Free 次数的测试分配器，用于断言池是否真正触达底层分配。
#[derive(Default)]
struct TestAllocator {
    allocs: AtomicUsize,
    frees: AtomicUsize,
}

impl Allocator for TestAllocator {
    fn Alloc(&self, n: usize) -> Vec<u8> {
        self.allocs.fetch_add(1, Ordering::SeqCst);
        vec![0; n]
    }

    fn Free(&self, bytes: Vec<u8>) {
        self.frees.fetch_add(1, Ordering::SeqCst);
        drop(bytes);
    }
}

/// 统一读取 Owned/Borrowed 变体的长度。
fn allocated_len(value: &AllocatedBytes<'_>) -> usize {
    match value {
        AllocatedBytes::Owned(bytes) => bytes.len(),
        AllocatedBytes::Borrowed(bytes) => bytes.len(),
    }
}

/// 验证同块内 bump 分配、跨块取新块、大对象不计入池分配，以及 Destroy 后块回池再复用。
#[test]
fn test_buffer_pool() {
    let allocator = Arc::new(TestAllocator::default());
    let pool = NewPool(vec![
        WithBlockNum(2),
        WithAllocator(allocator.clone()),
        WithBlockSize(1024),
    ]);

    let mut bytes_buf = pool.NewBuffer(vec![]);
    assert!(bytes_buf.AllocBytes(256).is_some());
    assert_eq!(1, allocator.allocs.load(Ordering::SeqCst));
    assert!(bytes_buf.AllocBytes(512).is_some());
    assert_eq!(1, allocator.allocs.load(Ordering::SeqCst));
    // 257 超出当前块剩余，应触发第二块。
    assert!(bytes_buf.AllocBytes(257).is_some());
    assert_eq!(2, allocator.allocs.load(Ordering::SeqCst));
    assert!(bytes_buf.AllocBytes(767).is_some());
    assert_eq!(2, allocator.allocs.load(Ordering::SeqCst));

    // 超过 blockSize 的大对象独立分配，不增加 TestAllocator 计数。
    let large_bytes = bytes_buf.AllocBytes(1025).unwrap();
    assert_eq!(1025, allocated_len(&large_bytes));
    assert_eq!(2, allocator.allocs.load(Ordering::SeqCst));
    drop(large_bytes);

    assert_eq!(0, allocator.frees.load(Ordering::SeqCst));
    bytes_buf.Destroy();
    // 块回到池缓存，尚未 Free 到底层。
    assert_eq!(0, allocator.frees.load(Ordering::SeqCst));

    bytes_buf = pool.NewBuffer(vec![]);
    for _ in 0..6 {
        assert!(bytes_buf.AllocBytes(512).is_some());
    }
    bytes_buf.Destroy();
    // 缓存容量为 2，第三块溢出时才会 Free。
    assert_eq!(3, allocator.allocs.load(Ordering::SeqCst));
    assert_eq!(1, allocator.frees.load(Ordering::SeqCst));
    pool.Destroy();
}

/// Go 的 channel 缓存按 FIFO 顺序归还块，复用时应先取最早归还的块。
#[test]
fn test_pool_reuses_cached_blocks_in_fifo_order() {
    let pool = NewPool(vec![WithBlockNum(2), WithBlockSize(1)]);
    let mut buffer = pool.NewBuffer(vec![]);

    buffer.AllocBytesWithSliceLocation(1).0.unwrap()[0] = 0x11;
    buffer.AllocBytesWithSliceLocation(1).0.unwrap()[0] = 0x22;
    buffer.Destroy();

    let mut reused = pool.NewBuffer(vec![]);
    assert_eq!(0x11, reused.AllocBytesWithSliceLocation(1).0.unwrap()[0]);
    reused.Destroy();
    pool.Destroy();
}

/// 验证 Limiter：阻塞等待、Reset 不释放配额、Destroy 唤醒等待者，以及 Try* 失败原子性。
#[test]
fn test_pool_mem_limit() {
    let limiter = NewLimiter(2 * 1024 * 1024 + 2 * smallObjOverheadBatch);
    let pool = NewPool(vec![
        WithBlockSize(2 * 1024 * 1024),
        WithPoolMemoryLimiter(limiter),
    ]);
    let mut buf = pool.NewBuffer(vec![]);
    assert!(buf.AllocBytes(1024 * 1024).is_some());
    assert!(buf.AllocBytes(1024 * 1024).is_some());

    // 配额已满：另一线程 Alloc 应阻塞，直到本 Buffer Destroy。
    let mut buf2 = pool.NewBuffer(vec![]);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let waiter = thread::spawn(move || {
        assert!(buf2.AllocBytes(1024 * 1024).is_some());
        buf2.Destroy();
        done_tx.send(()).unwrap();
    });

    thread::sleep(Duration::from_millis(50));
    assert!(done_rx.try_recv().is_err());
    // Reset 复用块但不归还 Limiter，等待者仍应阻塞。
    buf.Reset();
    assert!(buf.AllocBytes(1024 * 1024).is_some());
    assert!(buf.AllocBytes(1024 * 1024).is_some());
    assert!(done_rx.try_recv().is_err());
    buf.Destroy();
    done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    waiter.join().unwrap();
    assert!(buf.AllocBytes(2 * 1024 * 1024).is_some());
    buf.Destroy();
    pool.Destroy();

    // 非阻塞路径：配额不足返回 ErrCannotAcquireMemory。
    let limiter = NewLimiter(1024 + smallObjOverheadBatch);
    let pool = NewPool(vec![
        WithBlockNum(0),
        WithBlockSize(1024),
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
    second.Destroy();
    pool.Destroy();

    // TryAlloc 失败后 Buffer 内部写指针与元数据账本必须保持原状。
    const BLOCK_SIZE: usize = 1024;
    let limiter = NewLimiter(BLOCK_SIZE);
    let pool = NewPool(vec![
        WithBlockNum(0),
        WithBlockSize(BLOCK_SIZE),
        WithPoolMemoryLimiter(limiter.clone()),
    ]);
    let mut failed = pool.NewBuffer(vec![]);
    let error = match failed.TryAllocBytes(1) {
        Err(error) => error,
        Ok(_) => panic!("allocation unexpectedly succeeded"),
    };
    assert_eq!(error, ErrCannotAcquireMemory);
    assert!(limiter.TryAcquire(BLOCK_SIZE));
    limiter.Release(BLOCK_SIZE);
    assert!(failed.blocks.is_empty());
    assert_eq!(-1, failed.curBlockIdx);
    assert_eq!(0, failed.curIdx);
    assert_eq!(0, failed.smallObjOverhead);
    assert_eq!(0, failed.smallObjOverheadCache);
    pool.Destroy();
}

/// 验证不同 SliceLocation 指向独立区域，GetSlice 可按位置回读。
#[test]
fn test_buffer_isolation() {
    let pool = NewPool(vec![WithBlockSize(1024)]);
    let mut bytes_buf = pool.NewBuffer(vec![]);

    let (b1, b1_location) = bytes_buf.AllocBytesWithSliceLocation(16);
    b1.unwrap().fill(0x11);
    let (b2, b2_location) = bytes_buf.AllocBytesWithSliceLocation(16);
    let mut random = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut random);
    b2.unwrap().copy_from_slice(&random);
    let b3 = bytes_buf.GetSlice(&b2_location).to_vec();
    bytes_buf.GetSlice(&b1_location)[..4].copy_from_slice(&[0, 1, 2, 3]);
    assert_eq!(b3, bytes_buf.GetSlice(&b2_location));
    assert_ne!(
        bytes_buf.GetSlice(&b2_location).to_vec(),
        bytes_buf.GetSlice(&b1_location).to_vec()
    );
    bytes_buf.Destroy();
    pool.Destroy();
}

/// 验证 WithBufferMemoryLimit 将字节上限对齐为块数，Reset 后可在原块上重新分配。
#[test]
fn test_buffer_mem_limit() {
    let pool = NewPool(vec![WithBlockSize(10)]);
    let mut bytes_buf = pool.NewBuffer(vec![WithBufferMemoryLimit(5)]);

    assert!(bytes_buf.AllocBytesWithSliceLocation(9).0.is_some());
    // 仅允许 1 块：第二次需新块时应失败。
    assert!(bytes_buf.AllocBytesWithSliceLocation(3).0.is_none());
    bytes_buf.Destroy();
    assert!(bytes_buf.AllocBytesWithSliceLocation(3).0.is_some());

    bytes_buf = pool.NewBuffer(vec![WithBufferMemoryLimit(20)]);
    assert!(bytes_buf.AllocBytesWithSliceLocation(9).0.is_some());
    assert!(bytes_buf.AllocBytesWithSliceLocation(9).0.is_some());
    assert!(bytes_buf.AllocBytesWithSliceLocation(2).0.is_none());
    bytes_buf.Reset();
    assert!(bytes_buf.AllocBytesWithSliceLocation(9).0.is_some());
    assert!(bytes_buf.AllocBytesWithSliceLocation(9).0.is_some());
    assert!(bytes_buf.AllocBytesWithSliceLocation(2).0.is_none());
    pool.Destroy();
}

/// 验证向上取整块数与对齐字节数的辅助函数。
#[test]
fn test_get_aligned_size_get_block_cnt() {
    assert_eq!(1, getBlockCnt(10, 16));
    assert_eq!(2, getBlockCnt(17, 16));
    assert_eq!(16, GetAlignedSize(10, 16));
    assert_eq!(32, GetAlignedSize(17, 16));
    assert_eq!(0, getBlockCnt(u64::MAX, 2));
    assert_eq!(0, GetAlignedSize(u64::MAX, 2));
}

/// 大规模分配基准用的数据条目数。
const DATA_NUM: usize = 100 * 1024 * 1024;
/// 排序基准用的数据条目数。
const SORT_DATA_NUM: usize = 1024 * 1024;

/// 轻量基准驱动：调用一次 body，便于与 Go testing.B 结构对照。
struct Bencher;

impl Bencher {
    /// 执行一轮基准体（迁移基线中不做真实计时循环）。
    fn iter(&mut self, mut body: impl FnMut()) {
        body();
    }
}

/// 基准：以 `Vec<u8>` 形式存储大量小切片。
fn benchmark_store_slice(b: &mut Bencher) {
    let mut data = vec![Vec::new(); DATA_NUM];
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            let value = buffer.AllocBytes(10).unwrap();
            *slot = match value {
                AllocatedBytes::Owned(value) => value,
                AllocatedBytes::Borrowed(value) => value.to_vec(),
            };
        }
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 基准：以 SliceLocation 存储位置句柄，避免持有指针切片。
fn benchmark_store_location(b: &mut Bencher) {
    let mut data = vec![SliceLocation::default(); DATA_NUM];
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            *slot = buffer.AllocBytesWithSliceLocation(10).1;
        }
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 基准：对裸切片向量排序。
fn benchmark_sort_slice(b: &mut Bencher) {
    let mut data = vec![Vec::new(); SORT_DATA_NUM];
    let mut random = StdRng::seed_from_u64(6716);
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            let value = buffer.AllocBytes(10).unwrap();
            *slot = match value {
                AllocatedBytes::Owned(value) => value,
                AllocatedBytes::Borrowed(value) => value.to_vec(),
            };
            random.fill_bytes(slot);
        }
        data.sort();
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 通过 hook 注入 GC/调度点，比较 Location 排序路径开销。
fn benchmark_sort_location_with_hook(b: &mut Bencher, mut hook: impl FnMut()) {
    let mut data = vec![SliceLocation::default(); SORT_DATA_NUM];
    let mut random = StdRng::seed_from_u64(6716);
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            let (bytes, location) = buffer.AllocBytesWithSliceLocation(10);
            random.fill_bytes(bytes.unwrap());
            *slot = location;
        }
        hook();
        data.sort_by_key(|location| buffer.GetSlice(location).to_vec());
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 基准：无额外 hook 的 Location 排序。
fn benchmark_sort_location(b: &mut Bencher) {
    benchmark_sort_location_with_hook(b, || {});
}

/// 基准：排序前 yield，模拟 GC/调度压力下的切片路径。
fn benchmark_sort_slice_with_gc(b: &mut Bencher) {
    let mut data = vec![Vec::new(); SORT_DATA_NUM];
    let mut random = StdRng::seed_from_u64(6716);
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            let value = buffer.AllocBytes(10).unwrap();
            *slot = match value {
                AllocatedBytes::Owned(value) => value,
                AllocatedBytes::Borrowed(value) => value.to_vec(),
            };
            random.fill_bytes(slot);
        }
        thread::yield_now();
        data.sort();
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 基准：排序前 yield 的 Location 路径。
fn benchmark_sort_location_with_gc(b: &mut Bencher) {
    benchmark_sort_location_with_hook(b, thread::yield_now);
}

/// 基准：排序比较器中是否让 Location 逃逸到堆（Box）对性能的影响。
fn benchmark_sort_location_escape_mode(b: &mut Bencher, escape: bool) {
    let mut data = vec![SliceLocation::default(); SORT_DATA_NUM];
    let mut random = StdRng::seed_from_u64(6716);
    b.iter(|| {
        let pool = NewPool(vec![]);
        let mut buffer = pool.NewBuffer(vec![]);
        for slot in &mut data {
            let (bytes, location) = buffer.AllocBytesWithSliceLocation(10);
            random.fill_bytes(bytes.unwrap());
            *slot = location;
        }

        let mut sortable: Vec<_> = data
            .iter()
            .map(|location| (buffer.GetSlice(location).to_vec(), *location))
            .collect();
        let mut duplicate_found = false;
        let mut escaped_location = None;
        let mut copied_location = SliceLocation::default();
        // 首次相等时按 escape 开关选择堆逃逸或栈拷贝，对齐 Go 逃逸分析场景。
        sortable.sort_by(|left, right| {
            let result = left.0.cmp(&right.0);
            if result.is_eq() && !duplicate_found {
                duplicate_found = true;
                if escape {
                    escaped_location = Some(Box::new(left.1));
                } else {
                    copied_location = left.1;
                }
            }
            result
        });
        drop(escaped_location);
        let _ = copied_location;
        buffer.Destroy();
        pool.Destroy();
    });
}

/// 基准：强制 Location 逃逸。
fn benchmark_sort_location_with_escape(b: &mut Bencher) {
    benchmark_sort_location_escape_mode(b, true);
}

/// 基准：Location 仅栈拷贝、不逃逸。
fn benchmark_sort_location_without_escape(b: &mut Bencher) {
    benchmark_sort_location_escape_mode(b, false);
}

/// 基准：多线程并发从带 Limiter 的池中分配。
fn benchmark_concurrent_acquire(b: &mut Bencher) {
    b.iter(|| {
        let limiter = NewLimiter(512 * 1024 * 1024);
        let pool = NewPool(vec![
            WithPoolMemoryLimiter(limiter),
            WithBlockSize(4 * 1024),
        ]);
        let mut workers = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let pool = pool.clone();
            workers.push(thread::spawn(move || {
                let mut buffer = pool.NewBuffer(vec![]);
                for _ in 0..1000 {
                    assert!(buffer.AllocBytes(100).is_some());
                }
                buffer.Destroy();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        pool.Destroy();
    });
}
