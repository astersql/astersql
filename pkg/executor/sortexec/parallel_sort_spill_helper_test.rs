// Copyright 2026 AsterSQL.

use super::parallel_sort_spill_helper::parallelSortSpillHelper;
use super::parallel_sort_worker::parallelSortWorker;
use super::sort_util::{MemoryTracker, comparator, notSpilled};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[test]
fn empty_spill_resets_status_without_reporting_disk_spill() {
    let memory_tracker = Arc::new(MemoryTracker::new(-1));
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let mut helper = parallelSortSpillHelper::new(
        Vec::new(),
        comparator(Vec::new()),
        memory_tracker,
        disk_tracker,
    );

    helper.spill().unwrap();

    assert_eq!(helper.spillStatus(), notSpilled);
    assert!(!helper.isSpillTriggered());
}

#[test]
fn failed_spill_resets_status_for_a_future_attempt() {
    let memory_tracker = Arc::new(MemoryTracker::new(-1));
    let disk_tracker = Arc::new(MemoryTracker::new(-1));
    let worker = Arc::new(Mutex::new(parallelSortWorker::new(
        comparator(Vec::new()),
        1,
        Arc::new(AtomicBool::new(false)),
        memory_tracker.clone(),
    )));
    let poisoned_worker = worker.clone();
    let _ = std::panic::catch_unwind(move || {
        let _guard = poisoned_worker.lock().unwrap();
        panic!("poison worker lock");
    });
    let mut helper = parallelSortSpillHelper::new(
        vec![worker],
        comparator(Vec::new()),
        memory_tracker,
        disk_tracker,
    );

    assert!(helper.spill().is_err());
    assert_eq!(helper.spillStatus(), notSpilled);
    assert!(helper.setNeedSpill());
}
