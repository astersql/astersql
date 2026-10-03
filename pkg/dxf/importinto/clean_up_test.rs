// Copyright 2026 AsterSQL.

use astersql_ingestor_globalsort::Storage;
use std::collections::HashMap;

use astersql_lightning_verification::DataKVGroupID;

use crate::{Checksum, PostProcessStepMeta, meterDataFromPostProcess};

#[test]
fn meter_data_matches_go_uint64_aggregation() {
    let meta = PostProcessStepMeta {
        Checksum: HashMap::from([
            (
                DataKVGroupID,
                Checksum {
                    KVs: 7,
                    Size: 11,
                    ..Default::default()
                },
            ),
            (
                1,
                Checksum {
                    Size: u64::MAX,
                    ..Default::default()
                },
            ),
            (
                2,
                Checksum {
                    Size: 1,
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };

    assert_eq!(meterDataFromPostProcess(&meta), (7, 11, 0));
}

struct MetadataObservingRuntime;
impl crate::ImportCleanUpRuntime for MetadataObservingRuntime {
    fn is_classic(&self) -> bool {
        false
    }
    fn is_nextgen(&self) -> bool {
        true
    }
    fn restore_table_mode(&self, _: i64, _: i64) -> Result<(), crate::RestoreTableModeError> {
        Ok(())
    }
    fn open_global_sort_store(
        &self,
        uri: &str,
    ) -> Result<Box<dyn crate::ImportCleanUpStorage>, astersql_errors::SharedError> {
        assert!(uri.contains("cleanup-access-key"));
        Ok(Box::new(CleanupMemoryStore::default()))
    }
    fn post_process_meta(
        &self,
        _: i64,
    ) -> Result<Option<PostProcessStepMeta>, astersql_errors::SharedError> {
        Ok(Some(PostProcessStepMeta::default()))
    }
    fn send_meter_data(
        &self,
        task: &astersql_dxf_framework_proto::Task,
        _: i64,
        _: i64,
        _: i64,
    ) -> Result<(), astersql_errors::SharedError> {
        assert!(
            !String::from_utf8_lossy(&task.Meta).contains("cleanup-access-key"),
            "metering must observe immediately redacted metadata"
        );
        Ok(())
    }
}
#[test]
fn cleanup_redacts_metadata_before_metering_but_keeps_live_storage_credentials() {
    let mut meta = crate::TaskMeta::default();
    meta.Plan.CloudStorageURI =
        "s3://bucket/import?access-key=cleanup-access-key&secret-access-key=cleanup-secret".into();
    let mut framework_task = astersql_dxf_framework_scheduler::Task::default();
    framework_task.base.task_type = astersql_dxf_framework_proto::ImportInto.into();
    let mut task = crate::frameworkTaskToImportTask(&framework_task).unwrap();
    task.ID = 42;
    task.State = astersql_dxf_framework_proto::TaskStateSucceed;
    task.Meta = meta.Marshal().unwrap();
    crate::ImportCleanUp::new(std::sync::Arc::new(MetadataObservingRuntime))
        .CleanUp(&mut task)
        .unwrap();
}

#[derive(Default, Clone)]
struct CleanupMemoryStore {
    inner: astersql_ingestor_globalsort::MemoryStorage,
    scans: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    closes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail_scan: bool,
    fail_delete: bool,
}
impl astersql_ingestor_globalsort::Storage for CleanupMemoryStore {
    fn read(&self, path: &str) -> astersql_ingestor_globalsort::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, value: Vec<u8>) -> astersql_ingestor_globalsort::Result<()> {
        self.inner.write(path, value)
    }
    fn delete_files(&self, paths: &[String]) -> astersql_ingestor_globalsort::Result<()> {
        if self.fail_delete {
            return Err(astersql_ingestor_globalsort::Error::InvalidData(
                "delete failed".into(),
            ));
        }
        self.inner.delete_files(paths)
    }
    fn list_prefix(&self, prefix: &str) -> astersql_ingestor_globalsort::Result<Vec<String>> {
        self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_scan {
            return Err(astersql_ingestor_globalsort::Error::InvalidData(
                "scan failed".into(),
            ));
        }
        self.inner.list_prefix(prefix)
    }
}
impl crate::ImportCleanUpStorage for CleanupMemoryStore {
    fn close(&self) {
        self.closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Default)]
struct CleanupRuntime {
    stores: HashMap<String, CleanupMemoryStore>,
    events: std::sync::Mutex<Vec<String>>,
    classic: bool,
    table_error: Option<bool>,
    open_error: bool,
    meter_error: bool,
    parallel_gate: Option<std::sync::Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>>,
}
impl crate::ImportCleanUpRuntime for CleanupRuntime {
    fn is_classic(&self) -> bool {
        self.classic
    }
    fn is_nextgen(&self) -> bool {
        !self.classic
    }
    fn restore_table_mode(
        &self,
        database_id: i64,
        _: i64,
    ) -> Result<(), crate::RestoreTableModeError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("table:{database_id}"));
        match self.table_error {
            Some(true) => Err(crate::RestoreTableModeError::TableNotFound),
            Some(false) => Err(crate::RestoreTableModeError::Other(astersql_errors::New(
                "table failed",
            ))),
            None => Ok(()),
        }
    }
    fn open_global_sort_store(
        &self,
        uri: &str,
    ) -> Result<Box<dyn crate::ImportCleanUpStorage>, astersql_errors::SharedError> {
        self.events.lock().unwrap().push(format!("open:{uri}"));
        if self.open_error {
            return Err(astersql_errors::New("open failed"));
        }
        Ok(Box::new(
            self.stores
                .get(uri)
                .expect("must use unredacted URI")
                .clone(),
        ))
    }
    fn post_process_meta(
        &self,
        task_id: i64,
    ) -> Result<Option<PostProcessStepMeta>, astersql_errors::SharedError> {
        self.events.lock().unwrap().push(format!("post:{task_id}"));
        Ok(Some(PostProcessStepMeta {
            Checksum: HashMap::from([
                (
                    DataKVGroupID,
                    crate::Checksum {
                        KVs: 7,
                        Size: 11,
                        ..Default::default()
                    },
                ),
                (
                    1,
                    crate::Checksum {
                        Size: 13,
                        ..Default::default()
                    },
                ),
            ]),
            ..Default::default()
        }))
    }
    fn send_meter_data(
        &self,
        task: &astersql_dxf_framework_proto::Task,
        rows: i64,
        data: i64,
        index: i64,
    ) -> Result<(), astersql_errors::SharedError> {
        if let Some(gate) = &self.parallel_gate {
            let mut started = gate.0.lock().unwrap();
            *started += 1;
            gate.1.notify_all();
            let (started, timed_out) = gate
                .1
                .wait_timeout_while(started, std::time::Duration::from_secs(2), |count| {
                    *count < 4
                })
                .unwrap();
            assert!(
                !timed_out.timed_out() && *started >= 4,
                "four metering workers must start concurrently"
            );
        }
        assert_eq!((rows, data, index), (7, 11, 13));
        assert!(!String::from_utf8_lossy(&task.Meta).contains("cleanup-access-key"));
        self.events
            .lock()
            .unwrap()
            .push(format!("meter:{}", task.ID));
        if self.meter_error {
            for store in self.stores.values() {
                assert_eq!(store.closes.load(std::sync::atomic::Ordering::Acquire), 1);
                assert!(
                    astersql_ingestor_globalsort::Storage::list_prefix(&store.inner, "")
                        .unwrap()
                        .is_empty()
                );
            }
            return Err(astersql_errors::New("meter failed"));
        }
        Ok(())
    }
}
fn cleanup_task(id: i64, uri: &str, state: &'static str) -> astersql_dxf_framework_proto::Task {
    let mut meta = crate::TaskMeta::default();
    meta.Plan.CloudStorageURI = uri.into();
    meta.Plan.DBID = id;
    meta.Stmt = "IMPORT INTO sensitive".into();
    let mut task = astersql_dxf_framework_scheduler::Task::default();
    task.base.task_type = astersql_dxf_framework_proto::ImportInto.into();
    task.base.id = id;
    task.base.state = state;
    task.meta = meta.Marshal().unwrap();
    crate::frameworkTaskToImportTask(&task).unwrap()
}
fn live_uri(bucket: &str) -> String {
    format!("s3://{bucket}/import?access-key=cleanup-access-key&secret-access-key=cleanup-secret")
}
#[test]
fn cleanup_batch_groups_live_uris_scans_once_preserves_neighbors_and_redacts() {
    use astersql_ingestor_globalsort::Storage;
    use std::sync::atomic::Ordering;
    let uri = live_uri("bucket");
    let other = live_uri("other-bucket");
    let first_store = CleanupMemoryStore::default();
    let other_store = CleanupMemoryStore::default();
    for path in ["42/data", "p00110000/43/stat", "kept/data"] {
        first_store.write(path, b"data".to_vec()).unwrap();
    }
    for path in ["44/data", "kept/data"] {
        other_store.write(path, b"data".to_vec()).unwrap();
    }
    let runtime = std::sync::Arc::new(CleanupRuntime {
        stores: HashMap::from([
            (uri.clone(), first_store.clone()),
            (other.clone(), other_store.clone()),
        ]),
        ..Default::default()
    });
    let mut tasks = vec![
        cleanup_task(42, &uri, astersql_dxf_framework_proto::TaskStateSucceed),
        cleanup_task(43, &uri, astersql_dxf_framework_proto::TaskStateFailed),
        cleanup_task(44, &other, astersql_dxf_framework_proto::TaskStateSucceed),
    ];
    crate::ImportCleanUp::new(runtime.clone())
        .CleanUpBatch(&mut tasks)
        .unwrap();
    assert_eq!(first_store.scans.load(Ordering::SeqCst), 1);
    assert_eq!(other_store.scans.load(Ordering::SeqCst), 1);
    assert_eq!(first_store.closes.load(Ordering::SeqCst), 1);
    assert_eq!(other_store.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        first_store.inner.list_prefix("").unwrap(),
        vec!["kept/data"]
    );
    assert_eq!(
        other_store.inner.list_prefix("").unwrap(),
        vec!["kept/data"]
    );
    for task in tasks {
        let meta = crate::TaskMeta::Unmarshal(&task.Meta).unwrap();
        assert!(meta.Stmt.is_empty());
        assert!(meta.Plan.CloudStorageURI.contains("access-key=xxxxxx"));
        assert!(!meta.Plan.CloudStorageURI.contains("cleanup-access-key"));
    }
    let events = runtime.events.lock().unwrap();
    let last_open = events
        .iter()
        .rposition(|event| event.starts_with("open:"))
        .unwrap();
    let first_post = events
        .iter()
        .position(|event| event.starts_with("post:"))
        .unwrap();
    assert!(last_open < first_post);
    assert!(events.contains(&"meter:42".into()));
    assert!(events.contains(&"meter:44".into()));
    assert!(!events.contains(&"meter:43".into()));
}
#[test]
fn cleanup_batch_errors_close_store_and_prevent_metering() {
    use std::sync::atomic::Ordering;
    let uri = live_uri("bucket");
    for (scan, delete, open, expected) in [
        (true, false, false, "scan failed"),
        (false, true, false, "delete failed"),
        (false, false, true, "open failed"),
    ] {
        let store = CleanupMemoryStore {
            fail_scan: scan,
            fail_delete: delete,
            ..Default::default()
        };
        let runtime = std::sync::Arc::new(CleanupRuntime {
            stores: HashMap::from([(uri.clone(), store.clone())]),
            open_error: open,
            ..Default::default()
        });
        let mut tasks = vec![cleanup_task(
            42,
            &uri,
            astersql_dxf_framework_proto::TaskStateSucceed,
        )];
        let error = crate::ImportCleanUp::new(runtime.clone())
            .CleanUpBatch(&mut tasks)
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(store.closes.load(Ordering::SeqCst), usize::from(!open));
        assert!(
            !runtime
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.starts_with("post:"))
        );
        assert!(!String::from_utf8_lossy(&tasks[0].Meta).contains("cleanup-access-key"));
    }
}
#[test]
fn cleanup_batch_table_modes_precede_files_and_missing_table_is_ignored() {
    let uri = live_uri("bucket");
    for error in [None, Some(true), Some(false)] {
        let runtime = std::sync::Arc::new(CleanupRuntime {
            classic: true,
            table_error: error,
            stores: HashMap::from([(uri.clone(), CleanupMemoryStore::default())]),
            ..Default::default()
        });
        let mut tasks = vec![
            cleanup_task(42, &uri, astersql_dxf_framework_proto::TaskStateSucceed),
            cleanup_task(43, &uri, astersql_dxf_framework_proto::TaskStateFailed),
        ];
        let result = crate::ImportCleanUp::new(runtime.clone()).CleanUpBatch(&mut tasks);
        let events = runtime.events.lock().unwrap();
        if error == Some(false) {
            assert_eq!(result.unwrap_err().to_string(), "table failed");
            assert_eq!(*events, vec!["table:42"]);
            assert!(!String::from_utf8_lossy(&tasks[0].Meta).contains("cleanup-access-key"));
            assert!(String::from_utf8_lossy(&tasks[1].Meta).contains("cleanup-access-key"));
        } else {
            result.unwrap();
            assert_eq!(&events[..2], &["table:42", "table:43"]);
            assert!(events[2].starts_with("open:"));
            assert_eq!(events.len(), 3);
        }
    }
}
#[test]
fn cleanup_batch_empty_local_and_malformed_inputs_follow_go_order() {
    let runtime = std::sync::Arc::new(CleanupRuntime::default());
    let cleaner = crate::ImportCleanUp::new(runtime.clone());
    cleaner.CleanUpBatch(&mut []).unwrap();
    let mut local = [cleanup_task(
        1,
        "",
        astersql_dxf_framework_proto::TaskStateSucceed,
    )];
    cleaner.CleanUpBatch(&mut local).unwrap();
    assert!(runtime.events.lock().unwrap().is_empty());
    let uri = live_uri("bucket");
    let mut tasks = vec![
        cleanup_task(42, &uri, astersql_dxf_framework_proto::TaskStateFailed),
        cleanup_task(43, &uri, astersql_dxf_framework_proto::TaskStateFailed),
    ];
    tasks[1].Meta = b"invalid".to_vec();
    assert!(cleaner.CleanUpBatch(&mut tasks).is_err());
    assert!(!String::from_utf8_lossy(&tasks[0].Meta).contains("cleanup-access-key"));
    assert_eq!(tasks[1].Meta, b"invalid");
    assert!(runtime.events.lock().unwrap().is_empty());
}
#[test]
fn cleanup_batch_meter_failure_occurs_after_files() {
    use astersql_ingestor_globalsort::Storage;
    let uri = live_uri("bucket");
    let store = CleanupMemoryStore::default();
    store.write("42/data", b"data".to_vec()).unwrap();
    let runtime = std::sync::Arc::new(CleanupRuntime {
        meter_error: true,
        stores: HashMap::from([(uri.clone(), store.clone())]),
        ..Default::default()
    });
    let mut tasks = vec![
        cleanup_task(42, &uri, astersql_dxf_framework_proto::TaskStateSucceed),
        cleanup_task(43, &uri, astersql_dxf_framework_proto::TaskStateSucceed),
    ];
    assert_eq!(
        crate::ImportCleanUp::new(runtime.clone())
            .CleanUpBatch(&mut tasks)
            .unwrap_err()
            .to_string(),
        "meter failed"
    );
    assert!(store.inner.list_prefix("").unwrap().is_empty());
    assert_eq!(store.closes.load(std::sync::atomic::Ordering::Acquire), 1);
}
#[test]
fn registered_import_cleanup_exposes_batch_and_writes_redaction_on_error() {
    let uri = live_uri("bucket");
    let runtime = std::sync::Arc::new(CleanupRuntime {
        open_error: true,
        ..Default::default()
    });
    crate::RegisterImportCleanUpFactory(runtime);
    let factory = astersql_dxf_framework_scheduler::get_scheduler_cleanup_factory(
        astersql_dxf_framework_proto::ImportInto,
    )
    .unwrap();
    let cleaner = factory();
    let mut tasks: Vec<_> = [42, 43]
        .into_iter()
        .map(|id| {
            let task = cleanup_task(id, &uri, astersql_dxf_framework_proto::TaskStateFailed);
            let mut framework_task = astersql_dxf_framework_scheduler::Task::default();
            framework_task.base.task_type = astersql_dxf_framework_proto::ImportInto.into();
            framework_task.base.id = id;
            framework_task.meta = task.Meta;
            framework_task
        })
        .collect();
    assert_eq!(
        cleaner
            .batch_cleanup()
            .unwrap()
            .clean_up_batch(&mut tasks)
            .unwrap_err()
            .to_string(),
        "open failed"
    );
    for task in tasks {
        assert!(!String::from_utf8_lossy(&task.meta).contains("cleanup-access-key"));
    }
}

