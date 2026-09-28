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

// ID 池单元测试：AutoIncPool、LockFreeCircularPool 与锁实现对照池的并发安全。
//
// 对齐 Go `pool_test.go`：基本满/空流程、head/tail 溢出、多生产者消费者总和校验，
// 以及基于 C++ Concurrency in Action 场景的表驱动并发用例；含 benchmark 骨架。

use crate as globalconn;
use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// AutoIncPool：顺序、回绕、耗尽与 Put 后复用。
// TestAutoIncPool 对应 Go 的 AutoIncPool 顺序、回绕、耗尽和 Put 后复用测试。
#[test]
fn TestAutoIncPool() {
    const SizeInBits: u32 = 8;
    const Size: u64 = 1 << SizeInBits;
    const TryCnt: i32 = 4;

    let mut pool = globalconn::AutoIncPool::default();
    pool.InitExt(Size, true, TryCnt);
    assert_eq!(Size as i32, pool.Cap());
    assert_eq!(0, pool.Len());

    // get all.
    for i in 1..Size {
        let (val, ok) = pool.Get();
        assert!(ok);
        assert_eq!(i, val);
    }
    let (val, ok) = pool.Get();
    assert!(ok);
    assert_eq!(0u64, val); // wrap around to 0
    assert_eq!(Size as i32, pool.Len());

    let (_, ok) = pool.Get(); // exhausted. try TryCnt times, lastID is added to 0+TryCnt.
    assert!(!ok);

    // Put 释放指定 ID 后，下一次 Get 应该拿到该 ID。
    let mut nextVal = (TryCnt + 1) as u64;
    pool.Put(nextVal);
    let (val, ok) = pool.Get();
    assert!(ok);
    assert_eq!(nextVal, val);

    nextVal += (TryCnt - 1) as u64;
    pool.Put(nextVal);
    let (val, ok) = pool.Get();
    assert!(ok);
    assert_eq!(nextVal, val);

    nextVal += (TryCnt + 1) as u64;
    pool.Put(nextVal);
    let (_, ok) = pool.Get();
    assert!(!ok);
}

/// 无锁池：满初始化、取空、放满、再取空。
// TestLockFreePoolBasic 对应 Go 的无锁环形池满初始化、取空、放满和再次取空流程。
#[test]
fn TestLockFreePoolBasic() {
    const SizeInBits: u32 = 8;
    const Size: u64 = (1 << SizeInBits) - 1;

    let mut pool = globalconn::LockFreeCircularPool::default();
    pool.InitExt(1u32 << SizeInBits, u32::MAX);
    assert_eq!(Size as i32, pool.Cap());
    assert_eq!(Size as i32, pool.Len());

    // get all.
    for i in 1..=Size {
        let (val, ok) = pool.Get();
        assert!(ok);
        assert_eq!(i, val);
    }
    let (_, ok) = pool.Get();
    assert!(!ok);
    assert_eq!(0, pool.Len());

    // put to full.
    for i in 1..=Size {
        assert!(pool.Put(i));
    }
    assert!(!pool.Put(0));
    assert_eq!(Size as i32, pool.Len());

    // get all.
    for i in 1..=Size {
        let (val, ok) = pool.Get();
        assert!(ok);
        assert_eq!(i, val);
    }
    let (_, ok) = pool.Get();
    assert!(!ok);
    assert_eq!(0, pool.Len());
}

/// 无锁池空初始化：先 Put 满再全部 Get。
// TestLockFreePoolInitEmpty 对应 Go 的空初始化路径，先放满再全部取出。
#[test]
fn TestLockFreePoolInitEmpty() {
    const SizeInBits: u32 = 8;
    const Size: u64 = (1 << SizeInBits) - 1;

    let mut pool = globalconn::LockFreeCircularPool::default();
    pool.InitExt(1u32 << SizeInBits, 0);
    assert_eq!(Size as i32, pool.Cap());
    assert_eq!(0, pool.Len());

    // put to full.
    for i in 1..=Size {
        assert!(pool.Put(i));
    }
    assert!(!pool.Put(0));
    assert_eq!(Size as i32, pool.Len());

    // get all.
    for i in 1..=Size {
        let (val, ok) = pool.Get();
        assert!(ok);
        assert_eq!(i, val);
    }
    let (_, ok) = pool.Get();
    assert!(!ok);
    assert_eq!(0, pool.Len());
}

