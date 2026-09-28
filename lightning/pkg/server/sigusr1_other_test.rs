// Copyright 2026 AsterSQL.

#[path = "sigusr1_other.rs"]
mod sigusr1_other;

#[test]
fn non_unix_handler_accepts_borrowed_non_send_callbacks() {
    use std::cell::Cell;
    use std::rc::Rc;

    let marker = Rc::new(Cell::new(0));
    let observed = &marker;

    sigusr1_other::handleSigUsr1(|| {
        observed.set(observed.get() + 1);
    });

    assert_eq!(marker.get(), 0);
}