#[test]
fn cleanup_batch_meters_eight_tasks_with_four_concurrent_workers() {
    let uri = live_uri("parallel-meter");
    let runtime = std::sync::Arc::new(CleanupRuntime {
        stores: HashMap::from([(uri.clone(), CleanupMemoryStore::default())]),
        parallel_gate: Some(std::sync::Arc::new((
            std::sync::Mutex::new(0),
            std::sync::Condvar::new(),
        ))),
        ..Default::default()
    });
    let mut tasks: Vec<_> = (1..=8)
        .map(|id| cleanup_task(id, &uri, astersql_dxf_framework_proto::TaskStateSucceed))
        .collect();
    crate::ImportCleanUp::new(runtime.clone())
        .CleanUpBatch(&mut tasks)
        .unwrap();
    assert_eq!(
        runtime
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.starts_with("meter:"))
            .count(),
        8
    );
}

#[test]
fn parallel_metering_cancels_pending_tasks_joins_workers_and_preserves_first_error() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    let tasks: Vec<_> = (1..=8)
        .map(|id| {
            cleanup_task(
                id,
                &live_uri("cancel"),
                astersql_dxf_framework_proto::TaskStateSucceed,
            )
        })
        .collect();
    let refs: Vec<_> = tasks.iter().collect();
    let started = AtomicUsize::new(0);
    let exited = AtomicUsize::new(0);
    let barrier = Arc::new(Barrier::new(4));
    let parent = astersql_dxf_framework_metering::Context::background();
    let error = astersql_errors::New("metering failed");
    let returned =
        crate::clean_up::sendMeterOnCleanUpInParallel(&parent, &refs, |context, task| {
            started.fetch_add(1, Ordering::AcqRel);
            barrier.wait();
            let result = if task.ID == 1 {
                Err(error.clone())
            } else {
                while !context.is_cancelled() {
                    std::thread::yield_now();
                }
                Err(astersql_errors::New("context canceled"))
            };
            exited.fetch_add(1, Ordering::AcqRel);
            result
        })
        .unwrap_err();
    assert!(returned.ptr_eq(&error));
    assert_eq!(started.load(Ordering::Acquire), 4);
    assert_eq!(exited.load(Ordering::Acquire), 4);
    assert!(!parent.is_cancelled());
}

