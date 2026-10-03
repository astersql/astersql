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
        metrics: std::sync::Arc::new(newMetrics(factory.as_ref(), &labels)),
        speedRecorder: std::sync::Arc::new(std::sync::Mutex::new(NewSpeedRecorder())),
        status: std::sync::Arc::new(std::sync::Mutex::new(DumpStatus::default())),
        totalTables: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
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

    assert_eq!(d.GetStatus(), mid);
    d.RefreshStatus();
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
        metrics: std::sync::Arc::new(newMetrics(factory.as_ref(), &labels)),
        speedRecorder: std::sync::Arc::new(std::sync::Mutex::new(NewSpeedRecorder())),
        status: std::sync::Arc::new(std::sync::Mutex::new(DumpStatus::default())),
        totalTables: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
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

#[test]
fn status_reads_do_not_sample_live_metrics() {
    let conf = default_config_for_test();
    let d = Dumper {
        tctx: tcontext::Background(),
        metrics: std::sync::Arc::new(newMetrics(conf.PromFactory.as_ref(), &conf.Labels)),
        conf: std::sync::Arc::new(conf),
        db: None,
        ext_storage: None,
        speedRecorder: std::sync::Arc::new(std::sync::Mutex::new(NewSpeedRecorder())),
        status: std::sync::Arc::new(std::sync::Mutex::new(DumpStatus::default())),
        totalTables: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        cancel: None,
        http: None,
        pd_client: None,
    };
    AddGauge(Some(&d.metrics.finishedSizeGauge), 4096.0);
    assert_eq!(d.GetStatus().FinishedBytes, 0.0);
}

fn status_dumper() -> Dumper {
    let conf = default_config_for_test();
    Dumper {
        tctx: tcontext::Background(),
        metrics: Arc::new(newMetrics(conf.PromFactory.as_ref(), &conf.Labels)),
        conf: Arc::new(conf),
        db: None,
        ext_storage: None,
        speedRecorder: Arc::new(Mutex::new(NewSpeedRecorder())),
        status: Arc::new(Mutex::new(DumpStatus::default())),
        totalTables: Arc::new(AtomicI64::new(0)),
        cancel: None,
        http: None,
        pd_client: None,
    }
}

#[test]
fn status_snapshots_are_independent_and_progress_is_optional_clamped() {
    let d = status_dumper();
    assert_eq!(d.GetStatus(), DumpStatus::default());
    AddGauge(Some(&d.metrics.finishedSizeGauge), 500.0);
    d.totalTables.store(7, Ordering::SeqCst);
    d.RefreshStatus();
    assert_eq!(d.GetStatus().ProgressPercent, None);
    assert_eq!(d.GetStatus().TotalTables, 7);
    d.metrics.progressReady.store(true, Ordering::SeqCst);
    for (total, completed, expected, text) in [
        (4, 1, 25.0, "25.00 %"),
        (4, 4, 100.0, "100 %"),
        (4, 5, 100.0, "100 %"),
        (0, 0, 100.0, "100 %"),
    ] {
        d.metrics.totalChunks.store(total, Ordering::SeqCst);
        d.metrics.completedChunks.store(completed, Ordering::SeqCst);
        d.RefreshStatus();
        let mut first = d.GetStatus();
        let second = d.GetStatus();
        assert_eq!(first.ProgressPercent, Some(expected));
        assert_eq!(first.Progress, text);
        first.FinishedBytes = 999.0;
        first.ProgressPercent = Some(0.0);
        assert_eq!(second, d.GetStatus());
        assert_eq!(second.FinishedBytes, 500.0);
    }
}

#[test]
fn stop_log_progress_waits_for_final_snapshot_and_is_repeatable() {
    let d = status_dumper();
    let mut guard = d.startLogProgress(&d.tctx);
    AddGauge(Some(&d.metrics.finishedSizeGauge), 500.0);
    d.metrics.totalChunks.store(4, Ordering::SeqCst);
    d.metrics.completedChunks.store(4, Ordering::SeqCst);
    d.metrics.progressReady.store(true, Ordering::SeqCst);
    guard.stop();
    let final_status = d.GetStatus();
    assert_eq!(final_status.FinishedBytes, 500.0);
    assert_eq!(final_status.ProgressPercent, Some(100.0));
    AddGauge(Some(&d.metrics.finishedSizeGauge), 500.0);
    guard.stop();
    assert_eq!(d.GetStatus(), final_status);
}

#[test]
fn periodic_refresh_and_log_average_use_separate_samples() {
    let d = Arc::new(status_dumper());
    let (ctx, cancel) = tcontext::Background().WithCancel();
    let view = d.clone();
    let worker = thread::spawn(move || {
        view.runLogProgressWithTicks(
            &ctx,
            Duration::from_millis(50),
            Duration::from_millis(120),
            false,
        )
    });
    AddGauge(Some(&d.metrics.finishedSizeGauge), 500.0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while d.GetStatus().FinishedBytes != 500.0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.GetStatus().FinishedBytes, 500.0);
    AddGauge(Some(&d.metrics.finishedSizeGauge), 1000.0);
    cancel.call();
    worker.join().unwrap();
    assert_eq!(d.GetStatus().FinishedBytes, 1500.0);
    let final_status = d.GetStatus();
    thread::sleep(Duration::from_millis(60));
    assert_eq!(d.GetStatus(), final_status);
}

