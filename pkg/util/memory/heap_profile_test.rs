// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::arbitrator::{ArbitratorRuntimeStats, NewMemArbitrator};
use super::global_arbitrator::HeapProfileRuntime;
use super::heap_profile::{HeapProfileCollector, parse_heap_profile_file_name};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

#[global_allocator]
static GO_MERGE_28_ALLOCATOR: rpprof::alloc::AllocProfiler = rpprof::alloc::AllocProfiler::system();

#[test]
fn go_merge_28_threshold_capture_and_reset() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_700_000);
    let clock = Arc::new(std::sync::Mutex::new(now));
    let writes = Arc::new(AtomicUsize::new(0));
    let collector = HeapProfileCollector::new_with_hooks(
        dir.path().join("heap_profiles"),
        {
            let clock = clock.clone();
            Arc::new(move || *clock.lock().unwrap())
        },
        {
            let writes = writes.clone();
            Arc::new(move |out| {
                writes.fetch_add(1, Ordering::SeqCst);
                out.write_all(b"profile")
            })
        },
    );
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    for used in [600, 720, 820, 870] {
        arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
            heap_alloc: used,
            heap_inuse: used,
            ..Default::default()
        });
        collector.try_capture(&arbitrator);
        *clock.lock().unwrap() += Duration::from_secs(10);
    }
    assert_eq!(writes.load(Ordering::SeqCst), 3);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 640,
        heap_inuse: 640,
        ..Default::default()
    });
    collector.try_capture(&arbitrator);
    *clock.lock().unwrap() += Duration::from_secs(60);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 740,
        heap_inuse: 740,
        ..Default::default()
    });
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 4);
}

#[test]
fn go_merge_28_profile_filename_validation() {
    assert!(parse_heap_profile_file_name("2026-08-14T10-00-00+0800.85pct.meta.json").is_some());
    assert!(parse_heap_profile_file_name("2026-08-14T10-00-00+0800.90pct.pprof").is_none());
    assert!(parse_heap_profile_file_name("2026-08-14T10-00-00+0800.085pct.pprof").is_none());
}