#[test]
fn parallel_metering_propagates_parent_cancellation_and_recovers_panic() {
    let task = cleanup_task(
        1,
        &live_uri("panic"),
        astersql_dxf_framework_proto::TaskStateSucceed,
    );
    let parent = astersql_dxf_framework_metering::Context::background();
    parent.cancel();
    assert_eq!(
        crate::clean_up::sendMeterOnCleanUpInParallel(&parent, &[&task], |_, _| panic!(
            "must not send cancelled task"
        ))
        .unwrap_err()
        .to_string(),
        "context canceled"
    );
    assert!(
        crate::clean_up::sendMeterOnCleanUpInParallel(&parent, &[], |_, _| panic!("empty")).is_ok()
    );
    let deadline = astersql_dxf_framework_metering::Context::background()
        .with_timeout(std::time::Duration::ZERO);
    assert_eq!(
        crate::clean_up::sendMeterOnCleanUpInParallel(&deadline, &[&task], |_, _| panic!(
            "expired"
        ))
        .unwrap_err()
        .to_string(),
        "context deadline exceeded"
    );
    let parent = astersql_dxf_framework_metering::Context::background();
    assert_eq!(
        crate::clean_up::sendMeterOnCleanUpInParallel(&parent, &[&task], |_, _| panic!(
            "metering panic"
        ))
        .unwrap_err()
        .to_string(),
        "metering panic"
    );
}

