// Copyright 2026 AsterSQL.

// Plan Replayer handle 的 Go 对抗测试：任务收集、投递/处理、GC 与原始 SQL 保真。

#[test]
fn collector_matches_go_empty_success_failed_and_duplicate_status_cases() {
    use crate::plan_replayer::{PlanReplayerTaskCollector, PlanReplayerTaskKey};
    use std::collections::BTreeSet;

    let a = PlanReplayerTaskKey {
        sql_digest: "123".into(),
        plan_digest: "123".into(),
    };
    let b = PlanReplayerTaskKey {
        sql_digest: "345".into(),
        plan_digest: "345".into(),
    };
    let collector = PlanReplayerTaskCollector::default();

    collector.collect(Vec::new(), &BTreeSet::new());
    assert!(collector.tasks().is_empty());
    collector.collect(vec![a.clone()], &BTreeSet::new());
    assert_eq!(collector.tasks(), vec![a.clone()]);

    // Go 只把 fail_reason IS NULL 的成功状态视为 handled。
    collector.collect(
        vec![a.clone(), b.clone(), a.clone()],
        &BTreeSet::from([a.clone()]),
    );
    assert_eq!(collector.tasks(), vec![b.clone()]);

    // 失败状态不会进入 handled 集合，因此两个注册任务都必须保留。
    collector.collect(vec![a.clone(), b.clone()], &BTreeSet::new());
    assert_eq!(collector.tasks(), vec![a.clone(), b]);
    collector.remove(&a);
    assert_eq!(collector.tasks().len(), 1);
}

#[test]
fn handle_send_and_finished_state_match_go_capture_lifecycle() {
    use crate::plan_replayer::{
        PlanReplayerDumpTask, PlanReplayerDumpTaskStatus, PlanReplayerDumper, PlanReplayerHandle,
        PlanReplayerTaskKey, handle_dump_task,
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    struct Dumper {
        calls: Arc<Mutex<usize>>,
        fail: bool,
    }
    impl PlanReplayerDumper for Dumper {
        fn dump(
            &self,
            _task: &mut PlanReplayerDumpTask,
        ) -> Result<Vec<crate::plan_replayer::PlanReplayerStatusRecord>, String> {
            *self.calls.lock().expect("dump count lock poisoned") += 1;
            if self.fail {
                Err("dump failed".into())
            } else {
                Ok(Vec::new())
            }
        }
    }

    let normal_key = PlanReplayerTaskKey {
        sql_digest: "sql".into(),
        plan_digest: "plan".into(),
    };
    let continuous_key = PlanReplayerTaskKey {
        sql_digest: "sql".into(),
        plan_digest: "*".into(),
    };
    let (handle, receiver) = PlanReplayerHandle::new(2);
    let collector = handle.collector();
    collector.collect(
        vec![normal_key.clone(), continuous_key.clone()],
        &BTreeSet::new(),
    );

    assert!(handle.send_task(PlanReplayerDumpTask {
        key: normal_key.clone(),
        ..Default::default()
    }));
    assert_eq!(collector.tasks(), vec![continuous_key.clone()]);
    let mut normal = receiver.recv().expect("normal task must be queued");

    assert!(handle.send_task(PlanReplayerDumpTask {
        key: continuous_key.clone(),
        is_continuous_capture: true,
        ..Default::default()
    }));
    assert_eq!(collector.tasks(), vec![continuous_key]);
    let mut continuous = receiver.recv().expect("continuous task must be queued");

    let calls = Arc::new(Mutex::new(0));
    let dumper = Dumper {
        calls: Arc::clone(&calls),
        fail: false,
    };
    let status = PlanReplayerDumpTaskStatus::default();
    assert!(handle_dump_task(&dumper, &status, &mut normal));
    assert_eq!(status.running_len(), 0);
    assert!(handle_dump_task(&dumper, &status, &mut continuous));
    assert!(!handle_dump_task(&dumper, &status, &mut continuous));
    assert_eq!(*calls.lock().expect("dump count lock poisoned"), 2);
    assert_eq!(status.running_len(), 0);
    assert_eq!(status.finished_len(), 1);
    status.clean_finished();
    assert_eq!(status.finished_len(), 0);

    let failing = Dumper { calls, fail: true };
    assert!(!handle_dump_task(&failing, &status, &mut normal));
    assert_eq!(
        status.running_len(),
        0,
        "errors must release the running key"
    );
}

#[test]
fn gc_removes_expired_replayer_file_and_its_status_like_go() {
    use crate::plan_replayer::{DumpFileGcChecker, DumpFileStore};
    use std::sync::Mutex;
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Default)]
    struct Store {
        deleted: Mutex<Vec<String>>,
        statuses: Mutex<Vec<String>>,
    }
    impl DumpFileStore for Store {
        fn list(&self, path: &str) -> Result<Vec<String>, String> {
            if path == "broken" {
                return Err("walk failed".into());
            }
            Ok(vec![
                "replayer_single_xxxxxx_100.zip".into(),
                "trace_100.zip".into(),
                "not-a-dump".into(),
            ])
        }
        fn delete(&self, path: &str) -> Result<(), String> {
            self.deleted
                .lock()
                .expect("delete lock poisoned")
                .push(path.into());
            Ok(())
        }
        fn delete_status(&self, token: &str) -> Result<(), String> {
            self.statuses
                .lock()
                .expect("status lock poisoned")
                .push(token.into());
            Ok(())
        }
    }

    let store = Store::default();
    let checker = DumpFileGcChecker::new(vec!["broken".into(), "plan_replayer".into()]);
    let deleted = checker
        .gc(
            &store,
            UNIX_EPOCH + Duration::from_nanos(1_000),
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("best-effort GC must continue after a path walk error");
    assert_eq!(
        deleted,
        vec!["replayer_single_xxxxxx_100.zip", "trace_100.zip"]
    );
    assert_eq!(
        *store.statuses.lock().expect("status lock poisoned"),
        vec!["replayer_single_xxxxxx_100.zip"]
    );
}

