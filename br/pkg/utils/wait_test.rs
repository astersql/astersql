// Copyright 2026 AsterSQL.

use std::thread;
use std::time::{Duration, Instant};

use crate::WaitUntil;
use crate::stubs::context::Context;

#[test]
fn slow_condition_does_not_cause_burst_checks_for_dropped_ticks() {
    let ctx = Context::new();
    let mut checks = Vec::new();

    WaitUntil(
        &ctx,
        || {
            checks.push(Instant::now());
            match checks.len() {
                2 => {
                    // Go's time.Ticker buffers at most one tick while the receiver is busy.
                    thread::sleep(Duration::from_millis(250));
                    false
                }
                4 => true,
                _ => false,
            }
        },
        Duration::from_millis(100),
        Duration::from_secs(2),
    )
    .unwrap();

    assert!(
        checks[3].duration_since(checks[2]) >= Duration::from_millis(25),
        "dropped ticker events must not be replayed as a burst"
    );
}