/// 基准对照用的加锁环形池（仅测试可见）。
// LockBasedCircularPool implements IDPool by lock-based manner.
// For benchmark purpose.
// LockBasedCircularPool 是 Go 测试文件里的基准对照实现，用互斥锁保护 head/tail 和 slots。
#[derive(Default)]
pub(super) struct LockBasedCircularPool {
    _align: u64, // align to 64bits
    /// 受互斥锁保护的环形状态。
    state: Mutex<LockBasedCircularPoolState>,
}

/// 锁实现池的内部状态（须在同一把锁下读写）。
// LockBasedCircularPoolState 保存 Go 结构体中需要在同一把锁下读写的字段。
#[derive(Default)]
struct LockBasedCircularPoolState {
    head: u32, // first available slot
    tail: u32, // first empty slot. `head==tail` means empty.
    cap: u32,
    slots: Vec<u32>,
}

impl LockBasedCircularPool {
    // Init 对应 Go 的同名方法，默认 fillCount 为 0。
    fn Init(&mut self, size: u64) {
        self.InitExt(size as u32, 0);
    }

    // InitExt 初始化锁实现环形池；前 fillCount 个槽位可读，其余槽位填 MaxUint32。
    pub(super) fn InitExt(&mut self, size: u32, fillCount: u32) {
        let mut state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        state.cap = size;
        state.slots = vec![0; state.cap as usize];

        let fillCount = fillCount.min(state.cap.wrapping_sub(1));
        let mut i = 0u32;
        while i < fillCount {
            state.slots[i as usize] = i + 1;
            i += 1;
        }
        while i < state.cap {
            state.slots[i as usize] = u32::MAX;
            i += 1;
        }

        state.head = 0;
        state.tail = fillCount;
    }

    // Len 对应 Go 的加锁长度计算，tail-head 使用 u32 回绕语义。
    fn Len(&self) -> i32 {
        let state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        state.tail.wrapping_sub(state.head) as i32
    }

    // Cap 对应 Go 的 cap-1，有一个槽位用于区分空和满。
    fn Cap(&self) -> i32 {
        let state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        state.cap.wrapping_sub(1) as i32
    }

    // Put 加锁写入 tail 指向的槽位；满时返回 false。
    fn Put(&self, val: u64) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        if state.tail.wrapping_sub(state.head) == state.cap.wrapping_sub(1) {
            return false;
        }

        let slot = (state.tail & state.cap.wrapping_sub(1)) as usize;
        state.slots[slot] = val as u32;
        state.tail = state.tail.wrapping_add(1);
        true
    }

    // Get 加锁读取 head 指向的槽位；空时返回 IDPoolInvalidValue 和 false。
    fn Get(&self) -> (u64, bool) {
        let mut state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        if state.head == state.tail {
            return (globalconn::IDPoolInvalidValue, false);
        }

        let slot = (state.head & state.cap.wrapping_sub(1)) as usize;
        let val = state.slots[slot] as u64;
        state.head = state.head.wrapping_add(1);
        (val, true)
    }
}

// String 对应 Go 的 fmt.Stringer 输出，保留 head/tail 和槽位快照。
impl fmt::Display for LockBasedCircularPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self
            .state
            .lock()
            .expect("LockBasedCircularPool mutex poisoned");
        let head = state.head;
        let tail = state.tail;
        let headVal = state.slots[(head & state.cap.wrapping_sub(1)) as usize];
        let tailVal = state.slots[(tail & state.cap.wrapping_sub(1)) as usize];
        let length = tail.wrapping_sub(head);

        write!(
            f,
            "cap:{}, len:{}; head:{:x}, slot:{{{:x}}}; tail:{:x}, slot:{{{:x}}}",
            state.cap, length, head, headVal, tail, tailVal
        )
    }
}

// IDPool 实现对应 Go 的 var _ globalconn.IDPool 编译期检查。
impl globalconn::IDPool for LockBasedCircularPool {
    fn Init(&mut self, size: u64) {
        LockBasedCircularPool::Init(self, size);
    }

    fn Len(&self) -> i32 {
        LockBasedCircularPool::Len(self)
    }

    fn Cap(&self) -> i32 {
        LockBasedCircularPool::Cap(self)
    }

    fn Put(&self, val: u64) -> bool {
        LockBasedCircularPool::Put(self, val)
    }

    fn Get(&self) -> (u64, bool) {
        LockBasedCircularPool::Get(self)
    }
}

/// 创建空的锁实现环形池并装箱为 `IDPool`。
// prepareLockBasedPool 对应 Go helper，创建空的锁实现环形池并作为 IDPool 返回。
fn prepareLockBasedPool(
    sizeInBits: u32,
    fillCount: u32,
) -> Arc<dyn globalconn::IDPool + Send + Sync> {
    let mut pool = LockBasedCircularPool::default();
    pool.InitExt(1u32 << sizeInBits, fillCount);
    Arc::new(pool)
}

