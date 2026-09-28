// Copyright 2026 AsterSQL.

use std::thread;

use crate::interface::Context;

#[test]
fn cancel_func_is_repeatable_concurrent_and_child_scoped() {
    let parent = Context::background();
    let (child, cancel) = Context::with_cancel(&parent);
    let (grandchild, _grandchild_cancel) = Context::with_cancel(&child);

    let first = cancel.clone();
    let second = cancel.clone();
    let first_call = thread::spawn(move || first.call());
    let second_call = thread::spawn(move || second.call());
    first_call.join().expect("first cancel call must not panic");
    second_call
        .join()
        .expect("concurrent cancel call must not panic");

    cancel.call();
    assert!(child.is_done(), "cancel must close the child context");
    assert!(
        grandchild.is_done(),
        "cancel must propagate through the child context tree"
    );
    assert!(
        !parent.is_done(),
        "cancelling a child must not cancel its parent"
    );
}

#[test]
fn parent_cancellation_propagates_to_existing_and_new_children() {
    let parent = Context::background();
    let (existing_child, _cancel) = Context::with_cancel(&parent);

    parent.cancel();
    assert!(existing_child.is_done());

    let (new_child, _cancel) = Context::with_cancel(&parent);
    assert!(
        new_child.is_done(),
        "a child created after parent cancellation must start cancelled"
    );
}
