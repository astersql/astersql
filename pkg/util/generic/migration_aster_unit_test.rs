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

// generic 包迁移补充单元测试。
//
// 覆盖 `BoundedMinHeap` 的 top-N 顺序、反向比较器、零/负容量，以及
// `SyncMap` 的 Store/Load/Delete/Keys、并发写入与负容量 panic，对齐 Go。

#![cfg(test)]
#![allow(dead_code, non_snake_case, non_camel_case_types)]

use super::{NewBoundedMinHeap, NewSyncMap};
use std::sync::Arc;

/// 将 `Ord` 比较结果转为 Go 风格 i32（Less=-1, Equal=0, Greater=1）。
fn int_comparator(a: &i32, b: &i32) -> i32 {
    a.cmp(b) as i32
}

/// 容量 3 时保留最大的三个整数，排序从最好到最差。
#[test]
fn bounded_heap_keeps_best_items_in_go_order() {
    let mut heap = NewBoundedMinHeap(3, int_comparator);
    assert_eq!(None, heap.ToSortedSlice());

    for item in [5, 3, 8, 1, 9, 2, 7, 4] {
        heap.Add(item);
    }

    assert_eq!(3, heap.Len());
    assert_eq!(Some(vec![9, 8, 7]), heap.ToSortedSlice());
}

/// 反向比较器保留最小值；重复值满堆后相等项不挤占。
#[test]
fn bounded_heap_honors_reverse_comparator_and_duplicates() {
    let mut smallest = NewBoundedMinHeap(3, |a: &i32, b: &i32| -int_comparator(a, b));
    for item in [9, 2, 7, 1, 8, 3] {
        smallest.Add(item);
    }
    assert_eq!(Some(vec![1, 2, 3]), smallest.ToSortedSlice());

    let mut duplicates = NewBoundedMinHeap(3, int_comparator);
    for item in [5, 5, 3, 8, 5] {
        duplicates.Add(item);
    }
    assert_eq!(Some(vec![8, 5, 5]), duplicates.ToSortedSlice());
}

/// 零容量 Add 无效；负容量构造 panic。
#[test]
fn bounded_heap_handles_zero_and_negative_capacity_like_go() {
    let mut empty = NewBoundedMinHeap(0, int_comparator);
    empty.Add(10);
    assert_eq!(0, empty.Len());
    assert_eq!(None, empty.ToSortedSlice());

    assert!(std::panic::catch_unwind(|| NewBoundedMinHeap(-1, int_comparator)).is_err());
}

/// 自定义元素：按 value 取 top-3 并保留完整结构体字段。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    value: i32,
    name: &'static str,
}

/// 自定义 `Item` 堆按 value 排序后 name/value 对应关系正确。
#[test]
fn bounded_heap_preserves_custom_items() {
    let mut heap = NewBoundedMinHeap(3, |a: &Item, b: &Item| int_comparator(&a.value, &b.value));
    for item in [
        Item {
            value: 10,
            name: "ten",
        },
        Item {
            value: 5,
            name: "five",
        },
        Item {
            value: 15,
            name: "fifteen",
        },
        Item {
            value: 8,
            name: "eight",
        },
        Item {
            value: 12,
            name: "twelve",
        },
    ] {
        heap.Add(item);
    }

    assert_eq!(
        Some(vec![
            Item {
                value: 15,
                name: "fifteen"
            },
            Item {
                value: 12,
                name: "twelve"
            },
            Item {
                value: 10,
                name: "ten"
            },
        ]),
        heap.ToSortedSlice()
    );
}

/// SyncMap：写入、覆盖、删除与 Keys 行为对齐 Go。
#[test]
fn sync_map_matches_go_store_load_delete_and_keys() {
    let map = NewSyncMap::<i64, String>(10);
    map.Store(1, "a".into());
    map.Store(2, "b".into());
    assert_eq!((Some("a".into()), true), map.Load(&1));
    assert_eq!((None, false), map.Load(&3));

    map.Store(1, "c".into());
    assert_eq!((Some("c".into()), true), map.Load(&1));
    assert_eq!((Some("c".into()), true), map.Delete(&1));
    assert_eq!((None, false), map.Delete(&1));

    let mut keys = map.Keys();
    keys.sort();
    assert_eq!(vec![2], keys);
}

/// 多线程并发 Store/Load 后 Keys 数量等于写入键数。
#[test]
fn sync_map_supports_concurrent_access() {
    let map = Arc::new(NewSyncMap::<usize, usize>(32));
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let map = Arc::clone(&map);
            std::thread::spawn(move || {
                for offset in 0..100 {
                    let key = worker * 100 + offset;
                    map.Store(key, key + 1);
                    assert_eq!((Some(key + 1), true), map.Load(&key));
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(400, map.Keys().len());
}

/// 负容量构造 SyncMap 应 panic（对齐 Go）。
#[test]
fn sync_map_negative_capacity_panics_like_go() {
    assert!(std::panic::catch_unwind(|| NewSyncMap::<i32, i32>(-1)).is_err());
}
