// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// LFU 统计缓存的单元与并发测试。
//
// 覆盖 Put/Get/Del、内存成本跟踪、超大条目拒绝、长度与壳表淘汰状态、
// 多线程并发读写、容量缩减内存控制，以及 Copy/Close 的共享与幂等语义。

use statistics::{AllEvicted, Table};
use std::sync::{Arc, Barrier};
use std::thread;

use crate::*;

/// mock CMS（Count-Min Sketch）单份占用的内存常量，用于断言成本。
const mockCMSMemoryUsage: i64 = 4;

/// 构造带指定列/索引及 CMS、TopN、直方图开关的 mock 统计表。
fn table(columns: i32, indices: i32, cms: bool, topn: bool, hist: bool) -> Arc<Table> {
    Arc::new(testutil::NewMockStatisticsTable(
        columns, indices, cms, topn, hist,
    ))
}

/// 断言 `Cost == CostAdded - CostEvicted`，保证指标与成本一致。
fn assertMetricsMatchCost(lfu: &LFU) {
    assert_eq!(
        lfu.Cost() as u64,
        lfu.metrics().CostAdded() - lfu.metrics().CostEvicted()
    );
}

/// 基本 Put → Get → Del 流程，并确认 Values 为空且指标对齐。
#[test]
fn TestLFUPutGetDel() {
    let lfu = NewLFU(100).unwrap();
    lfu.Put(1, table(1, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    lfu.Del(1);
    assert!(lfu.Get(1).is_none());
    lfu.WaitForAsyncUpdates();
    assertMetricsMatchCost(&lfu);
    assert!(lfu.Values().is_empty());
}

/// 多次更新同一键时，跟踪内存随列/索引规模变化而增减。
#[test]
fn TestLFUFreshMemUsage() {
    let lfu = NewLFU(10_000).unwrap();
    let t1 = table(1, 1, true, false, false);
    assert_eq!(t1.MemoryUsage().TotalMemUsage, 2 * mockCMSMemoryUsage);
    let t2 = table(2, 2, true, false, false);
    assert_eq!(t2.MemoryUsage().TotalMemUsage, 4 * mockCMSMemoryUsage);
    let t3 = table(3, 3, true, false, false);
    assert_eq!(t3.MemoryUsage().TotalMemUsage, 6 * mockCMSMemoryUsage);
    lfu.Put(1, t1);
    lfu.Put(2, t2);
    lfu.Put(3, t3);
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 12 * mockCMSMemoryUsage);

    lfu.Put(1, table(2, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 13 * mockCMSMemoryUsage);
    lfu.Put(1, table(2, 2, true, false, false));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 14 * mockCMSMemoryUsage);

    lfu.Put(1, table(1, 2, true, false, false));
    assert_eq!(lfu.Cost(), 13 * mockCMSMemoryUsage);
    lfu.Put(1, table(1, 1, true, false, false));
    assert_eq!(lfu.Cost(), 12 * mockCMSMemoryUsage);
    lfu.WaitForAsyncUpdates();
    assertMetricsMatchCost(&lfu);
}

/// 单表成本超过容量时仍可通过二级集合 Get 到，验证拒绝路径先发布壳表。
#[test]
fn TestLFUPutTooBig() {
    let lfu = NewLFU(1).unwrap();
    lfu.Put(1, table(1, 1, true, false, false));
    // resultKeySet is published before TinyLFU admission finishes.
    // 二级集合在 TinyLFU 准入完成前已发布，故 Get 仍能命中。
    assert!(lfu.Get(1).is_some());
    lfu.WaitForAsyncUpdates();
    assertMetricsMatchCost(&lfu);
}

/// 验证 Len 统计二级集合键数，且超容写入后 Cost 反映淘汰后的跟踪内存。
#[test]
fn TestCacheLen() {
    let lfu = NewLFU(12).unwrap();
    let t1 = table(2, 1, true, false, false);
    assert_eq!(t1.MemoryUsage().TotalTrackingMemUsage(), 12);
    lfu.Put(1, t1);
    lfu.Put(2, table(1, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Len(), 2);
    assert_eq!(lfu.Cost(), 8);

    lfu.Put(3, table(2, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Len(), 3);
    assert_eq!(lfu.Cost(), 12);
}

/// 32 线程分片写入 1000 键，验证并发 Put/Get 后长度与 Values 完整。
#[test]
fn TestLFUCachePutGetWithManyConcurrency() {
    let lfu = Arc::new(NewLFU(100_000_000_000).unwrap());
    let barrier = Arc::new(Barrier::new(33));
    thread::scope(|scope| {
        for worker in 0..32 {
            let lfu = Arc::clone(&lfu);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for key in (worker..1000).step_by(32) {
                    lfu.Put(key, table(1, 1, true, false, false));
                    let _ = lfu.Get(key);
                }
            });
        }
        barrier.wait();
    });
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Len(), 1000);
    assertMetricsMatchCost(&lfu);
    assert_eq!(lfu.Values().len(), 1000);
}

/// 写线程与读线程同时跑满 0..1000，验证最终 Values 齐全且指标对齐。
#[test]
fn TestLFUCachePutGetWithManyConcurrency2() {
    let lfu = Arc::new(NewLFU(100_000_000_000).unwrap());
    thread::scope(|scope| {
        for _ in 0..5 {
            let lfu = Arc::clone(&lfu);
            scope.spawn(move || {
                for key in 0..1000 {
                    lfu.Put(key, table(1, 1, true, false, false));
                }
            });
        }
        for _ in 0..5 {
            let lfu = Arc::clone(&lfu);
            scope.spawn(move || {
                for key in 0..1000 {
                    let _ = lfu.Get(key);
                }
            });
        }
    });
    lfu.WaitForAsyncUpdates();
    assertMetricsMatchCost(&lfu);
    assert_eq!(lfu.Values().len(), 1000);
}

