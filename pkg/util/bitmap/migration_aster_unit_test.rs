// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `bitmap` 迁移期单元测试：对齐 Go ConcurrentBitmap 的构造与操作语义。
//
// 覆盖 segment 向上取整与越界忽略、跨段置位唯一 setter、并发竞争胜者、
// Clone 独立性 / Reset 复用或扩容，以及 `BytesConsumed` 内存估算。

use super::concurrent::{NewConcurrentBitmap, bytesConcurrentBitmap};
use std::mem;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::thread;

/// 验证构造按 32-bit 向上取整，且越界 Set/UnsafeIsSet 被忽略。
#[test]
fn migration_constructor_rounds_segments_and_ignores_out_of_range_bits() {
    let bitmap = NewConcurrentBitmap(33);

    assert_eq!(bitmap.bitLen, 33);
    assert_eq!(bitmap.segments.len(), 2);
    assert!(!bitmap.Set(-1));
    assert!(!bitmap.Set(33));
    assert!(!bitmap.UnsafeIsSet(-1));
    assert!(!bitmap.UnsafeIsSet(33));
    assert!(
        bitmap
            .segments
            .iter()
            .all(|segment| segment.load(Ordering::Relaxed) == 0)
    );
}

/// 验证跨 segment 边界的置位：首次为唯一 setter，二次返回 false。
#[test]
fn migration_set_maps_bits_across_segments_and_has_one_setter() {
    let bitmap = NewConcurrentBitmap(65);

    for bit in [0, 31, 32, 63, 64] {
        assert!(bitmap.Set(bit), "first Set({bit}) must be the setter");
        assert!(!bitmap.Set(bit), "second Set({bit}) must not be the setter");
        assert!(bitmap.UnsafeIsSet(bit));
    }
    for bit in [1, 30, 33, 62] {
        assert!(!bitmap.UnsafeIsSet(bit));
    }
}

/// 验证多线程同时 Set 同一 bit 时恰好一个胜者（返回 true）。
#[test]
fn migration_concurrent_set_reports_exactly_one_winner() {
    const COMPETITORS: usize = 64;
    let bitmap = Arc::new(NewConcurrentBitmap(32));
    let barrier = Arc::new(Barrier::new(COMPETITORS));

    let workers: Vec<_> = (0..COMPETITORS)
        .map(|_| {
            let bitmap = Arc::clone(&bitmap);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                bitmap.Set(31)
            })
        })
        .collect();

    let winners = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker must not panic"))
        .filter(|is_setter| *is_setter)
        .count();
    assert_eq!(winners, 1);
    assert!(bitmap.UnsafeIsSet(31));
}

/// 验证 Clone 独立副本；Reset 短长度复用 segment、长长度扩容并清零。
#[test]
fn migration_clone_is_independent_and_reset_reuses_or_grows_storage() {
    let mut bitmap = NewConcurrentBitmap(65);
    bitmap.UnsafeSet(1);
    bitmap.UnsafeSet(64);

    let clone = bitmap.clone();
    assert!(clone.UnsafeIsSet(1));
    assert!(clone.UnsafeIsSet(64));

    // 缩短 bitLen：清零复用，不缩减 segments 容量。
    bitmap.Reset(8);
    assert_eq!(bitmap.bitLen, 8);
    assert_eq!(bitmap.segments.len(), 3);
    assert!(!bitmap.UnsafeIsSet(1));
    assert!(clone.UnsafeIsSet(1));
    assert!(clone.UnsafeIsSet(64));

    // 扩大到超出当前 segments：重新分配并清零。
    bitmap.Reset(129);
    assert_eq!(bitmap.bitLen, 129);
    assert_eq!(bitmap.segments.len(), 5);
    assert!(
        bitmap
            .segments
            .iter()
            .all(|segment| segment.load(Ordering::Relaxed) == 0)
    );
}

/// 验证 `BytesConsumed` = 结构体大小 + segments capacity × u32。
#[test]
fn migration_bytes_consumed_tracks_allocated_segment_capacity() {
    let bitmap = NewConcurrentBitmap(65);
    let expected = mem::size_of_val(&bitmap) as i64
        + (mem::size_of::<u32>() * bitmap.segments.capacity()) as i64;

    assert_eq!(bytesConcurrentBitmap, mem::size_of_val(&bitmap) as i64);
    assert_eq!(bitmap.BytesConsumed(), expected);
}