/// 创建无锁池；`headPos > 0` 时用 `InitForTest` 推近溢出。
// prepareLockFreePool 对应 Go helper，可选设置 head/tail 到接近溢出的位置。
fn prepareLockFreePool(
    sizeInBits: u32,
    fillCount: u32,
    headPos: u32,
) -> Arc<dyn globalconn::IDPool + Send + Sync> {
    let mut pool = globalconn::LockFreeCircularPool::default();
    pool.InitExt(1u32 << sizeInBits, fillCount);
    if headPos > 0 {
        pool.InitForTest(headPos, fillCount);
    }

    Arc::new(pool)
}

/// 编排生产者/消费者线程（对齐 Go goroutine + channel 就绪/结束信号）。
// prepareConcurrencyTest 对应 Go 的 goroutine/channel/waitgroup 编排。
// Rust 用线程和共享标记表达生产者消费者结构，重点保留总和校验的迁移意图。
fn prepareConcurrencyTest(
    pool: Arc<dyn globalconn::IDPool + Send + Sync>,
    producers: usize,
    consumers: usize,
    requests: usize,
    total: Arc<AtomicI64>,
) -> (
    Vec<thread::JoinHandle<()>>,
    Vec<thread::JoinHandle<()>>,
    Arc<AtomicBoolPair>,
) {
    let flags = Arc::new(AtomicBoolPair::default());
    let mut producerHandles = Vec::new();
    let mut consumerHandles = Vec::new();

    if producers > 0 {
        let reqsPerProducer = (requests + producers - 1) / producers;
        for p in 0..producers {
            let pool = Arc::clone(&pool);
            let flags = Arc::clone(&flags);
            producerHandles.push(thread::spawn(move || {
                while !flags.ready() {
                    thread::yield_now();
                }

                let start = p * reqsPerProducer;
                let end = ((p + 1) * reqsPerProducer).min(requests);
                for i in start..end {
                    while !pool.Put(i as u64) {
                        // Go 这里在池满时 runtime.Gosched；用 yield_now 标注调度让步。
                        thread::yield_now();
                    }
                }
            }));
        }
    }

    if consumers > 0 {
        for _ in 0..consumers {
            let pool = Arc::clone(&pool);
            let flags = Arc::clone(&flags);
            let total = Arc::clone(&total);
            consumerHandles.push(thread::spawn(move || {
                while !flags.ready() {
                    thread::yield_now();
                }

                let mut sum = 0i64;
                loop {
                    let (val, ok) = pool.Get();
                    if ok {
                        sum += val as i64;
                        continue;
                    }
                    if flags.done() {
                        break;
                    }
                    thread::yield_now();
                }
                total.fetch_add(sum, Ordering::SeqCst);
            }));
        }
    }

    (producerHandles, consumerHandles, flags)
}

/// 用两个原子布尔模拟 Go 的 ready/done channel 关闭语义。
// AtomicBoolPair 机械表达 Go ready/done 两个 channel 的关闭状态。
#[derive(Default)]
struct AtomicBoolPair {
    ready: std::sync::atomic::AtomicBool,
    done: std::sync::atomic::AtomicBool,
}

