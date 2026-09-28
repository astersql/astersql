// Copyright 2026 AsterSQL.

use std::thread;
use std::time::Duration;

use crate::ticker::dispatcherTicker;

#[test]
fn test_dispatcher_ticker_drops_ticks_when_receiver_is_slow() {
    let mut ticker = dispatcherTicker(Duration::from_millis(5));
    let rx = ticker
        .Ch()
        .expect("positive duration has a channel")
        .clone();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("ticker should produce an initial tick");

    thread::sleep(Duration::from_millis(50));

    let queued = rx.len();
    ticker.Stop();
    assert!(
        queued <= 1,
        "Go time.Ticker keeps at most one pending tick, got {queued}"
    );
}
