// Copyright 2026 AsterSQL.

use super::parallel_sort_worker::parallelSortWorker;
use super::sort_util::{DataChunk, MemoryTracker, Row, RowComparator, SortValue};
use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

#[test]
fn cancellation_raised_during_comparison_interrupts_sort() {
    let killed = Arc::new(AtomicBool::new(false));
    let kill_from_compare = killed.clone();
    let compare: RowComparator = Arc::new(move |lhs, rhs| {
        kill_from_compare.store(true, AtomicOrdering::Release);
        match (&lhs.0[0], &rhs.0[0]) {
            (SortValue::Int(lhs), SortValue::Int(rhs)) => lhs.cmp(rhs),
            _ => Ordering::Equal,
        }
    });
    let mem = Arc::new(MemoryTracker::new(-1));
    let mut worker = parallelSortWorker::new(compare, 30_000, killed, mem);
    let rows = (0..30_000)
        .rev()
        .map(|value| Row(vec![SortValue::Int(value)]))
        .collect();
    worker.saveChunk(DataChunk::new(rows)).unwrap();

    let error = worker.sortBatch().unwrap_err();

    assert_eq!(error.0, "query interrupted");
}