#[test]
fn cleanup_drain_moves_all_bounded_batches_through_real_sql_history() {
    use astersql_dxf_framework_scheduler as scheduler;
    struct Cleaner(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl scheduler::CleanUpRoutine for Cleaner {
        fn clean_up(&self, task: &mut scheduler::Task) -> scheduler::Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            task.meta = b"cleaned-metadata".to_vec();
            Ok(())
        }
    }
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let storage_manager = session.ImportTaskManager().unwrap();
    storage_manager
        .InitMeta((), ":4000".into(), "".into())
        .unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cleaner_calls = calls.clone();
    scheduler::RegisterSchedulerCleanUpFactory(
        "sql-history-drain",
        std::sync::Arc::new(move || std::sync::Arc::new(Cleaner(cleaner_calls.clone()))),
    );
    let restore = astersql_dxf_framework_proto::SetTaskCleanupBatchSizeForTest(2);
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = storage_manager
            .CreateTask(
                (),
                format!("sql-history-drain-{index}"),
                "sql-history-drain",
                "".into(),
                1,
                "".into(),
                0,
                Default::default(),
                b"original-metadata".to_vec(),
            )
            .unwrap();
        let task = storage_manager.GetTaskByID((), id).unwrap();
        storage_manager
            .SwitchTaskStep(
                (),
                task,
                astersql_dxf_framework_proto::TaskStateRunning,
                astersql_dxf_framework_proto::StepOne,
                Vec::new(),
            )
            .unwrap();
        storage_manager.SucceedTask((), id).unwrap();
        ids.push(id);
    }
    let manager = scheduler::Manager::new(
        std::sync::Arc::new(scheduler::StorageTaskManagerAdapter::new(
            storage_manager.clone(),
        )),
        ":4000",
        None,
    );
    manager.drain_cleanup_task_batches();
    restore();
    assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 3);
    assert!(storage_manager.GetCleanupTasks(()).unwrap().is_empty());
    for id in ids {
        let history = storage_manager.GetTaskByIDWithHistory((), id).unwrap();
        assert_eq!(
            history.State,
            astersql_dxf_framework_proto::TaskStateSucceed
        );
        assert_eq!(history.Meta, b"cleaned-metadata");
        assert!(storage_manager.GetTaskByID((), id).is_err());
    }
    drop(manager);
    domain.close();
}
