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

use super::sort::VecRowSource;
use super::sort_partition::SortPartition;
use super::sort_util::{DataChunk, MemoryTracker, Row, RowComparator, SortValue};
use super::{Limit, RankInfo, SortExec, SortKey, TopNExec};
use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

/// 空 ByItems 时 Open 应直接报错，且不消费数据源。
#[test]
fn sort_rejects_missing_ordering_items_before_reading_source() {
    let mut executor = SortExec::new(
        Box::new(VecRowSource::new(Vec::new())),
        Vec::new(),
        1,
        8,
        -1,
    );
    assert_eq!(
        executor.Open().unwrap_err().to_string(),
        "sort requires at least one ordering item"
    );
}

#[test]
fn serial_spill_releases_memory_and_disk_trackers_on_close() {
    let source = VecRowSource::new(vec![
        DataChunk::new(vec![
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(1)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(2)]),
            Row(vec![SortValue::Int(4)]),
        ]),
    ]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 1, 2, 180);
    let first = executor.Next(8).unwrap();
    assert_eq!(first.num_rows(), 2);
    assert!(executor.IsSpillTriggered());
    while executor.Next(8).unwrap().num_rows() != 0 {}
    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
    assert_eq!(executor.GetDiskTracker().bytes_consumed(), 0);
}

#[test]
fn parallel_sort_releases_worker_memory_on_close() {
    let source = VecRowSource::new(vec![
        DataChunk::new(vec![
            Row(vec![SortValue::Int(4)]),
            Row(vec![SortValue::Int(1)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(2)]),
        ]),
    ]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 2, 8, -1);
    assert_eq!(executor.Next(8).unwrap().num_rows(), 4);
    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
}

#[test]
fn rank_topn_count_zero_returns_without_underflow() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(1)]),
        Row(vec![SortValue::Int(2)]),
    ])]);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 0,
            Count: 0,
        },
        Some(RankInfo {
            prefixKeys: vec![SortKey::asc(0)],
            expectedCount: 0,
        }),
        1,
        8,
        -1,
    );
    assert!(executor.Next(8).unwrap().is_empty());
}

#[test]
fn kill_interrupts_sort_before_result_is_materialized() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(1)]),
        Row(vec![SortValue::Int(2)]),
    ])]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 1, 8, -1);
    executor.Kill();
    assert_eq!(
        executor.Next(8).unwrap_err().to_string(),
        "query interrupted"
    );
}

#[test]
fn parallel_sort_spill_can_repeat_and_still_merge_in_order() {
    let source = VecRowSource::new(vec![
        DataChunk::new(vec![
            Row(vec![SortValue::Int(9)]),
            Row(vec![SortValue::Int(1)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(8)]),
            Row(vec![SortValue::Int(2)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(7)]),
            Row(vec![SortValue::Int(3)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(6)]),
            Row(vec![SortValue::Int(4)]),
        ]),
    ]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 2, 2, 180);
    let mut values = Vec::new();
    loop {
        let chunk = executor.Next(2).unwrap();
        let empty = chunk.is_empty();
        values.extend(chunk.rows.into_iter().map(|row| row.0[0].clone()));
        if empty {
            break;
        }
    }
    assert!(executor.IsSpillTriggered());
    assert_eq!(
        values,
        [1, 2, 3, 4, 6, 7, 8, 9]
            .into_iter()
            .map(SortValue::Int)
            .collect::<Vec<_>>()
    );
    executor.Close().unwrap();
}

#[test]
fn parallel_sort_kill_is_reported_by_worker() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(1)]),
        Row(vec![SortValue::Int(2)]),
    ])]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 2, 8, -1);
    executor.Kill();
    assert_eq!(
        executor.Next(8).unwrap_err().to_string(),
        "query interrupted"
    );
}

#[test]
fn topn_applies_offset_count_and_projection_after_sorting() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(3), SortValue::Bytes(b"c".to_vec())]),
        Row(vec![SortValue::Int(1), SortValue::Bytes(b"a".to_vec())]),
        Row(vec![SortValue::Int(2), SortValue::Bytes(b"b".to_vec())]),
    ])]);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 1,
            Count: 1,
        },
        None,
        2,
        8,
        -1,
    );
    executor.SetColumnIdxsUsedByChild(vec![1]);
    let output = executor.Next(8).unwrap();
    assert_eq!(
        output.rows,
        vec![Row(vec![SortValue::Bytes(b"b".to_vec())])]
    );
}

