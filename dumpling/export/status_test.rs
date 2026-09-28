// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `status_test.go` 的 Rust 单测：验证 `GetStatus` 指标聚合与 `SpeedRecorder` 速率计算。

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::main_test::default_config_for_test;
use crate::*;

/// 验证零初始状态下 GetStatus 各 counter/gauge 为 0，写入 mock 指标后读回一致。
#[test]
fn test_get_parameters() {
    let conf = default_config_for_test();
    let factory = conf.PromFactory.clone();
    let labels = conf.Labels.clone();
    // 构造最小 Dumper，仅挂载 metrics 与 speedRecorder，无需真实 DB。
    let d = Dumper {
        tctx: tcontext::Background(),
        conf: std::sync::Arc::new(conf),
        db: None,
        ext_storage: None,
        metrics: newMetrics(factory.as_ref(), &labels),
        speedRecorder: std::sync::Mutex::new(NewSpeedRecorder()),
        totalTables: std::sync::atomic::AtomicI64::new(0),
        cancel: None,
        http: None,
        pd_client: None,
    };

    let mid = d.GetStatus();
    // 初始 counter/gauge 均为 0。
    assert_eq!(mid.CompletedTables, 0.0);
    assert_eq!(mid.FinishedBytes, 0.0);
    assert_eq!(mid.FinishedRows, 0.0);
    assert_eq!(mid.EstimateTotalRows, 0.0);

    // 模拟 worker 上报进度，与 Go 测试相同的 AddCounter/AddGauge 序列。
    AddCounter(Some(&d.metrics.finishedTablesCounter), 10.0);
    AddGauge(Some(&d.metrics.finishedSizeGauge), 20.0);
    AddGauge(Some(&d.metrics.finishedRowsGauge), 30.0);
    AddCounter(Some(&d.metrics.estimateTotalRowsCounter), 40.0);

    let mid = d.GetStatus();
    // GetStatus 应原样反映 Prometheus 指标写入值。
    assert_eq!(mid.CompletedTables, 10.0);
    assert_eq!(mid.FinishedBytes, 20.0);
    assert_eq!(mid.FinishedRows, 30.0);
    assert_eq!(mid.EstimateTotalRows, 40.0);
}

/// 验证 SpeedRecorder 在 sleep 间隔内 bytes/s 与 Go 用例一致（允许 10% 相对误差）。
#[test]
fn test_speed_recorder() {
    let cases = [(1i64, 100.0, 100.0), (2, 200.0, 50.0), (3, 200.0, 50.0)];
    // Go 用例：100B/1s=100，再 100B/2s=50，finished 不变则保持 50。
    let mut speed_recorder = NewSpeedRecorder();
    for (spent, finished, expected) in cases {
        // 真实 sleep 测速率，允许 10% 相对误差。
        thread::sleep(Duration::from_secs(spent as u64));
        let recent = speed_recorder.GetSpeed(finished);
        let rel = (expected - recent).abs() / expected;
        // 与 Go assert.InDelta 等价容差。
        assert!(
            rel <= 0.1,
            "speed unexpected expected={expected:.2} recent={recent:.2}"
        );
    }
}

/// Go `runLogProgress` stays alive between ticks and exits only after context cancellation.
#[test]
fn test_run_log_progress_waits_until_cancelled() {
    let conf = default_config_for_test();
    let factory = conf.PromFactory.clone();
    let labels = conf.Labels.clone();
    let d = std::sync::Arc::new(Dumper {
        tctx: tcontext::Background(),
        conf: std::sync::Arc::new(conf),
        db: None,
        ext_storage: None,
        metrics: newMetrics(factory.as_ref(), &labels),
        speedRecorder: std::sync::Mutex::new(NewSpeedRecorder()),
        totalTables: std::sync::atomic::AtomicI64::new(0),
        cancel: None,
        http: None,
        pd_client: None,
    });
    let (ctx, cancel) = tcontext::Background().WithCancel();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        d.runLogProgress(&ctx);
        done_tx.send(()).unwrap();
    });

    started_rx.recv().unwrap();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    cancel.call();
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("runLogProgress should observe context cancellation");
    worker.join().unwrap();
}