#[test]
fn dump_task_preserves_origin_sql_containing_single_quotes() {
    use crate::plan_replayer::{
        PlanReplayerDumpTask, PlanReplayerDumpTaskStatus, PlanReplayerDumper,
        PlanReplayerStatusRecord, handle_dump_task,
    };
    use std::sync::{Arc, Mutex};

    struct Dumper(Arc<Mutex<Vec<PlanReplayerStatusRecord>>>);
    impl PlanReplayerDumper for Dumper {
        fn dump(
            &self,
            task: &mut PlanReplayerDumpTask,
        ) -> Result<Vec<PlanReplayerStatusRecord>, String> {
            let records = vec![PlanReplayerStatusRecord {
                sql_digest: task.key.sql_digest.clone(),
                plan_digest: task.key.plan_digest.clone(),
                origin_sql: task.statements.first().cloned().unwrap_or_default(),
                token: task.file_name.clone(),
                failed_reason: String::new(),
            }];
            *self.0.lock().expect("record lock poisoned") = records.clone();
            Ok(records)
        }
    }

    let sql = "SELECT * from tableA where SUBSTRING_INDEX(tableA.columnC, '_', 1) = tableA.columnA";
    let mut task = PlanReplayerDumpTask {
        statements: vec![sql.into()],
        file_name: "replayer.zip".into(),
        ..Default::default()
    };
    let records = Arc::new(Mutex::new(Vec::new()));
    assert!(handle_dump_task(
        &Dumper(Arc::clone(&records)),
        &PlanReplayerDumpTaskStatus::default(),
        &mut task
    ));
    assert_eq!(
        records.lock().expect("record lock poisoned")[0].origin_sql,
        sql
    );
}
