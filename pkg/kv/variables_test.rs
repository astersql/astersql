// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicU32, Ordering};

use super::{DefBackOffWeight, DefBackoffLockFast, NewVariables};

#[test]
fn new_variables_matches_client_go_defaults_and_kill_reasons() {
    let killed = AtomicU32::new(0);
    let variables = NewVariables(&killed);

    assert_eq!(variables.BackoffLockFast, DefBackoffLockFast);
    assert_eq!(variables.BackOffWeight, DefBackOffWeight);
    assert!(!variables.IsKilled());

    for reason in [1, 2, u32::MAX] {
        killed.store(reason, Ordering::SeqCst);
        assert!(variables.IsKilled(), "kill reason {reason} must stop work");
    }

    killed.store(0, Ordering::SeqCst);
    assert!(!variables.IsKilled());
}
