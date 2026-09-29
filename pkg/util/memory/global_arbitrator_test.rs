// Copyright 2026 AsterSQL.

use super::arbitrator::{
    ArbitrateHelper, ArbitrationPriorityLow, ArbitratorRuntimeStats, ArbitratorStopReason,
    CancelReceiver, NewArbitrationContext,
};
use super::global_arbitrator::{
    CleanupGlobalMemArbitratorForTest, GlobalMemArbitrator, HandleGlobalMemArbitratorRuntime,
    HeapProfileRuntime, MemArbitratorStateDir, RuntimeMemStateRecorder,
    SetGlobalMemArbitratorWorkMode, SetHeapProfileRuntimeForTest, SetRuntimeMemStatsSamplerForTest,
    SetupGlobalMemArbitratorForTest,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn go_merge_26_state_directory_follows_log_or_temp_dir() {
    let temp = tempfile::tempdir().unwrap();
    let temp_dir = temp.path().join("temp");
    let log = temp.path().join("logs").join("tidb.log");
    assert_eq!(
        MemArbitratorStateDir(&log, &temp_dir, 4000),
        temp.path().join("logs").join("mem_arbitrator")
    );
    assert_eq!(
        MemArbitratorStateDir(std::path::Path::new("tidb.log"), &temp_dir, 4000),
        temp_dir.join("mem_arbitrator-4000")
    );
}

#[test]
fn go_merge_26_lazy_initialization_restores_persisted_tuning() {
    let _guard = super::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    struct RestoreConfig(Option<Box<dyn FnOnce()>>);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                restore();
            }
        }
    }
    let restore = RestoreConfig(Some(Box::new(config_crate::restore_func())));
    CleanupGlobalMemArbitratorForTest();
    let dir = tempfile::tempdir().unwrap();
    let temp_dir = dir.path().to_path_buf();
    config_crate::update_global(|cfg| {
        cfg.log.file.filename.clear();
        cfg.temp_dir = temp_dir.display().to_string();
        cfg.port = 4011;
    });
    let state_dir = dir.path().join("mem_arbitrator-4011");
    let expected = serde_json::json!({
        "version": 1,
        "last-risk": {"heap": 900, "quota": 100},
        "magnif": 1300,
        "pool-medium-cap": 4096,
    });
    RuntimeMemStateRecorder::new(&state_dir)
        .store(&expected)
        .unwrap();
    assert!(SetGlobalMemArbitratorWorkMode("priority".to_owned()));
    let core = GlobalMemArbitrator().unwrap();
    core.StopAutoRun();
    assert_eq!(core.MemMagnif(), 1300);
    assert_eq!(core.SuggestPoolInitCap(), 4096);
    assert_eq!(
        RuntimeMemStateRecorder::new(state_dir).load().unwrap(),
        Some(expected)
    );
    CleanupGlobalMemArbitratorForTest();
    drop(restore);
}

#[test]
fn go_merge_26_runtime_handler_checks_and_resets_heap_profiler() {
    struct Profiler {
        checks: AtomicUsize,
        captures: AtomicUsize,
        resets: AtomicUsize,
    }
    impl HeapProfileRuntime for Profiler {
        fn reset_trigger_state(&self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
        fn should_check(&self) -> bool {
            self.checks.fetch_add(1, Ordering::SeqCst);
            HandleGlobalMemArbitratorRuntime();
            true
        }
        fn try_capture(&self, _: &super::arbitrator::MemArbitrator) {
            self.captures.fetch_add(1, Ordering::SeqCst);
        }
    }
    let _guard = super::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    SetupGlobalMemArbitratorForTest(dir.path().display().to_string());
    let profiler = Arc::new(Profiler {
        checks: AtomicUsize::new(0),
        captures: AtomicUsize::new(0),
        resets: AtomicUsize::new(0),
    });
    SetHeapProfileRuntimeForTest(Some(profiler.clone()));
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    SetRuntimeMemStatsSamplerForTest(super::arbitrator::ArbitratorRuntimeStats::default);
    HandleGlobalMemArbitratorRuntime();
    assert_eq!(profiler.checks.load(Ordering::SeqCst), 1);
    assert_eq!(profiler.captures.load(Ordering::SeqCst), 1);
    assert!(SetGlobalMemArbitratorWorkMode("disable".to_owned()));
    HandleGlobalMemArbitratorRuntime();
    assert_eq!(profiler.resets.load(Ordering::SeqCst), 1);
    assert_eq!(profiler.captures.load(Ordering::SeqCst), 1);
    CleanupGlobalMemArbitratorForTest();
}

#[test]
fn go_merge_26_load_uses_exact_versioned_state_file() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(dir.path());
    std::fs::write(
        dir.path().join("mem-state.v1.legacy.json"),
        br#"{"version":1,"stale":true}"#,
    )
    .unwrap();
    assert_eq!(recorder.load().unwrap(), None);

    let expected = serde_json::json!({"version": 1, "magnif": 1300});
    recorder.store(&expected).unwrap();
    assert_eq!(
        RuntimeMemStateRecorder::new(dir.path()).load().unwrap(),
        Some(expected)
    );
}

