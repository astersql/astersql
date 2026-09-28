// Copyright 2026 AsterSQL.

use crate::RegisteredPlannerCallbacks;

#[test]
fn registered_planner_callbacks_cover_every_go_init_side_effect() {
    let callbacks = RegisteredPlannerCallbacks();

    assert_eq!(callbacks.len(), 111);
    assert!(callbacks.contains("DefaultDisabledLogicalRulesList"));
}

#[test]
fn planner_callback_registration_is_idempotent() {
    let first = RegisteredPlannerCallbacks();
    let second = RegisteredPlannerCallbacks();

    assert_eq!(first, second);
}
