// Copyright 2026 AsterSQL.

use crate::plan_replayer::{DumpFileGcChecker, DumpFileStore, parse_dump_time};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

#[test]
/// 合法文件名解析为 UNIX_EPOCH+nanos；非法名返回错误。
fn canonical_plan_replayer_dump_time_parses_nanosecond_suffix_and_rejects_bad_names() {
    assert_eq!(
        parse_dump_time("capture_replayer_123456.zip").unwrap(),
        UNIX_EPOCH + Duration::from_nanos(123456)
    );
    assert_eq!(
        parse_dump_time("capture_replayer_-123456.zip").unwrap(),
        UNIX_EPOCH - Duration::from_nanos(123456)
    );
    assert!(parse_dump_time("replayer.zip").is_err());
    assert!(parse_dump_time("replayer_bad.zip").is_err());
}

#[derive(Default)]
struct FaultTolerantStore {
    deleted: Mutex<Vec<String>>,
    statuses: Mutex<Vec<String>>,
}

impl DumpFileStore for FaultTolerantStore {
    fn list(&self, path: &str) -> Result<Vec<String>, String> {
        if path == "broken" {
            return Err("walk failed".to_string());
        }
        Ok(vec![
            "plan_replayer_capture_100.zip".to_string(),
            "plan_replayer_100.zip".to_string(),
            "trace_100.zip".to_string(),
        ])
    }

    fn delete(&self, path: &str) -> Result<(), String> {
        if path == "plan_replayer_capture_100.zip" {
            return Err("delete failed".to_string());
        }
        self.deleted.lock().unwrap().push(path.to_string());
        Ok(())
    }

    fn delete_status(&self, token: &str) -> Result<(), String> {
        self.statuses.lock().unwrap().push(token.to_string());
        Err("status cleanup failed".to_string())
    }
}

#[test]
fn gc_matches_go_by_continuing_after_walk_delete_and_status_errors() {
    let store = FaultTolerantStore::default();
    let checker = DumpFileGcChecker::new(vec!["broken".to_string(), "good".to_string()]);

    let deleted = checker
        .gc(
            &store,
            UNIX_EPOCH + Duration::from_nanos(1_000),
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();

    assert_eq!(deleted, vec!["plan_replayer_100.zip", "trace_100.zip"]);
    assert_eq!(
        *store.deleted.lock().unwrap(),
        vec!["plan_replayer_100.zip", "trace_100.zip"]
    );
    assert_eq!(
        *store.statuses.lock().unwrap(),
        vec!["plan_replayer_100.zip"]
    );
}

#[test]
fn dump_gc_uses_current_retention_and_keeps_capture_seven_days() {
    let original = astersql_sessionctx_vardef::GetPlanReplayerFileRetentionTime();
    struct Restore(Duration);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetPlanReplayerFileRetentionTime(self.0);
        }
    }
    let _restore = Restore(original);
    // Deletion succeeds for capture files as well, so retention is what keeps them.
    struct Store;
    impl DumpFileStore for Store {
        fn list(&self, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![
                "plan_replayer_capture_100.zip".into(),
                "plan_replayer_100.zip".into(),
                "trace_100.zip".into(),
            ])
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete_status(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let checker = DumpFileGcChecker::new(vec!["good".to_string()]);
    let now = UNIX_EPOCH + Duration::from_secs(2);
    astersql_sessionctx_vardef::SetPlanReplayerFileRetentionTime(Duration::from_secs(3));
    assert!(
        checker
            .gc_with_current_retention(&Store, now)
            .unwrap()
            .is_empty()
    );
    astersql_sessionctx_vardef::SetPlanReplayerFileRetentionTime(Duration::from_secs(1));
    assert_eq!(
        checker.gc_with_current_retention(&Store, now).unwrap(),
        vec!["plan_replayer_100.zip", "trace_100.zip"]
    );
    assert_eq!(
        checker
            .gc_with_current_retention(
                &Store,
                UNIX_EPOCH + Duration::from_secs(7 * 24 * 60 * 60 + 1)
            )
            .unwrap(),
        vec![
            "plan_replayer_capture_100.zip",
            "plan_replayer_100.zip",
            "trace_100.zip"
        ]
    );
}