#[test]
fn go_merge_24_failed_persist_keeps_previous_runtime_state() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("state");
    let recorder = RuntimeMemStateRecorder::new(&base);
    let first = serde_json::json!({"version": 1, "magnif": 1200});
    recorder.store(&first).unwrap();
    assert_eq!(recorder.last_state(), Some(first.clone()));
    std::fs::remove_dir_all(&base).unwrap();
    std::fs::write(&base, b"cannot create directory").unwrap();
    assert!(
        recorder
            .store(&serde_json::json!({"version": 1, "magnif": 1400}))
            .is_err()
    );
    assert_eq!(recorder.last_state(), Some(first));
}

#[test]
fn go_merge_24_loaded_runtime_state_becomes_last_state() {
    let temp = tempfile::tempdir().unwrap();
    let stored = serde_json::json!({"version": 1, "last-risk": {"heap": 200, "quota": 100}});
    RuntimeMemStateRecorder::new(temp.path())
        .store(&stored)
        .unwrap();
    let recorder = RuntimeMemStateRecorder::new(temp.path());
    assert_eq!(recorder.load().unwrap(), Some(stored.clone()));
    assert_eq!(recorder.last_state(), Some(stored));
}

#[test]
fn go_merge_24_medium_cap_persist_preserves_last_risk() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(temp.path());
    assert!(
        recorder
            .persist_pool_medium_if_changed(0, 120, i64::MAX - 10_000)
            .unwrap()
    );
    let first = recorder.load().unwrap().unwrap();
    assert_eq!(first["pool-medium-cap"], 120);
    assert_eq!(first["last-risk"]["heap"], 0);
    recorder
        .store(&serde_json::json!({
            "version": 1,
            "last-risk": {"heap": 900, "quota": 400},
            "magnif": 2350,
            "pool-medium-cap": 120
        }))
        .unwrap();
    assert!(
        recorder
            .persist_pool_medium_if_changed(2350, 240, i64::MAX - 10_000)
            .unwrap()
    );
    let updated = recorder.load().unwrap().unwrap();
    assert_eq!(updated["pool-medium-cap"], 240);
    assert_eq!(updated["last-risk"]["heap"], 900);
    assert_eq!(updated["magnif"], 2350);
}

#[test]
fn go_merge_24_magnif_decay_persists_last_risk() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(temp.path());
    recorder
        .store(&serde_json::json!({
            "version": 1,
            "last-risk": {"heap": 1_800, "quota": 900},
            "magnif": 2_100,
            "pool-medium-cap": 120,
        }))
        .unwrap();
    assert!(recorder.persist_magnif_if_decreased(1_600).unwrap());
    let updated = recorder.load().unwrap().unwrap();
    assert_eq!(updated["magnif"], 1_600);
    assert_eq!(updated["last-risk"]["heap"], 1_800);
    assert_eq!(updated["pool-medium-cap"], 120);
    assert!(!recorder.persist_magnif_if_decreased(1_700).unwrap());
}

#[test]
fn go_merge_24_global_tick_persists_new_medium_cap() {
    let _guard = super::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    struct Noop;
    impl ArbitrateHelper for Noop {
        fn Stop(&self, _: ArbitratorStopReason) -> bool {
            true
        }
        fn HeapInuse(&self) -> i64 {
            0
        }
        fn Finish(&self) {}
    }
    let temp = tempfile::tempdir().unwrap();
    SetupGlobalMemArbitratorForTest(temp.path().display().to_string());
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    let core = GlobalMemArbitrator().unwrap();
    assert!(core.SetLimit(10_000));
    let now_sec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    core.setUnixTimeSec(now_sec);
    let root = core.EmplaceRootPool(1).unwrap();
    let ctx = NewArbitrationContext(
        CancelReceiver::none(),
        Some(Arc::new(Noop)),
        ArbitrationPriorityLow,
        false,
        false,
    );
    assert!(core.RestartEntryByContext(root, ctx));
    assert!(core.ResetRootPoolByID(1, 100, true));
    core.RunOneRound();
    assert_eq!(core.SuggestPoolInitCap(), 120);
    SetRuntimeMemStatsSamplerForTest(ArbitratorRuntimeStats::default);
    HandleGlobalMemArbitratorRuntime();
    let recorded = RuntimeMemStateRecorder::new(temp.path())
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(recorded["pool-medium-cap"], 120);
    assert_eq!(recorded["last-risk"]["heap"], 0);
    CleanupGlobalMemArbitratorForTest();
}

#[test]
fn setup_removes_stale_runtime_memory_state_like_go() {
    let _guard = super::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(dir.path());
    recorder
        .store(&serde_json::json!({"version": 1, "stale": true}))
        .unwrap();

    SetupGlobalMemArbitratorForTest(dir.path().display().to_string());

    assert_eq!(recorder.load().unwrap(), None);
    CleanupGlobalMemArbitratorForTest();
}
