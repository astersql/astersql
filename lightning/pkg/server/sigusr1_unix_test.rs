// Copyright 2026 AsterSQL.

use crate::handleSigUsr1;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn sigusr1_notifies_every_registered_handler() {
    let (tx, rx) = mpsc::channel();
    let first = tx.clone();
    handleSigUsr1(move || {
        first.send("first").unwrap();
    });
    handleSigUsr1(move || {
        tx.send("second").unwrap();
    });

    unsafe {
        libc::raise(libc::SIGUSR1);
    }

    let mut notified = [
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
    ];
    notified.sort_unstable();
    assert_eq!(notified, ["first", "second"]);
}
