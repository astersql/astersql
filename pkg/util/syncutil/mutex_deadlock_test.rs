// Copyright 2026 AsterSQL.

use super::mutex_deadlock;

#[test]
fn constructing_a_deadlock_mutex_starts_detection() {
    let _mutex = mutex_deadlock::Mutex::new(());
    assert!(mutex_deadlock::detector_started());

    mutex_deadlock::init();
    assert!(mutex_deadlock::detector_started());
}
