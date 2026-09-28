// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `memory/utils` 辅助结构的 Aster 单元测试。
//
// 覆盖 wrapList 槽位复用与顺序、Notifer 唤醒合并、哈希/分片/比例与
// `IntoRuntimeMemStats` 字段映射，行为对齐 Go 用例。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::*;

/// 验证 remove 后 pushBack 复用同一槽位，且 moveToFront/popFront 顺序正确。
#[test]
fn wrap_list_reuses_removed_slots_and_preserves_order() {
    let mut list = wrapList::new();
    assert_eq!(list.size(), 0);
    assert!(list.empty());
    assert_eq!(list.allocated_len(), 0);
    list.init();
    assert_eq!(list.size(), 0);
    assert_eq!(list.allocated_len(), 0);
    assert_eq!(list.popFront(), None);
    assert_eq!(list.front(), None);

    let first = list.pushBack(11);
    let second = list.pushBack(22);
    assert_eq!(list.front(), Some(&11));
    assert_eq!(list.allocated_len(), 2);

    list.remove(second);
    assert_eq!(list.size(), 1);
    let third = list.pushBack(33);
    assert_eq!(third.slot(), second.slot());
    assert_eq!(list.allocated_len(), 2);
    assert_eq!(list.popFront(), Some(11));
    assert_eq!(list.front(), Some(&33));

    let first_again = list.pushBack(11);
    list.moveToFront(first_again);
    assert_eq!(list.popFront(), Some(11));
    assert_eq!(list.popFront(), Some(33));
    assert!(list.approxEmpty());
    assert_eq!(list.approxSize(), 0);
    assert!(first.valid());

    let mut detached = first;
    detached.reset();
    assert!(!detached.valid());
}

/// 连续 Wake/WeakWake 应合并为一次 awake；Wait 后可再次唤醒。
#[test]
fn notifier_coalesces_wakes_and_can_wake_again_after_wait() {
    let notifier = Arc::new(NewNotifer());
    assert!(!notifier.isAwake());

    let producer = Arc::clone(&notifier);
    thread::spawn(move || {
        producer.Wake();
        producer.Wake();
        producer.WeakWake();
    })
    .join()
    .unwrap();

    assert!(notifier.isAwake());
    notifier.Wait();
    assert!(!notifier.isAwake());

    notifier.Wake();
    notifier.Wait();
    assert!(!notifier.isAwake());
}

/// 哈希分片奇偶分布、HashStr、配额分片桶、nextPow2 与千分比换算对齐 Go。
#[test]
fn hashes_shards_ratios_and_powers_match_go_cases() {
    const COUNT: u64 = 1 << 8;
    let odd = (0..COUNT)
        .filter(|i| shardIndexByUID(4_068_484_684 + i * 2, COUNT - 1) & 1 != 0)
        .count();
    assert_eq!(odd, COUNT as usize / 2);
    assert_eq!(HashStr("TiDB"), 8_506_710_899_071_881_958);

    assert_eq!(baseQuotaUnit * (1 << (27 - 2)), 128_i64 << 30);
    assert_eq!(getQuotaShard(0, 27), 0);
    assert_eq!(getQuotaShard(baseQuotaUnit - 1, 27), 0);
    assert_eq!(getQuotaShard(baseQuotaUnit, 27), 1);
    assert_eq!(getQuotaShard(baseQuotaUnit * 2 - 1, 27), 1);
    assert_eq!(getQuotaShard(baseQuotaUnit * 2, 27), 2);
    assert_eq!(getQuotaShard(baseQuotaUnit * 4 - 1, 27), 2);
    assert_eq!(getQuotaShard(baseQuotaUnit * 4, 27), 3);
    assert_eq!(getQuotaShard(baseQuotaUnit * (1 << (27 - 2)) - 1, 27), 25);
    assert_eq!(getQuotaShard(baseQuotaUnit * (1 << (27 - 2)), 27), 26);
    assert_eq!(getQuotaShard(i64::MAX, 27), 26);

    for shift in 0..63 {
        let value = 1_u64 << shift;
        assert_eq!(nextPow2(value), value);
        if value > 2 {
            assert_eq!(nextPow2(value - 1), value);
        }
    }
    assert_eq!(nextPow2(0), 1);
    assert_eq!(calcRatio(1, 4), 250);
    assert_eq!(multiRatio(200, 250), 50);
    assert_eq!(intoRatio(0.125), 125);
}

/// 时间单调递增，且 RustMemStats→RuntimeMemStats 字段映射与 Go 一致。
#[test]
fn time_and_runtime_stat_conversion_match_go_field_mapping() {
    let before = nowUnixMilli();
    thread::sleep(Duration::from_millis(1));
    assert!(nowUnixMilli() >= before);
    assert!(nowUnixSec() > 0);

    let input = RustMemStats {
        HeapAlloc: 10,
        HeapInuse: 20,
        TotalAlloc: 90,
        Alloc: 30,
        Sys: 100,
        HeapSys: 70,
        NumGC: 4,
    };
    let output = IntoRuntimeMemStats(&input);
    assert_eq!(output.HeapAlloc, 10);
    assert_eq!(output.HeapInuse, 20);
    assert_eq!(output.TotalFree, 60);
    assert_eq!(output.MemOffHeap, 30);
    assert_eq!(output.NumGC, 4);

    let sampled = SampleRuntimeMemStats();
    assert!(sampled.HeapInuse >= sampled.HeapAlloc);
}
