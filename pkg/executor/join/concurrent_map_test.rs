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

// `ConcurrentMap` 单元测试。
//
// 验证并发插入后冲突链完整、迭代覆盖全部头节点，以及插入内存增量精确可核对。
// 对应 Go `concurrent_map_test.go`。

/*
//

#![allow(dead_code, non_snake_case, unused_variables)]

// TestConcurrentMap first inserts 1000 entries, then checks them
// TestConcurrentMap 对应 Go 的并发插入测试：两个 goroutine 共享 map，插入后按 hash bucket 链表查回每个 row pointer。
#[test]
pub fn TestConcurrentMap() {
    let mut m = newConcurrentMap();
    const iterations: i32 = 1000;
    const mod_value: i32 = 111;
    let mut wg = sync::WaitGroup::new();
    wg.Add(2);

    // Using go routines insert 1000 entries into the map.
    wg.Go(|| {
        defer!(wg.Done());
        for i in 0..iterations / 2 {
            // Add entry to map.
            m.Insert(
                (i % mod_value) as u64,
                entry { Ptr: chunk::RowPtr { ChkIdx: i as u32, RowIdx: i as u32 }, Next: nil },
            );
        }
    });

    wg.Go(|| {
        defer!(wg.Done());
        for i in iterations / 2..iterations {
            // Add entry to map.
            m.Insert(
                (i % mod_value) as u64,
                entry { Ptr: chunk::RowPtr { ChkIdx: i as u32, RowIdx: i as u32 }, Next: nil },
            );
        }
    });
    wg.Wait();

    // check whether i exist in the map, surely
    for i in 0..iterations {
        let mut found = false;
        let (mut en, mut ok) = m.Get((i % mod_value) as u64);
        while en != nil {
            require::True(ok);
            if en.Ptr.RowIdx == i as u32 && en.Ptr.ChkIdx == i as u32 {
                found = true;
            }
            en = en.Next;
        }
        require::True(found);
    }

    // test some unexpected cases
    let (_, ok) = m.Get(mod_value as u64);
    require::False(ok);

    let (_, ok) = m.Get((mod_value + 1) as u64);
    require::False(ok);
}

// TestConcurrentMapMemoryUsage 对应 Go 的内存统计测试：并发插入后校验 arena 追踪值和实际分配值。
#[test]
pub fn TestConcurrentMapMemoryUsage() {
    let mut m = newConcurrentMap();
    let mut memUsage: i64 = 0;
    for s in &m {
        memUsage += s.items.Bytes as i64;
    }
    let iterations = 1024 * 10;
    let mut wg = sync::WaitGroup::new();
    wg.Add(2);

    // Using go routines insert 1000 entries into the map.
    wg.Go(|| {
        defer!(wg.Done());
        let mut memDelta: i64 = 0;
        for i in 0..iterations / 2 {
            // Add entry to map.
            memDelta += m.Insert(i as u64, entry { Ptr: chunk::RowPtr { ChkIdx: i as u32, RowIdx: i as u32 }, Next: nil });
        }
        // Go 使用 atomic.AddInt64 汇总两个 goroutine 的 delta，迁移时应替换为原子或锁。
        atomic::AddInt64(&memUsage, memDelta);
    });

    wg.Go(|| {
        defer!(wg.Done());
        let mut memDelta: i64 = 0;
        for i in iterations / 2..iterations {
            // Add entry to map.
            memDelta += m.Insert(i as u64, entry { Ptr: chunk::RowPtr { ChkIdx: i as u32, RowIdx: i as u32 }, Next: nil });
        }
        atomic::AddInt64(&memUsage, memDelta);
    });
    wg.Wait();

    require::Equal(283840_i64, memUsage);
    let mut realSize: i64 = 0;
    for s in &m {
        realSize += s.items.RealBytes() as i64;
    }
    require::Equal(376320_i64, realSize);
}
*/

use crate::concurrent_map::{ConcurrentMap, SHARD_COUNT};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;

/// 多线程按取模制造冲突，插入后沿冲突链应能找回每一个值。
#[test]
fn concurrent_map_preserves_every_collision_chain_entry() {
    let map = Arc::new(ConcurrentMap::new());
    // 四个线程各插入 250 个值，key 对 111 取模以制造冲突链。
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let map = map.clone();
            thread::spawn(move || {
                for value in worker * 250..(worker + 1) * 250 {
                    map.insert((value % 111) as u64, value);
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("concurrent insertion");
    }
    // 遍历冲突链确认每个插入值都存在。
    for value in 0..1000 {
        let mut entry = map.get((value % 111) as u64);
        let mut found = false;
        while let Some(current) = entry {
            found |= current.value == value;
            entry = current.next.clone();
        }
        assert!(found, "missing collision-chain value {value}");
    }
    assert!(map.get(111).is_none());
    assert_eq!(SHARD_COUNT, 320);
}

/// 无冲突键时 for_each 访问每个头节点，且累计内存增量等于实时容量统计。
#[test]
fn concurrent_map_iteration_visits_each_head_and_memory_delta_matches_capacity() {
    let map = Arc::new(ConcurrentMap::new());
    let delta = Arc::new(AtomicI64::new(0));
    let workers: Vec<_> = [(0, 5_120), (5_120, 10_240)]
        .into_iter()
        .map(|(start, end)| {
            let map = map.clone();
            let delta = delta.clone();
            thread::spawn(move || {
                let local_delta = (start..end).map(|key| map.insert(key, key)).sum();
                delta.fetch_add(local_delta, Ordering::Relaxed);
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("concurrent insertion");
    }
    let mut visited = 0;
    map.for_each(|_, head| {
        // 唯一键插入，冲突链应只有头节点。
        assert!(head.next.is_none());
        visited += 1;
    });
    assert_eq!(visited, 10_240);
    assert_eq!(delta.load(Ordering::Relaxed), map.real_memory_bytes());
}
