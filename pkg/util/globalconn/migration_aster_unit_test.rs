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

// AsterSQL 迁移补充单测：GCID、AutoIncPool、LockFreeCircularPool 与分配器行为。
//
// 相对 Go 原测，额外用并发生产者/消费者校验无锁池取值完整性，并覆盖保留号、
// 升级路径与释放复用等分配器语义。

use std::sync::Arc;
use std::thread;

use crate::*;

/// 固定 ServerID=1001 的 getter，供 GlobalAllocator 测试使用。
fn server_1001() -> u64 {
    1001
}

/// 32/64 位编码与解析边界（含溢出与截断）。
#[test]
fn gcid_encoding_and_parsing_match_go_boundaries() {
    let id64 = GCID {
        ServerID: 1001,
        LocalConnID: 123,
        Is64bits: true,
    }
    .ToConnID();
    assert_eq!((1001u64 << 41) | (123u64 << 1) | 1, id64);
    let (parsed64, truncated) = ParseConnID(id64).unwrap();
    assert!(!truncated);
    assert_eq!(
        (1001, 123, true),
        (parsed64.ServerID, parsed64.LocalConnID, parsed64.Is64bits)
    );

    let id32 = GCID {
        ServerID: 2002,
        LocalConnID: 321,
        Is64bits: false,
    }
    .ToConnID();
    assert_eq!((2002u64 << 21) | (321u64 << 1), id32);
    let (parsed32, truncated) = ParseConnID(id32).unwrap();
    assert!(!truncated);
    assert_eq!(
        (2002, 321, false),
        (parsed32.ServerID, parsed32.LocalConnID, parsed32.Is64bits)
    );

    assert!(ParseConnID(0x8000_0000_0000_0321).is_err());
    assert_eq!(true, ParseConnID(101).unwrap().1);
    assert!(ParseConnID(0x1_0000_0320).is_err());
}

/// 字段超出位宽时应 panic（对齐 Go）。
#[test]
fn gcid_encoding_rejects_fields_outside_go_bit_layout() {
    assert!(
        std::panic::catch_unwind(|| GCID {
            ServerID: 1 << 22,
            LocalConnID: 123,
            Is64bits: true
        }
        .ToConnID())
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| GCID {
            ServerID: 1001,
            LocalConnID: 1 << 40,
            Is64bits: true
        }
        .ToConnID())
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| GCID {
            ServerID: 1 << 11,
            LocalConnID: 123,
            Is64bits: false
        }
        .ToConnID())
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| GCID {
            ServerID: 1001,
            LocalConnID: 1 << 20,
            Is64bits: false
        }
        .ToConnID())
        .is_err()
    );
}

/// AutoIncPool：顺序发放、回绕、耗尽与 Put 后复用。
#[test]
fn auto_inc_pool_matches_go_wrap_exhaust_and_release_sequence() {
    let mut pool = AutoIncPool::default();
    pool.InitExt(8, true, 4);
    let allocated: Vec<_> = (0..8).map(|_| pool.Get()).collect();
    assert_eq!(
        vec![
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, true),
            (7, true),
            (0, true)
        ],
        allocated
    );
    assert_eq!(8, pool.Len());
    assert_eq!((0, false), pool.Get());
    pool.Put(5);
    assert_eq!((5, true), pool.Get());
}

/// 无锁池：满初始化、取空、FIFO 放满再取。
#[test]
fn lock_free_pool_matches_go_full_empty_and_fifo_behavior() {
    let mut pool = LockFreeCircularPool::default();
    pool.InitExt(8, u32::MAX);
    assert_eq!(7, pool.Cap());
    assert_eq!(7, pool.Len());
    for expected in 1..=7 {
        assert_eq!((expected, true), pool.Get());
    }
    assert_eq!((IDPoolInvalidValue, false), pool.Get());
    for value in 10..=16 {
        assert!(pool.Put(value));
    }
    assert!(!pool.Put(17));
    for expected in 10..=16 {
        assert_eq!((expected, true), pool.Get());
    }
}

/// 多生产者/单消费者并发下，无锁池取值集合应完整无损。
#[test]
fn lock_free_pool_preserves_values_under_concurrent_producers_and_consumers() {
    let mut pool = LockFreeCircularPool::default();
    pool.InitExt(256, 0);
    let pool = Arc::new(pool);
    let producers: Vec<_> = (0..4)
        .map(|worker| {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                for value in (worker * 1000)..((worker + 1) * 1000) {
                    while !pool.Put(value) {
                        thread::yield_now();
                    }
                }
            })
        })
        .collect();
    let consumer_pool = Arc::clone(&pool);
    let consumer = thread::spawn(move || {
        let mut values = Vec::with_capacity(4000);
        while values.len() < 4000 {
            let (value, ok) = consumer_pool.Get();
            if ok {
                values.push(value);
            } else {
                thread::yield_now();
            }
        }
        values
    });
    for producer in producers {
        producer.join().unwrap();
    }
    let mut values = consumer.join().unwrap();
    values.sort_unstable();
    assert_eq!((0u64..4000).collect::<Vec<_>>(), values);
}

/// 分配器：保留号、32 位分配、释放后继续分配、64 位保留号编码。
#[test]
fn allocators_match_go_reserved_upgrade_release_and_reuse_behavior() {
    let simple = NewSimpleAllocator();
    assert_eq!(u64::MAX, simple.GetReservedConnID(0));
    assert_eq!(u64::MAX - 1, simple.GetReservedConnID(1));

    let global = GlobalAllocator::NewGlobalAllocator(server_1001, true);
    assert!(!global.is64());
    let first = global.Allocate();
    assert_eq!(
        (1001, 1, false),
        (first.ServerID, first.LocalConnID, first.Is64bits)
    );
    global.Release(first.ToConnID());
    let reused = global.Allocate();
    assert_eq!(
        (1001, 2, false),
        (reused.ServerID, reused.LocalConnID, reused.Is64bits)
    );

    let max_local = (1u64 << 40) - 1;
    assert_eq!(
        (1001u64 << 41) | (max_local << 1) | 1,
        global.GetReservedConnID(0)
    );
}
