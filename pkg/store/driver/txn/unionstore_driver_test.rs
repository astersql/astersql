// Copyright 2026 AsterSQL.

use crate::*;

/// AssertNone 必须清除已有断言位，对齐 client-go 的 SetAssertNone。
#[test]
fn assert_none_clears_existing_assertion_flags() {
    let buffer = memBuffer::empty(false);
    let key = b"key".to_vec();
    buffer.Set(key.clone(), b"value".to_vec()).unwrap();

    buffer.UpdateAssertionFlags(key.clone(), AssertionOp::AssertExist);
    assert!(buffer.GetFlags(&key).unwrap().has_assert_exist());

    buffer.UpdateAssertionFlags(key.clone(), AssertionOp::AssertNone);
    let flags = buffer.GetFlags(&key).unwrap();
    assert!(!flags.has_assert_exist());
    assert!(!flags.has_assert_not_exist());
    assert!(!flags.has_assert_unknown());
}

#[test]
fn staging_handles_follow_go_stack_depth_semantics() {
    let buffer = memBuffer::empty(false);
    let first = buffer.Staging();
    assert_eq!(first, 1);
    buffer.Release(first);

    // client-go uses the current staging depth as the handle, so a released
    // outer stage makes the next outer stage reuse handle 1.
    assert_eq!(buffer.Staging(), 1);
}

#[test]
#[should_panic(expected = "cannot release staging buffer")]
fn release_rejects_a_non_top_stage_like_go() {
    let buffer = memBuffer::empty(false);
    let outer = buffer.Staging();
    let _inner = buffer.Staging();
    buffer.Release(outer);
}

#[test]
#[should_panic(expected = "cannot cleanup staging buffer")]
fn cleanup_rejects_a_non_top_stage_like_go() {
    let buffer = memBuffer::empty(false);
    let outer = buffer.Staging();
    let _inner = buffer.Staging();
    buffer.Cleanup(outer);
}
