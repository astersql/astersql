// Copyright 2026 AsterSQL.

use crate::hash_join_spill::{
    DEFAULT_SPILL_PRIORITY, HashJoinSpillAction, OomAction, has_enough_data_to_spill,
};
use crate::hash_join_spill_helper::{HashJoinSpillHelper, MemoryTracker, SpillStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

#[derive(Default)]
struct CountingFallback(AtomicUsize);

impl OomAction for CountingFallback {
    fn priority(&self) -> i64 {
        0
    }

    fn action(&self, _tracker: &MemoryTracker) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn spill_action_priority_matches_go_memory_default() {
    let helper = Arc::new(HashJoinSpillHelper::new(1, 1, 1, 100).unwrap());
    let action = HashJoinSpillAction::new(helper);

    // Go returns memory.DefSpillPriority, whose matching Rust constant is 2.
    assert_eq!(DEFAULT_SPILL_PRIORITY, 2);
    assert_eq!(action.priority(), 2);
}

#[test]
fn spill_threshold_and_fallback_branches_match_go() {
    let helper = Arc::new(HashJoinSpillHelper::new(1, 1, 1, 100).unwrap());
    let tracker = MemoryTracker::new(100);
    tracker.consume(101);
    let fallback = Arc::new(CountingFallback::default());
    let action = HashJoinSpillAction::new(helper.clone()).with_fallback(fallback.clone());

    helper.memory_tracker.consume(4);
    assert!(!has_enough_data_to_spill(&helper.memory_tracker, &tracker));
    action.action(&tracker);
    assert_eq!(fallback.0.load(Ordering::SeqCst), 1);
    assert_eq!(helper.status(), SpillStatus::NotSpilled);

    helper.memory_tracker.consume(1);
    helper.set_can_spill_flag(true);
    assert!(has_enough_data_to_spill(&helper.memory_tracker, &tracker));
    action.action(&tracker);
    assert_eq!(helper.status(), SpillStatus::NeedSpill);
    assert_eq!(fallback.0.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_actions_set_need_spill_only_once_like_go_cond_lock() {
    let helper = Arc::new(HashJoinSpillHelper::new(1, 1, 1, 100).unwrap());
    helper.memory_tracker.consume(5);
    helper.set_can_spill_flag(true);
    let tracker = Arc::new(MemoryTracker::new(100));
    tracker.consume(101);
    let action = Arc::new(HashJoinSpillAction::new(helper.clone()));
    let barrier = Arc::new(Barrier::new(16));

    let workers: Vec<_> = (0..16)
        .map(|_| {
            let action = action.clone();
            let tracker = tracker.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                usize::from(action.action_impl(&tracker))
            })
        })
        .collect();

    let successes: usize = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .sum();
    assert_eq!(successes, 1);
    assert_eq!(helper.status(), SpillStatus::NeedSpill);
}