#[test]
fn topn_spill_releases_trackers_on_close() {
    let source = VecRowSource::new(vec![
        DataChunk::new(vec![
            Row(vec![SortValue::Int(4)]),
            Row(vec![SortValue::Int(1)]),
        ]),
        DataChunk::new(vec![
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(2)]),
        ]),
    ]);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 0,
            Count: 2,
        },
        None,
        1,
        8,
        50,
    );
    assert_eq!(executor.Next(8).unwrap().num_rows(), 2);
    assert!(executor.IsSpillTriggered());
    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
    assert_eq!(executor.GetDiskTracker().bytes_consumed(), 0);
}

#[test]
fn topn_empty_input_returns_empty_chunk() {
    let mut executor = TopNExec::new(
        Box::new(VecRowSource::new(Vec::new())),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 0,
            Count: 4,
        },
        None,
        1,
        8,
        -1,
    );
    assert!(executor.Next(8).unwrap().is_empty());
}

#[test]
fn serial_sort_empty_input_returns_empty_chunk() {
    let mut executor = SortExec::new(
        Box::new(VecRowSource::new(Vec::new())),
        vec![SortKey::asc(0)],
        1,
        8,
        -1,
    );
    assert!(executor.Next(8).unwrap().is_empty());
    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
}

#[test]
fn close_after_topn_spill_is_idempotent() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(2)]),
        Row(vec![SortValue::Int(1)]),
    ])]);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 0,
            Count: 1,
        },
        None,
        1,
        8,
        50,
    );
    let _ = executor.Next(8).unwrap();
    executor.Close().unwrap();
    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
}

#[test]
fn topn_count_zero_does_not_read_or_spill_input() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(1)]),
        Row(vec![SortValue::Int(2)]),
    ])]);
    let mut executor = TopNExec::new(
        Box::new(source),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 3,
            Count: 0,
        },
        None,
        1,
        8,
        1,
    );
    assert!(executor.Next(8).unwrap().is_empty());
    assert!(!executor.IsSpillTriggered());
}

fn interrupted_partition() -> (SortPartition, Arc<AtomicBool>) {
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupt_from_compare = interrupted.clone();
    let compare: RowComparator = Arc::new(move |lhs, rhs| {
        interrupt_from_compare.store(true, AtomicOrdering::Release);
        match (&lhs.0[0], &rhs.0[0]) {
            (SortValue::Int(lhs), SortValue::Int(rhs)) => lhs.cmp(rhs),
            _ => Ordering::Equal,
        }
    });
    let memory = Arc::new(MemoryTracker::new(-1));
    let disk = Arc::new(MemoryTracker::new(-1));
    (
        SortPartition::newWithKiller(compare, interrupted.clone(), memory, disk),
        interrupted,
    )
}

fn descending_rows() -> DataChunk {
    DataChunk::new(
        (0..30_000)
            .rev()
            .map(|value| Row(vec![SortValue::Int(value)]))
            .collect(),
    )
}

/// Mirrors Go TestInterruptedDuringSort with deterministic cancellation from
/// the comparison boundary instead of a timing-dependent sleep.
#[test]
fn interrupted_during_sort_returns_query_interrupted() {
    let (mut partition, interrupted) = interrupted_partition();
    assert!(partition.add(descending_rows()));

    let error = partition.sort().unwrap_err();

    assert!(interrupted.load(AtomicOrdering::Acquire));
    assert_eq!(error.0, "query interrupted");
    partition.close();
}

/// Mirrors Go TestInterruptedDuringSpilling: spill must propagate a kill that
/// arrives while its mandatory sort phase is running and still be closeable.
#[test]
fn interrupted_during_spilling_returns_query_interrupted() {
    let (mut partition, interrupted) = interrupted_partition();
    assert!(partition.add(descending_rows()));

    let error = partition.spillToDisk().unwrap_err();

    assert!(interrupted.load(AtomicOrdering::Acquire));
    assert_eq!(error.0, "query interrupted");
    partition.close();
}
