// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::sort::VecRowSource;
use super::sort_util::{DataChunk, MemoryTracker, Row, SortKey, SortValue, comparator};
use super::topn_chunk_heap::topNChunkHeap;
use super::topn_spill::{topNSpillAction, topNSpillHelper};
use super::topn_worker::topNWorker;
use super::{Limit, TopNExec};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn worker_with_rows(values: &[i64], tracker: Arc<MemoryTracker>) -> Arc<Mutex<topNWorker>> {
    let compare = comparator(vec![SortKey::asc(0)]);
    let mut worker = topNWorker::new(
        topNChunkHeap::new(values.len(), compare),
        Arc::new(AtomicBool::new(false)),
        tracker,
    );
    worker
        .run(DataChunk::new(
            values
                .iter()
                .map(|value| Row(vec![SortValue::Int(*value)]))
                .collect(),
        ))
        .unwrap();
    Arc::new(Mutex::new(worker))
}

fn spill_fixture(
    values: &[i64],
    limit: i64,
) -> (
    Arc<Mutex<topNSpillHelper>>,
    Arc<MemoryTracker>,
    Arc<MemoryTracker>,
) {
    let memory_tracker = Arc::new(MemoryTracker::new(limit));
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let worker = worker_with_rows(values, memory_tracker.clone());
    let helper = Arc::new(Mutex::new(topNSpillHelper::new(
        vec![worker],
        comparator(vec![SortKey::asc(0)]),
        memory_tracker.clone(),
        disk_tracker.clone(),
    )));
    (helper, memory_tracker, disk_tracker)
}

#[test]
fn oom_action_ignores_usage_below_the_tracker_limit() {
    let (helper, tracker, _) = spill_fixture(&[3, 1, 2], 1_000);
    tracker.consume(200);

    topNSpillAction::new(helper.clone(), tracker)
        .Action()
        .unwrap();

    assert!(!helper.lock().unwrap().isSpillNeeded());
    assert!(!helper.lock().unwrap().isSpillTriggered());
}

#[test]
fn oom_action_only_requests_spill_until_the_executor_runs_it() {
    let (helper, tracker, _) = spill_fixture(&[3, 1, 2], 1);

    topNSpillAction::new(helper.clone(), tracker)
        .Action()
        .unwrap();

    assert!(helper.lock().unwrap().isSpillNeeded());
    assert!(!helper.lock().unwrap().isSpillTriggered());
    helper.lock().unwrap().spill().unwrap();
    assert!(helper.lock().unwrap().isSpillTriggered());
}

#[test]
fn empty_spill_does_not_report_a_disk_spill() {
    let memory_tracker = Arc::new(MemoryTracker::new(-1));
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let mut helper = topNSpillHelper::new(
        Vec::new(),
        comparator(Vec::new()),
        memory_tracker,
        disk_tracker,
    );

    helper.spill().unwrap();

    assert!(!helper.isSpillTriggered());
    assert!(helper.setNeedSpill());
}

#[test]
fn failed_spill_resets_status_for_a_future_attempt() {
    let memory_tracker = Arc::new(MemoryTracker::new(-1));
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let worker = worker_with_rows(&[1], memory_tracker.clone());
    let poisoned_worker = worker.clone();
    let _ = std::panic::catch_unwind(move || {
        let _guard = poisoned_worker.lock().unwrap();
        panic!("poison worker lock");
    });
    let mut helper = topNSpillHelper::new(
        vec![worker],
        comparator(vec![SortKey::asc(0)]),
        memory_tracker,
        disk_tracker,
    );

    assert!(helper.spill().is_err());
    assert!(helper.setNeedSpill());
}

/// 小内存限额下触发 spill 后，Next 仍只返回 Offset/Count 窗口内的行。
#[test]
fn topn_spill_returns_only_requested_offset_window() {
    let rows = (0..50)
        .rev()
        .map(|value| Row(vec![SortValue::Int(value)]))
        .collect();
    let mut executor = TopNExec::new(
        Box::new(VecRowSource::new(vec![DataChunk::new(rows)])),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 5,
            Count: 3,
        },
        None,
        2,
        8,
        128,
    );

    let output = executor.Next(8).unwrap();

    assert_eq!(
        output
            .rows
            .iter()
            .map(|row| row.0[0].clone())
            .collect::<Vec<_>>(),
        vec![SortValue::Int(5), SortValue::Int(6), SortValue::Int(7)]
    );
    assert!(executor.IsSpillTriggered());
}
