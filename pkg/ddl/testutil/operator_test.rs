// Copyright 2026 AsterSQL.

use std::sync::mpsc;
use std::time::Duration;

use crate::operator::{NewOperatorTestSink, NewOperatorTestSource};

#[test]
fn source_uses_go_equivalent_unbuffered_channel() {
    let mut source = NewOperatorTestSource(vec![1, 2, 3]);
    let mut sink = NewOperatorTestSink();
    sink.SetSource(source.DataChannel());

    source.Open().unwrap();
    let (closed_tx, closed_rx) = mpsc::channel();
    let source_thread = std::thread::spawn(move || {
        closed_tx.send(source.Close()).unwrap();
    });

    assert_eq!(
        closed_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "Go's unbuffered channel keeps the source blocked until a sink receives"
    );

    sink.Open().unwrap();
    assert_eq!(
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Ok(())
    );
    source_thread.join().unwrap();
    sink.Close().unwrap();

    assert_eq!(sink.Collect(), vec![1, 2, 3]);
    assert_eq!(sink.String(), "testSink");
}
