// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go `bench_test.go` 的可执行工作负载对应测试。
//!
//! Rust stable 没有内置 benchmark harness，因此这里保留相同的数据构造、分段、
//! build 与裸指针写入路径，并用结果断言防止基准退化成不可编译的迁移草稿。

use crate::hash_table_v2::{HashTableV2, get_hash_table_length_by_row_len};
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::join_table_meta::EncodedRow;
use std::sync::atomic::AtomicBool;

const WORKLOAD_ROWS: usize = 10_000;
const BUILD_THREADS: usize = 3;

fn make_segment(start: usize, end: usize) -> RowTableSegment {
    let rows = (start..end)
        .map(|value| EncodedRow {
            bytes: (value as u64).to_le_bytes().to_vec(),
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        })
        .collect();
    RowTableSegment {
        rows,
        hash_values: (start as u64..end as u64).collect(),
        valid_key_count: (end - start) as u64,
        ..Default::default()
    }
}

fn make_row_table(segments: Vec<RowTableSegment>) -> RowTable {
    let mut table = RowTable::default();
    table.segments_mut().extend(segments);
    table
}

#[test]
fn hash_table_build_matches_go_workload() {
    let table = HashTableV2::new(vec![make_row_table(vec![make_segment(0, WORKLOAD_ROWS)])]);

    assert_eq!(table.total_row_count(), WORKLOAD_ROWS as u64);
    assert_eq!(table.lookup(0, 0).len(), 1);
    assert_eq!(table.lookup(0, (WORKLOAD_ROWS - 1) as u64).len(), 1);
    assert_eq!(
        get_hash_table_length_by_row_len(WORKLOAD_ROWS as u64),
        16_384
    );
}

#[test]
fn concurrent_build_partitions_cover_remainder_like_go() {
    let mut workers = Vec::with_capacity(BUILD_THREADS);
    for worker in 0..BUILD_THREADS {
        let start = WORKLOAD_ROWS / BUILD_THREADS * worker;
        let end = if worker == BUILD_THREADS - 1 {
            WORKLOAD_ROWS
        } else {
            WORKLOAD_ROWS / BUILD_THREADS * (worker + 1)
        };
        workers.push(std::thread::spawn(move || make_segment(start, end)));
    }
    let segments = workers
        .into_iter()
        .map(|worker| worker.join().expect("build worker must not panic"))
        .collect();
    let table = HashTableV2::new(vec![make_row_table(segments)]);

    assert_eq!(table.total_row_count(), WORKLOAD_ROWS as u64);
    for hash in [
        0,
        (WORKLOAD_ROWS / BUILD_THREADS) as u64,
        (WORKLOAD_ROWS - 1) as u64,
    ] {
        assert_eq!(table.lookup(0, hash).len(), 1);
    }
}

#[test]
fn unsafe_pointer_writes_match_go_workload() {
    let size = 1_000;
    let mut bytes = vec![0_u8; size * size_of::<i64>()];
    let pointers: Vec<*mut i64> = bytes
        .chunks_exact_mut(size_of::<i64>())
        .map(|chunk| chunk.as_mut_ptr().cast::<i64>())
        .collect();

    for (index, pointer) in pointers.iter().copied().enumerate() {
        // SAFETY: each pointer addresses a distinct in-bounds 8-byte chunk; `write_unaligned`
        // handles the byte buffer's alignment, and `bytes` outlives every pointer.
        unsafe { pointer.write_unaligned(index as i64) };
    }

    for (index, chunk) in bytes.chunks_exact(size_of::<i64>()).enumerate() {
        assert_eq!(i64::from_ne_bytes(chunk.try_into().unwrap()), index as i64);
    }
}

#[test]
fn uintptr_round_trip_writes_match_go_workload() {
    let size = 1_000;
    let mut bytes = vec![0_u8; size * size_of::<i64>()];
    let addresses: Vec<usize> = bytes
        .chunks_exact_mut(size_of::<i64>())
        .map(|chunk| chunk.as_mut_ptr() as usize)
        .collect();

    for (index, address) in addresses.iter().copied().enumerate() {
        // SAFETY: addresses were obtained from distinct live chunks of `bytes`; unaligned
        // writes preserve the Go benchmark's uintptr round-trip without assuming alignment.
        unsafe { (address as *mut i64).write_unaligned(index as i64) };
    }

    for (index, chunk) in bytes.chunks_exact(size_of::<i64>()).enumerate() {
        assert_eq!(i64::from_ne_bytes(chunk.try_into().unwrap()), index as i64);
    }
}