#[test]
fn accelerated_log_tick_refreshes_before_five_second_status_tick() {
    let d = Arc::new(status_dumper());
    let (ctx, cancel) = tcontext::Background().WithCancel();
    let view = d.clone();
    let worker = thread::spawn(move || {
        view.runLogProgressWithTicks(&ctx, statusRefreshTick, logProgressTick, true)
    });
    thread::sleep(Duration::from_millis(30));
    d.metrics.totalChunks.store(4, Ordering::SeqCst);
    d.metrics.completedChunks.store(1, Ordering::SeqCst);
    d.metrics.progressReady.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(2);
    while d.GetStatus().ProgressPercent != Some(25.0) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let snapshot = d.GetStatus();
    cancel.call();
    worker.join().unwrap();
    assert_eq!(snapshot.ProgressPercent, Some(25.0));
}

#[test]
fn log_interval_average_reads_live_bytes_even_with_stale_snapshot() {
    let d = Arc::new(status_dumper());
    d.totalTables.store(1, Ordering::SeqCst);
    let logger = log::NewAppLogger(log::ZapLogger::capture(log::Level::Info));
    let (ctx, cancel) = tcontext::Background()
        .WithLogger(logger.clone())
        .WithCancel();
    let view = d.clone();
    let worker = thread::spawn(move || {
        view.runLogProgressWithTicks(&ctx, statusRefreshTick, Duration::from_millis(120), false)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while d.GetStatus().TotalTables != 1 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    AddGauge(Some(&d.metrics.finishedSizeGauge), 2000.0);
    while logger.entries().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let stale = d.GetStatus();
    let entries = logger.entries();
    cancel.call();
    worker.join().unwrap();
    assert_eq!(stale.FinishedBytes, 0.0);
    assert_eq!(entries.len(), 1);
    let average = entries[0]
        .split("average speed(MiB/s)=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    let expected = 2000.0 / 0.120 / 1048576.0;
    assert!((average / expected - 1.0).abs() < 0.2, "{}", entries[0]);
    assert_eq!(d.GetStatus().FinishedBytes, 2000.0);
}

#[test]
fn production_five_second_refresh_samples_speed_and_stops() {
    let d = Arc::new(status_dumper());
    d.totalTables.store(1, Ordering::SeqCst);
    let logger = log::NewAppLogger(log::ZapLogger::capture(log::Level::Info));
    let (ctx, cancel) = tcontext::Background()
        .WithLogger(logger.clone())
        .WithCancel();
    let view = d.clone();
    let worker = thread::spawn(move || view.runLogProgress(&ctx));
    let deadline = Instant::now() + Duration::from_secs(7);
    while d.GetStatus().TotalTables != 1 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let initial = d.GetStatus();
    AddGauge(Some(&d.metrics.finishedSizeGauge), 500.0);
    while d.GetStatus().FinishedBytes != 500.0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let sampled = d.GetStatus();
    cancel.call();
    worker.join().unwrap();
    assert_eq!(sampled.FinishedBytes, 500.0);
    assert!(
        (sampled.CurrentSpeedBPS - 100.0).abs() < 3.0,
        "{:?}",
        sampled
    );
    assert_eq!(initial.FinishedBytes, 0.0);
    assert!(logger.entries().is_empty());
}

#[test]
fn production_failpoint_refreshes_on_log_tick() {
    // Isolate environment changes in a child test process; other tasks/test threads are unaffected.
    if std::env::var("ASTER_TASK36_FAILPOINT_CHILD").is_err() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "status_test::production_failpoint_refreshes_on_log_tick",
                "--nocapture",
            ])
            .env("ASTER_TASK36_FAILPOINT_CHILD", "1")
            .env(
                "GO_FAILPOINTS",
                "github.com/pingcap/tidb/dumpling/export/EnableLogProgress=return()",
            )
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }
    assert!(failpoint_inject("EnableLogProgress"));
    let d = Arc::new(status_dumper());
    d.totalTables.store(1, Ordering::SeqCst);
    let logger = log::NewAppLogger(log::ZapLogger::capture(log::Level::Info));
    let (ctx, cancel) = tcontext::Background()
        .WithLogger(logger.clone())
        .WithCancel();
    let view = d.clone();
    let worker = thread::spawn(move || view.runLogProgress(&ctx));
    let deadline = Instant::now() + Duration::from_secs(3);
    while d.GetStatus().TotalTables != 1 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    d.metrics.totalChunks.store(4, Ordering::SeqCst);
    d.metrics.completedChunks.store(1, Ordering::SeqCst);
    d.metrics.progressReady.store(true, Ordering::SeqCst);
    while logger.entries().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let entries = logger.entries();
    cancel.call();
    worker.join().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].contains("chunks progress=25.00 %"),
        "{}",
        entries[0]
    );
}
