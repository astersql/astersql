// Copyright 2026 AsterSQL.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::worker::{BaseWorker, Worker};

#[test]
fn send_waits_until_the_worker_receives_like_go_unbuffered_channel() {
    let (started_tx, started_rx) = mpsc::channel();
    let (receive_tx, receive_rx) = mpsc::channel();
    let worker = BaseWorker::new(move |_cancellation, messages| {
        started_tx.send(()).unwrap();
        receive_rx.recv().unwrap();
        messages.recv().unwrap();
        Ok(())
    });

    worker.start();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let sender = worker.clone();
    let (sent_tx, sent_rx) = mpsc::channel();
    let send_thread = thread::spawn(move || {
        sender.send(Box::new(42_u64)).unwrap();
        sent_tx.send(()).unwrap();
    });

    assert_eq!(
        sent_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    receive_tx.send(()).unwrap();
    sent_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    send_thread.join().unwrap();
    worker
        .wait_stopped(&Default::default(), Duration::from_secs(1))
        .unwrap();
}
