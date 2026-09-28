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

// GCID 编解码与保留号分配的单元测试（对齐 Go `globalconn_test.go`）。
//
// 覆盖 `ToConnID` 边界 panic、`ParseConnID` 溢出/截断/32·64 位路径，以及
// Simple/Global 分配器的保留连接号；并保留本地池 benchmark 骨架。

use super::pool_test::LockBasedCircularPool;
use crate as globalconn;
use crate::Allocator;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// 单条 `ToConnID` 用例：输入 GCID、是否应 panic、期望编码。
// Case 对应 TestToConnID 内部的 Go 局部结构体，记录输入 GCID、是否应 panic 以及期望编码。
struct Case {
    gcid: globalconn::GCID,
    shouldPanic: bool,
    expected: u64,
}

/// 校验 32/64 位位布局编码与越界 panic。
// TestToConnID 对应 Go 的连接 ID 位编码测试，覆盖 64 位和 32 位边界。
#[test]
fn TestToConnID() {
    let cases = vec![
        Case {
            gcid: globalconn::GCID {
                Is64bits: true,
                ServerID: 1001,
                LocalConnID: 123,
            },
            shouldPanic: false,
            expected: (1001u64 << 41) | (123u64 << 1) | 1,
        },
        Case {
            gcid: globalconn::GCID {
                Is64bits: true,
                ServerID: 1 << 22,
                LocalConnID: 123,
            },
            shouldPanic: true,
            expected: 0,
        },
        Case {
            gcid: globalconn::GCID {
                Is64bits: true,
                ServerID: 1001,
                LocalConnID: 1 << 40,
            },
            shouldPanic: true,
            expected: 0,
        },
        Case {
            gcid: globalconn::GCID {
                Is64bits: false,
                ServerID: 1001,
                LocalConnID: 123,
            },
            shouldPanic: false,
            expected: (1001u64 << 21) | (123u64 << 1),
        },
        Case {
            gcid: globalconn::GCID {
                Is64bits: false,
                ServerID: 1 << 11,
                LocalConnID: 123,
            },
            shouldPanic: true,
            expected: 0,
        },
        Case {
            gcid: globalconn::GCID {
                Is64bits: false,
                ServerID: 1001,
                LocalConnID: 1 << 20,
            },
            shouldPanic: true,
            expected: 0,
        },
    ];

    for c in cases {
        if c.shouldPanic {
            // Go 使用 assert.Panics 包住 ToConnID；用 catch_unwind 表达同一断言。
            assert!(std::panic::catch_unwind(|| c.gcid.ToConnID()).is_err());
        } else {
            assert_eq!(c.expected, c.gcid.ToConnID());
        }
    }
}

/// 校验 `ParseConnID`：int64 溢出、64 位截断、正常 64/32 位解析。
// TestGlobalConnID 对应 Go 的 ParseConnID 解析测试，覆盖溢出、截断、64 位和 32 位路径。
#[test]
fn TestGlobalConnID() {
    // exceeds int64
    assert!(globalconn::ParseConnID(0x80000000_00000321).is_err());

    // 64bits truncated
    let (_, isTruncated) = globalconn::ParseConnID(101).expect("Go 测试期望截断场景无错误");
    assert!(isTruncated);

    // 64bits
    let id1 = (1001u64 << 41) | (123u64 << 1) | 1;
    let (gcid1, isTruncated) = globalconn::ParseConnID(id1).expect("Go 测试期望 64 位解析成功");
    assert!(!isTruncated);
    assert_eq!(1001u64, gcid1.ServerID);
    assert_eq!(123u64, gcid1.LocalConnID);
    assert!(gcid1.Is64bits);

    // exceeds uint32
    assert!(globalconn::ParseConnID(0x1_00000320).is_err());

    // 32bits
    let id2 = (2002u64 << 21) | (321u64 << 1);
    let (gcid2, isTruncated) = globalconn::ParseConnID(id2).expect("Go 测试期望 32 位解析成功");
    assert!(!isTruncated);
    assert_eq!(2002u64, gcid2.ServerID);
    assert_eq!(321u64, gcid2.LocalConnID);
    assert!(!gcid2.Is64bits);
    assert_eq!(gcid2.ToConnID(), id2);
}

