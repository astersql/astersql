// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn deadlock_history_retains_ten_complete_events() {
    RUNTIME_DEADLOCK_HISTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    let mut state = RuntimeRowLockState::default();
    state.wait_for.insert(1, 2);
    state.wait_for.insert(2, 1);
    for _ in 0..11 {
        record_runtime_deadlock(&state, 1);
    }

    let history = RUNTIME_DEADLOCK_HISTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let event_count = history
        .iter()
        .map(|record| record.deadlock_id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(event_count, 10);
    assert_eq!(history.len(), 20, "both wait edges must remain per event");
    drop(history);
    RUNTIME_DEADLOCK_HISTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}
