// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

#[test]
fn backup_filter_cleanup_runs_during_unwind() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let cleanup_events = Arc::clone(&events);

    let unwind = std::panic::catch_unwind(move || {
        let _cleanup = super::backup::scopeguard_restore_filter(Box::new(move || {
            cleanup_events.lock().unwrap().push("filter-restored");
        }));
        panic!("simulated backup panic");
    });

    assert!(unwind.is_err());
    assert_eq!(*events.lock().unwrap(), vec!["filter-restored"]);
}