#[test]
fn go_merge_28_capture_metadata_and_failure_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("heap_profiles");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_700_000);
    let collector = HeapProfileCollector::new_with_hooks(
        target.clone(),
        Arc::new(move || now),
        Arc::new(|out| out.write_all(b"profile")),
    );
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 700,
        heap_inuse: 700,
        ..Default::default()
    });
    assert!(collector.capture(&arbitrator, 70));
    let names: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(names.len(), 2);
    let profile = names
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "pprof"))
        .unwrap();
    assert_eq!(std::fs::read(profile).unwrap(), b"profile");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(profile).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let metadata = names
        .iter()
        .find(|p| p.to_string_lossy().ends_with(".meta.json"))
        .unwrap();
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(metadata).unwrap()).unwrap();
    assert_eq!(data["version"], 1);
    assert_eq!(data["threshold_pct"], 70);
    assert_eq!(data["state"]["mem_inuse_bytes"], 700);
    assert_eq!(data["state"]["limit_bytes"], 1000);

    let failed = tempfile::tempdir().unwrap();
    let collector = HeapProfileCollector::new_with_hooks(
        failed.path().join("heap_profiles"),
        Arc::new(move || now),
        Arc::new(|_| Err(std::io::ErrorKind::BrokenPipe.into())),
    );
    assert!(collector.capture(&arbitrator, 70));
    assert_eq!(
        std::fs::read_dir(failed.path().join("heap_profiles"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn go_merge_28_retention_removes_old_groups_and_orphans() {
    let dir = tempfile::tempdir().unwrap();
    for minute in 0..12 {
        let base = format!("2026-08-14T10-{minute:02}-00+0800.70pct");
        std::fs::write(dir.path().join(format!("{base}.pprof")), b"profile").unwrap();
        std::fs::write(dir.path().join(format!("{base}.meta.json")), b"{}").unwrap();
    }
    std::fs::write(
        dir.path().join("2026-08-14T10-12-00+0800.70pct.meta.json"),
        b"{}",
    )
    .unwrap();
    std::fs::write(dir.path().join(".heap-profile.stale.tmp"), b"tmp").unwrap();
    std::fs::write(dir.path().join("manual.pprof"), b"manual").unwrap();
    let collector = HeapProfileCollector::new_with_hooks(
        dir.path().to_path_buf(),
        Arc::new(SystemTime::now),
        Arc::new(|_| Ok(())),
    );
    collector.enforce_retention();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 21);
    assert!(
        !dir.path()
            .join("2026-08-14T10-00-00+0800.70pct.pprof")
            .exists()
    );
    assert!(
        dir.path()
            .join("2026-08-14T10-11-00+0800.70pct.pprof")
            .exists()
    );
    assert!(dir.path().join("manual.pprof").exists());
}

#[test]
fn go_merge_28_default_writer_emits_heap_pprof() {
    use rpprof::protos::Message;
    let _lock = super::global_arbitrator::GLOBAL_TEST_LOCK.lock().unwrap();
    struct StopProfiler;
    impl Drop for StopProfiler {
        fn drop(&mut self) {
            rpprof::alloc::stop();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let collector = HeapProfileCollector::new_default(dir.path().join("heap_profiles"));
    rpprof::alloc::set_sample_rate(512 * 1024);
    rpprof::alloc::start();
    let _stop = StopProfiler;
    let data = vec![3_u8; 8 * 1024 * 1024];
    std::hint::black_box(&data);
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 700,
        heap_inuse: 700,
        ..Default::default()
    });
    assert!(collector.capture(&arbitrator, 70));
    let path = std::fs::read_dir(dir.path().join("heap_profiles"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "pprof"))
        .unwrap();
    let profile = rpprof::protos::Profile::decode(&std::fs::read(path).unwrap()[..]).unwrap();
    assert_eq!(profile.sample_type.len(), 4);
    assert!(!profile.sample.is_empty());
}

#[test]
fn go_merge_28_cutoff_limit_change_and_disabled_mode() {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(std::sync::Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_700_000),
    ));
    let writes = Arc::new(AtomicUsize::new(0));
    let collector = HeapProfileCollector::new_with_hooks(
        dir.path().join("profiles"),
        {
            let now = now.clone();
            Arc::new(move || *now.lock().unwrap())
        },
        {
            let writes = writes.clone();
            Arc::new(move |out| {
                writes.fetch_add(1, Ordering::SeqCst);
                out.write_all(b"profile")
            })
        },
    );
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    let update = |used| {
        arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
            heap_alloc: used,
            heap_inuse: used,
            ..Default::default()
        })
    };
    update(910);
    collector.try_capture(&arbitrator);
    update(820);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    update(640);
    collector.try_capture(&arbitrator);
    update(700);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    *now.lock().unwrap() += Duration::from_secs(60);
    arbitrator.SetLimit(2000);
    update(1400);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeDisable);
    assert!(!collector.capture(&arbitrator, 70));
    assert_eq!(writes.load(Ordering::SeqCst), 2);
}

#[test]
fn go_merge_28_setup_failure_retries_after_cooldown() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("profiles");
    std::fs::write(&blocked, b"not a directory").unwrap();
    let now = Arc::new(std::sync::Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_700_000),
    ));
    let writes = Arc::new(AtomicUsize::new(0));
    let collector = HeapProfileCollector::new_with_hooks(
        blocked.clone(),
        {
            let now = now.clone();
            Arc::new(move || *now.lock().unwrap())
        },
        {
            let writes = writes.clone();
            Arc::new(move |out| {
                writes.fetch_add(1, Ordering::SeqCst);
                out.write_all(b"profile")
            })
        },
    );
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 700,
        heap_inuse: 700,
        ..Default::default()
    });
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    std::fs::remove_file(&blocked).unwrap();
    *now.lock().unwrap() += Duration::from_millis(100);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    *now.lock().unwrap() += Duration::from_secs(60);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[test]