/// 小容量下并发反复 Put，随后并发 Get 校验壳表；确认列/索引均为 AllEvicted。
#[test]
fn TestLFUCachePutGetWithManyConcurrencyAndSmallConcurrency() {
    let lfu = Arc::new(NewLFU(100).unwrap());
    thread::scope(|scope| {
        for _ in 0..5 {
            let lfu = Arc::clone(&lfu);
            scope.spawn(move || {
                for _ in 0..1000 {
                    for key in 0..50 {
                        lfu.Put(key, table(1, 1, true, true, true));
                    }
                }
            });
        }
    });
    lfu.WaitForAsyncUpdates();
    thread::scope(|scope| {
        for _ in 0..5 {
            let lfu = Arc::clone(&lfu);
            scope.spawn(move || {
                for _ in 0..1000 {
                    for key in 0..50 {
                        checkTable(&lfu.Get(key).expect("secondary key set lost table"));
                    }
                }
            });
        }
    });
    lfu.WaitForAsyncUpdates();
    let value = lfu.Get(17).unwrap();
    value.ForEachColumnImmutable(|_, column| {
        assert_eq!(column.GetEvictedStatus(), AllEvicted);
        true
    });
    value.ForEachIndexImmutable(|_, index| {
        assert_eq!(index.GetEvictedStatus(), AllEvicted);
        true
    });
}

/// 校验表列/索引：已全部淘汰则无 TopN 且直方图桶容量为 0，否则反之。
fn checkTable(table: &Table) {
    table.ForEachColumnImmutable(|_, column| {
        if column.GetEvictedStatus() == AllEvicted {
            assert!(column.TopN.is_none());
            assert_eq!(column.Histogram.Buckets.capacity(), 0);
        } else {
            assert!(column.TopN.is_some());
            assert!(column.Histogram.Buckets.capacity() > 0);
        }
        true
    });
    table.ForEachIndexImmutable(|_, index| {
        if index.GetEvictedStatus() == AllEvicted {
            assert!(index.TopN.is_none());
            assert_eq!(index.Histogram.Buckets.capacity(), 0);
        } else {
            assert!(index.TopN.is_some());
            assert!(index.Histogram.Buckets.capacity() > 0);
        }
        true
    });
}

/// 缩容至小于条目成本后 Put 新键，验证拒绝计数增加且壳表全部淘汰。
#[test]
fn TestLFUReject() {
    let lfu = NewLFU(100_000_000_000).unwrap();
    let itemCost = 3 * mockCMSMemoryUsage;
    lfu.Put(1, table(2, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), itemCost);
    lfu.SetCapacity(itemCost - 1);
    assert!(lfu.Put(2, table(2, 1, true, false, false)));
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 0);
    assert_eq!(lfu.Values().len(), 2);
    checkAllEvicted(&lfu.Get(2).unwrap());
    assert!(lfu.metrics().Rejections() > 0);
}

/// 断言表上所有列与索引的淘汰状态均为 `AllEvicted`。
fn checkAllEvicted(table: &Table) {
    table.ForEachColumnImmutable(|_, column| {
        assert_eq!(column.GetEvictedStatus(), AllEvicted);
        true
    });
    table.ForEachIndexImmutable(|_, index| {
        assert_eq!(index.GetEvictedStatus(), AllEvicted);
        true
    });
}

/// 逐步下调容量，验证 Cost 精确跟随新上限（含容量为 0 时保持当前成本）。
#[test]
fn TestMemoryControl() {
    let lfu = NewLFU(100_000_000_000).unwrap();
    let itemCost = 3 * mockCMSMemoryUsage;
    for key in 1..=1000 {
        let value = table(2, 1, true, false, false);
        assert_eq!(value.MemoryUsage().TotalTrackingMemUsage(), itemCost);
        lfu.Put(key, value);
    }
    assert_eq!(lfu.Cost(), 1000 * itemCost);

    for count in (990..1000).rev() {
        lfu.SetCapacity(count * itemCost);
        lfu.WaitForAsyncUpdates();
        assert_eq!(lfu.Cost(), count * itemCost);
    }
    for count in (100..990).rev().step_by(100) {
        lfu.SetCapacity(count * itemCost);
        lfu.WaitForAsyncUpdates();
        assert_eq!(lfu.Cost(), count * itemCost);
    }
    lfu.SetCapacity(10 * itemCost);
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 10 * itemCost);
    lfu.SetCapacity(0);
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 10 * itemCost);
}

/// 同一键反复更新为更大表，最终因超容被拒绝后 Cost 为 0。
#[test]
fn TestMemoryControlWithUpdate() {
    let lfu = NewLFU(100).unwrap();
    for columns in 0..100 {
        lfu.Put(1, table(columns, 1, true, false, false));
    }
    lfu.WaitForAsyncUpdates();
    assert_eq!(lfu.Cost(), 0);
}

/// Copy 与原实例共享状态；Close 幂等且关闭后 Put 返回 false。
#[test]
fn shared_copy_and_close_are_idempotent() {
    let lfu = NewLFU(100).unwrap();
    lfu.Put(1, table(1, 1, true, false, false));
    lfu.WaitForAsyncUpdates();
    let mut copy = lfu.Copy();
    copy.Del(1);
    copy.WaitForAsyncUpdates();
    assert!(lfu.Get(1).is_none());
    lfu.Close();
    lfu.Close();
    assert!(!lfu.Put(2, table(1, 1, true, false, false)));
}