impl AtomicBoolPair {
    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    fn done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

/// 先置 ready、等生产者，再置 done、等消费者。
// doConcurrencyTest 对应 Go 关闭 ready、等待生产者、关闭 done、等待消费者的顺序。
fn doConcurrencyTest(
    producerHandles: Vec<thread::JoinHandle<()>>,
    consumerHandles: Vec<thread::JoinHandle<()>>,
    flags: Arc<AtomicBoolPair>,
) {
    flags.ready.store(true, Ordering::SeqCst);
    for handle in producerHandles {
        handle.join().expect("producer thread should not panic");
    }
    flags.done.store(true, Ordering::SeqCst);
    for handle in consumerHandles {
        handle.join().expect("consumer thread should not panic");
    }
}

/// 计算并发测试期望总和（请求序号三角和 + 初始 fill 值三角和）。
// expectedConcurrencyTestResult 对应 Go 的期望总和计算，生产者请求和初始 fillCount 分别累加。
fn expectedConcurrencyTestResult(
    poolSizeInBits: u32,
    fillCount: u32,
    producers: usize,
    consumers: usize,
    requests: usize,
) -> i64 {
    let mut expected = 0i64;
    if producers > 0 && consumers > 0 {
        expected += (requests as i64 - 1) * requests as i64 / 2;
    }
    if fillCount > 0 {
        let fillCount = ((1u32 << poolSizeInBits) - 1).min(fillCount);
        expected += (1 + fillCount as i64) * fillCount as i64 / 2;
    }
    expected
}

/// 跑无锁池并发场景，返回 `(期望总和, 实际总和)`。
// testLockFreePoolConcurrency 对应 Go helper，运行无锁池并发测试并返回期望/实际总和。
fn testLockFreePoolConcurrency(
    poolSizeInBits: u32,
    fillCount: u32,
    producers: usize,
    consumers: usize,
    requests: usize,
    headPos: u32,
) -> (i64, i64) {
    let total = Arc::new(AtomicI64::new(0));
    let pool = prepareLockFreePool(poolSizeInBits, fillCount, headPos);
    let (wgProducer, wgConsumer, flags) =
        prepareConcurrencyTest(pool, producers, consumers, requests, Arc::clone(&total));

    doConcurrencyTest(wgProducer, wgConsumer, flags);

    let expected =
        expectedConcurrencyTestResult(poolSizeInBits, fillCount, producers, consumers, requests);
    (expected, total.load(Ordering::SeqCst))
}

/// 跑锁实现池并发场景，返回 `(期望总和, 实际总和)`。
// testLockBasedPoolConcurrency 对应 Go helper，使用锁实现池运行同样的并发总和校验。
fn testLockBasedPoolConcurrency(
    poolSizeInBits: u32,
    producers: usize,
    consumers: usize,
    requests: usize,
) -> (i64, i64) {
    let total = Arc::new(AtomicI64::new(0));
    let pool = prepareLockBasedPool(poolSizeInBits, 0);
    let (wgProducer, wgConsumer, flags) =
        prepareConcurrencyTest(pool, producers, consumers, requests, Arc::clone(&total));

    doConcurrencyTest(wgProducer, wgConsumer, flags);

    let expected = expectedConcurrencyTestResult(poolSizeInBits, 0, producers, consumers, requests);
    (expected, total.load(Ordering::SeqCst))
}

/// 无锁池基础并发安全 + head/tail 溢出场景。
// TestLockFreePoolBasicConcurrencySafety 对应 Go 的基础并发安全和 head/tail 溢出测试。
#[test]
fn TestLockFreePoolBasicConcurrencySafety() {
    const sizeInBits: u32 = 8;
    const fillCount: u32 = 0;
    const producers: usize = 20;
    const consumers: usize = 20;
    const requests: usize = 1 << 20;
    const headPos: u32 = 0x1_0000_0000u64.wrapping_sub(1 << (sizeInBits + 8)) as u32;

    let (expected, actual) =
        testLockFreePoolConcurrency(sizeInBits, fillCount, producers, consumers, requests, 0);
    assert_eq!(expected, actual);

    // test overflow of head & tail
    // Go 通过 InitForTest 把 head/tail 推到接近 uint32 溢出的位置。
    let (expected, actual) = testLockFreePoolConcurrency(
        sizeInBits, fillCount, producers, consumers, requests, headPos,
    );
    assert_eq!(expected, actual);
}

/// 锁实现池并发安全校验。
// TestLockBasedPoolConcurrencySafety 对应 Go 的锁实现并发安全校验。
#[test]
fn TestLockBasedPoolConcurrencySafety() {
    const sizeInBits: u32 = 8;
    const producers: usize = 20;
    const consumers: usize = 20;
    const requests: usize = 1 << 20;

    let (expected, actual) =
        testLockBasedPoolConcurrency(sizeInBits, producers, consumers, requests);
    assert_eq!(expected, actual);
}

/// 表驱动并发用例参数。
// poolConcurrencyTestCase 对应 Go 表驱动并发测试用例。
struct poolConcurrencyTestCase {
    sizeInBits: u32,
    fillCount: u32,
    producers: usize,
    consumers: usize,
    requests: usize,
}

impl fmt::Display for poolConcurrencyTestCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "size:{}, fillCount:{}, producers:{}, consumers:{}, requests:{}",
            1u64 << self.sizeInBits,
            self.fillCount,
            self.producers,
            self.consumers,
            self.requests
        )
    }
}

