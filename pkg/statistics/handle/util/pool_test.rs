// Copyright 2026 AsterSQL.

use super::pool::GoroutinePool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

#[test]
fn capacity_limits_idle_workers_not_concurrent_jobs() {
    let pool = GoroutinePool::new(1, Duration::from_secs(1));
    let release = Arc::new(Barrier::new(3));
    let (started_tx, started_rx) = mpsc::channel();

    for _ in 0..2 {
        let release = Arc::clone(&release);
        let started_tx = started_tx.clone();
        pool.submit(move || {
            started_tx.send(()).unwrap();
            release.wait();
        })
        .unwrap();
    }

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    release.wait();
}

#[test]
fn close_is_non_blocking_and_ignores_new_jobs() {
    let pool = GoroutinePool::new(1, Duration::from_secs(1));
    let release = Arc::new(Barrier::new(2));
    let started = Arc::new(Barrier::new(2));
    let ran_after_close = Arc::new(AtomicBool::new(false));

    let job_release = Arc::clone(&release);
    let job_started = Arc::clone(&started);
    pool.submit(move || {
        job_started.wait();
        job_release.wait();
    })
    .unwrap();
    started.wait();

    let before_close = Instant::now();
    pool.close();
    assert!(before_close.elapsed() < Duration::from_millis(250));

    let ran = Arc::clone(&ran_after_close);
    assert!(
        pool.submit(move || ran.store(true, Ordering::Release))
            .is_ok()
    );
    release.wait();
    std::thread::sleep(Duration::from_millis(20));
    assert!(!ran_after_close.load(Ordering::Acquire));
}

#[test]
fn zero_idle_timeout_keeps_worker_for_reuse() {
    let pool = GoroutinePool::new(1, Duration::ZERO);
    let (first_tx, first_rx) = mpsc::channel();
    pool.submit(move || first_tx.send(std::thread::current().id()).unwrap())
        .unwrap();
    let first = first_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    std::thread::sleep(Duration::from_millis(20));
    let (second_tx, second_rx) = mpsc::channel();
    pool.submit(move || second_tx.send(std::thread::current().id()).unwrap())
        .unwrap();
    let second = second_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    assert_eq!(first, second);
    pool.close();
}
