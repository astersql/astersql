// Copyright 2026 AsterSQL.

use crate::test_state::global_state_guard;

#[test]
fn global_state_guard_remains_usable_after_a_panicking_holder() {
    let panicked = std::thread::spawn(|| {
        let _guard = global_state_guard();
        panic!("poison the global test-state mutex");
    })
    .join();
    assert!(panicked.is_err());

    let _guard = global_state_guard();
}