/// 五类典型并发场景（部分满/空/满队列的多线程 push/pop）。
// TestLockFreePoolConcurrencySafety 对应 Go 引用 C++ Concurrency in Action 的五类并发场景。
#[test]
fn TestLockFreePoolConcurrencySafety() {
    const poolSizeInBits: u32 = 16;
    const requests: usize = 1 << 20;
    const concurrency: usize = 1000;

    // Test cases from Anthony Williams, "C++ Concurrency in Action, 2nd", 11.2.2 "Locating concurrency-related bugs by testing":
    let cases = vec![
        // #1 Multiple threads calling pop() on a partially full queue with insufficient items for all threads
        poolConcurrencyTestCase {
            sizeInBits: 4,
            fillCount: 1 << 3,
            producers: 0,
            consumers: 32,
            requests,
        },
        // #2 Multiple threads calling push() while one thread calls pop() on an empty queue
        poolConcurrencyTestCase {
            sizeInBits: poolSizeInBits,
            fillCount: 0,
            producers: concurrency,
            consumers: 1,
            requests,
        },
        // #3 Multiple threads calling push() while one thread calls pop() on a full queue
        poolConcurrencyTestCase {
            sizeInBits: poolSizeInBits,
            fillCount: 0xffff_ffff,
            producers: concurrency,
            consumers: 1,
            requests,
        },
        // #4 Multiple threads calling push() while multiple threads call pop() on an empty queue
        poolConcurrencyTestCase {
            sizeInBits: poolSizeInBits,
            fillCount: 0,
            producers: concurrency,
            consumers: concurrency,
            requests,
        },
        // #5 Multiple threads calling push() while multiple threads call pop() on a full queue
        poolConcurrencyTestCase {
            sizeInBits: poolSizeInBits,
            fillCount: 0xffff_ffff,
            producers: concurrency,
            consumers: concurrency,
            requests,
        },
    ];

    for (i, ca) in cases.into_iter().enumerate() {
        let (expected, actual) = testLockFreePoolConcurrency(
            ca.sizeInBits,
            ca.fillCount,
            ca.producers,
            ca.consumers,
            requests,
            0,
        );
        assert_eq!(expected, actual, "case #{}: {}", i + 1, ca);
    }
}

/// 锁实现 vs 无锁实现的并发对照骨架（未接入 Rust bench）。
// BenchmarkPoolConcurrency 保留 Go benchmark 的锁实现和无锁实现对照；当前不接入 Rust bench harness。
fn BenchmarkPoolConcurrency() {
    const poolSizeInBits: u32 = 16;
    const requests: usize = 1 << 18;

    let cases = vec![
        poolConcurrencyTestCase {
            sizeInBits: 0,
            fillCount: 0,
            producers: 1,
            consumers: 1,
            requests: 0,
        },
        poolConcurrencyTestCase {
            sizeInBits: 0,
            fillCount: 0,
            producers: 3,
            consumers: 3,
            requests: 0,
        },
        poolConcurrencyTestCase {
            sizeInBits: 0,
            fillCount: 0,
            producers: 10,
            consumers: 10,
            requests: 0,
        },
        poolConcurrencyTestCase {
            sizeInBits: 0,
            fillCount: 0,
            producers: 20,
            consumers: 20,
            requests: 0,
        },
        poolConcurrencyTestCase {
            sizeInBits: 0,
            fillCount: 0,
            producers: 100,
            consumers: 100,
            requests: 0,
        },
    ];

    for ta in cases {
        // 子项: LockBasedCircularPool: P:C: producers:consumers。
        let total = Arc::new(AtomicI64::new(0));
        let pool = prepareLockBasedPool(poolSizeInBits, 0);
        let (wgProducer, wgConsumer, flags) = prepareConcurrencyTest(
            pool,
            ta.producers,
            ta.consumers,
            requests,
            Arc::clone(&total),
        );
        doConcurrencyTest(wgProducer, wgConsumer, flags);
        let expected =
            expectedConcurrencyTestResult(poolSizeInBits, 0, ta.producers, ta.consumers, requests);
        let actual = total.load(Ordering::SeqCst);
        assert_eq!(expected, actual, "concurrency safety fail");

        // 子项: LockFreeCircularPool: P:C: producers:consumers。
        let total = Arc::new(AtomicI64::new(0));
        let pool = prepareLockFreePool(poolSizeInBits, 0, 0);
        let (wgProducer, wgConsumer, flags) = prepareConcurrencyTest(
            pool,
            ta.producers,
            ta.consumers,
            requests,
            Arc::clone(&total),
        );
        doConcurrencyTest(wgProducer, wgConsumer, flags);
        let expected =
            expectedConcurrencyTestResult(poolSizeInBits, 0, ta.producers, ta.consumers, requests);
        let actual = total.load(Ordering::SeqCst);
        assert_eq!(expected, actual, "concurrency safety fail");
    }
}