fn go_merge_28_emergency_capture_and_check_interval() {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(std::sync::Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_700_000),
    ));
    let writes = Arc::new(AtomicUsize::new(0));
    let collector = HeapProfileCollector::new_with_hooks(
        dir.path().join("profiles"),
        {
            let now = now.clone();
            Arc::new(move || *now.lock().unwrap())
        },
        {
            let writes = writes.clone();
            Arc::new(move |out| {
                writes.fetch_add(1, Ordering::SeqCst);
                out.write_all(b"profile")
            })
        },
    );
    assert!(collector.should_check());
    assert!(!collector.should_check());
    *now.lock().unwrap() += Duration::from_secs(1);
    assert!(collector.should_check());
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 1001,
        heap_inuse: 1001,
        ..Default::default()
    });
    assert!(arbitrator.AtOOMRisk());
    assert_eq!(arbitrator.Allocated(), 0);
    assert!(arbitrator.OutOfControl() > 0);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    *now.lock().unwrap() += Duration::from_secs(29);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    *now.lock().unwrap() += Duration::from_secs(1);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    let names: Vec<_> = std::fs::read_dir(dir.path().join("profiles"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names
            .iter()
            .filter(|name| name.ends_with(".95pct.pprof"))
            .count(),
        2
    );
}

fn go_merge_28_runtime_stats() -> ArbitratorRuntimeStats {
    ArbitratorRuntimeStats {
        heap_alloc: 700,
        heap_inuse: 700,
        ..Default::default()
    }
}

#[test]
fn go_merge_28_global_runtime_installs_default_collector() {
    use super::global_arbitrator::{
        CleanupGlobalMemArbitratorForTest, GLOBAL_TEST_LOCK, GlobalMemArbitrator,
        HandleGlobalMemArbitratorRuntime, SetGlobalMemArbitratorLimit,
        SetGlobalMemArbitratorWorkMode, SetRuntimeMemStatsSamplerForTest,
    };
    let _lock = GLOBAL_TEST_LOCK.lock().unwrap();
    CleanupGlobalMemArbitratorForTest();
    let restore = config_crate::restore_func();
    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().to_path_buf();
    config_crate::update_global(|cfg| {
        cfg.log.file.filename.clear();
        cfg.temp_dir = temp.display().to_string();
        cfg.port = 4108;
    });
    SetGlobalMemArbitratorLimit(1000);
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    GlobalMemArbitrator().unwrap().StopAutoRun();
    SetRuntimeMemStatsSamplerForTest(go_merge_28_runtime_stats);
    rpprof::alloc::start();
    let data = vec![1_u8; 8 * 1024 * 1024];
    std::hint::black_box(&data);
    HandleGlobalMemArbitratorRuntime();
    let profiles = dir.path().join("mem_arbitrator-4108").join("heap_profiles");
    assert!(std::fs::read_dir(profiles).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|ext| ext == "pprof")
    }));
    rpprof::alloc::stop();
    CleanupGlobalMemArbitratorForTest();
    restore();
}

#[test]
fn go_merge_28_writer_failure_consumes_threshold_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let writes = Arc::new(AtomicUsize::new(0));
    let collector = HeapProfileCollector::new_with_hooks(
        dir.path().join("profiles"),
        Arc::new(SystemTime::now),
        {
            let writes = writes.clone();
            Arc::new(move |_| {
                writes.fetch_add(1, Ordering::SeqCst);
                Err(std::io::ErrorKind::BrokenPipe.into())
            })
        },
    );
    let arbitrator = NewMemArbitrator(1000);
    arbitrator.SetWorkMode(super::arbitrator::ArbitratorModeStandard);
    arbitrator.HandleRuntimeStats(ArbitratorRuntimeStats {
        heap_alloc: 700,
        heap_inuse: 700,
        ..Default::default()
    });
    collector.try_capture(&arbitrator);
    collector.try_capture(&arbitrator);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}
