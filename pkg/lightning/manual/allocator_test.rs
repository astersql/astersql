// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use super::allocator::Allocator;

/// Go's zero value disables accounting, and copying Allocator retains the shared counter pointer.
#[test]
fn allocator_preserves_go_zero_value_and_value_copy_contract() {
    let zero_value = Allocator::default();
    let empty = zero_value.Alloc(0);
    zero_value.Free(empty);
    assert_eq!(zero_value.CheckRefCnt(), Ok(()));

    let ref_cnt = Arc::new(AtomicI64::new(0));
    let allocator = Allocator {
        RefCnt: Some(Arc::clone(&ref_cnt)),
    };
    let copied_allocator = allocator.clone();

    let bytes = allocator.Alloc(4);
    assert_eq!(ref_cnt.load(Ordering::SeqCst), 1);
    copied_allocator.Free(bytes);
    assert_eq!(ref_cnt.load(Ordering::SeqCst), 0);
    assert_eq!(allocator.CheckRefCnt(), Ok(()));
}

/// Go sync/atomic operations are sequentially consistent. A source contract is used here because
/// a relaxed-ordering regression cannot be exposed deterministically by a scheduler-based test.
#[test]
fn allocator_uses_go_sequentially_consistent_atomic_operations() {
    let source = include_str!("allocator.rs");

    assert!(
        !source.contains("Ordering::Relaxed"),
        "Go atomic operations must not be weakened to Relaxed ordering"
    );
    assert_eq!(source.matches("fetch_add(1, Ordering::SeqCst)").count(), 1);
    assert_eq!(source.matches("fetch_sub(1, Ordering::SeqCst)").count(), 1);
    assert_eq!(source.matches("load(Ordering::SeqCst)").count(), 2);
}