/// 校验 Simple/Global 分配器的保留连接号编码。
// TestGetReservedConnID 对应 Go 的内部保留连接 ID 分配测试。
#[test]
fn TestGetReservedConnID() {
    let simpleAlloc = globalconn::NewSimpleAllocator();
    assert_eq!(u64::MAX - 0u64, simpleAlloc.GetReservedConnID(0));
    assert_eq!(u64::MAX - 1u64, simpleAlloc.GetReservedConnID(1));

    let serverID = || -> u64 { 1001 };

    let globalAlloc = globalconn::GlobalAllocator::NewGlobalAllocator(serverID, true);
    let maxLocalConnID: u64 = (1u64 << 40) - 1;
    assert_eq!(
        (1001u64 << 41) | (maxLocalConnID << 1) | 1,
        globalAlloc.GetReservedConnID(0)
    );
    assert_eq!(
        (1001u64 << 41) | ((maxLocalConnID - 1) << 1) | 1,
        globalAlloc.GetReservedConnID(1)
    );
}

/// Go 的 `serverIDGetter func() uint64` 可以捕获状态，且每次分配都会重新读取。
#[test]
fn TestGlobalAllocatorAcceptsCapturingServerIDGetter() {
    let server_id = Arc::new(AtomicU64::new(1001));
    let getter_state = Arc::clone(&server_id);
    let allocator = globalconn::GlobalAllocator::NewGlobalAllocator(
        move || getter_state.load(Ordering::SeqCst),
        true,
    );

    assert_eq!(1001, allocator.Allocate().ServerID);
    server_id.store(1002, Ordering::SeqCst);
    assert_eq!(1002, allocator.Allocate().ServerID);
}

/// 单次 32 位本地 ID：循环 Get 直至成功，再 Put 归还。
// benchmarkLocalConnIDAllocator32 对应 Go benchmark helper：循环等待池里拿到 ID，再归还。
fn benchmarkLocalConnIDAllocator32(pool: &dyn globalconn::IDPool) {
    let (mut id, mut ok) = (0u64, false);

    // allocate local conn ID.
    while !ok {
        let got = pool.Get();
        id = got.0;
        ok = got.1;
        if !ok {
            // Go 这里调用 runtime.Gosched；用 yield_now 标注调度让步语义。
            std::thread::yield_now();
        }
    }

    // deallocate local conn ID.
    ok = pool.Put(id);
    if !ok {
        panic!("pool unexpected full");
    }
}

/// 保留 Go 多并发 benchmark 子项结构（当前未接入 Rust bench harness）。
// BenchmarkLocalConnIDAllocator 保留 Go benchmark 的并行分配场景；当前不接入 Rust benchmark harness。
fn BenchmarkLocalConnIDAllocator() {
    let concurrencyCases = vec![1, 3, 10, 20, 100];
    for concurrency in concurrencyCases {
        // Go b.Run("Allocator 64 xN") + b.RunParallel；这里用注释保留 benchmark 子项和并行语义。
        // 子项: Allocator 64 x{concurrency}，使用 AutoIncPool 和 LocalConnIDAllocator64TryCount。
        let mut pool = globalconn::AutoIncPool::default();
        pool.InitExt(
            1u64 << globalconn::LocalConnIDBits64,
            true,
            globalconn::LocalConnIDAllocator64TryCount,
        );
        let _parallelism = concurrency;
        let _ = pool.Get();

        // 子项: Allocator 32(LockBased) x{concurrency}，Go 版依赖 pool_test.go 内的锁实现。
        let mut lock_based_pool = LockBasedCircularPool::default();
        lock_based_pool.InitExt(1u32 << unsafe { globalconn::LocalConnIDBits32 }, u32::MAX);
        benchmarkLocalConnIDAllocator32(&lock_based_pool);

        // 子项: Allocator 32(LockFreeCircularPool) x{concurrency}，对照无锁环形池。
        let mut lock_free_pool = globalconn::LockFreeCircularPool::default();
        lock_free_pool.InitExt(1u32 << unsafe { globalconn::LocalConnIDBits32 }, u32::MAX);
        benchmarkLocalConnIDAllocator32(&lock_free_pool);
    }
}
