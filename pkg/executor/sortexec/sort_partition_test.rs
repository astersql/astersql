// Copyright 2026 AsterSQL.

use crate::sort_partition::SortPartition;
use crate::sort_util::{MemoryTracker, comparator, spillTriggered};
use std::sync::Arc;

fn partition() -> SortPartition {
    SortPartition::new(
        comparator(Vec::new()),
        Arc::new(MemoryTracker::new(-1)),
        Arc::new(MemoryTracker::new(-1)),
    )
}

#[test]
fn empty_spill_matches_go_error_and_completes_state_transition() {
    let mut partition = partition();

    let err = partition
        .spillToDisk()
        .expect_err("Go rejects spilling an empty partition");

    assert_eq!(err.0, "can not spill empty chunk to disk");
    assert_eq!(partition.spillStatus(), spillTriggered);
}

#[test]
fn spilling_a_closed_partition_is_a_successful_no_op() {
    let mut partition = partition();
    partition.close();

    partition
        .spillToDisk()
        .expect("Go treats spilling a closed partition as a no-op");

    assert_eq!(partition.spillStatus(), spillTriggered);
}
