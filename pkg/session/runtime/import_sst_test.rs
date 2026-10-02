// Copyright 2026 AsterSQL.

use super::*;
use astersql_dxf_framework_taskexecutor::{Context, StepExecutor};

pub(super) fn domain() -> Arc<Domain> {
    let store = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let store = Arc::try_unwrap(store).ok().unwrap();
    let domain = Arc::new(Domain::new(
        store,
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        Default::default(),
    ));
    domain.init().unwrap();
    domain
}
fn job(speed: i64) -> astersql_meta_model::group_3::Job {
    let job = astersql_meta_model::group_3::Job {
        id: 960610,
        reorg_meta: Some(Default::default()),
        ..Default::default()
    };
    job.reorg_meta.as_ref().unwrap().SetBatchSize(1);
    job.reorg_meta.as_ref().unwrap().SetMaxWriteSpeed(speed);
    job
}

#[test]
fn read_index_task_meta_updates_live_physical_import_limiter() {
    let domain = domain();
    let step = modify_column_dist_backfill::ReadIndex::new(
        domain.clone(),
        astersql_dxf_framework_storage::TaskManager::new(),
        job(1),
        vec![],
        false,
        String::new(),
    );
    let context = Context::Background();
    // Controls are already in use when the framework delivers a new task Meta.
    // Storage mode belongs to the initialized executor, so a changed URI in
    // incoming metadata cannot disable local write-speed updates.
    let control = step.import_control(&context);
    let mut next = job(0);
    next.reorg_meta.as_ref().unwrap().SetBatchSize(2048);
    let job: serde_json::Value = serde_json::from_slice(&next.encode(false).unwrap()).unwrap();
    let meta = serde_json::to_vec(&serde_json::json!({"job": job, "ele_ids": [], "ele_type_key": "", "cloud_storage_uri": "s3://changed-task-meta", "estimate_row_size": 0, "merge_temp_index": false, "version": 1})).unwrap();
    step.TaskMetaModified(&context, &meta).unwrap();
    let options = control.options.clone();
    let target = domain.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        done_tx
            .send(modify_column_backfill::ingest_with_options(
                target,
                960610,
                vec![astersql_lightning_verification::KvPair {
                    key: b"live-rate-control".to_vec(),
                    val: vec![7; 4096],
                }],
                options,
            ))
            .unwrap();
    });
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        drop(control);
        worker.join().unwrap();
        panic!("updated limit=0 must apply to physical import using existing controls");
    }
    result.unwrap().unwrap();
    worker.join().unwrap();
    let value = domain
        .storage()
        .with_storage(|store| {
            store.GetSnapshot(kv::MaxVersion).Get(
                &kv::Context::todo(),
                kv::Key(b"live-rate-control".to_vec()),
                &[],
            )
        })
        .unwrap();
    assert_eq!(value.Value, vec![7; 4096]);
    domain.close();
}

#[test]
fn read_index_cancel_stops_existing_physical_write_limiter() {
    let domain = domain();
    let step = modify_column_dist_backfill::ReadIndex::new(
        domain.clone(),
        astersql_dxf_framework_storage::TaskManager::new(),
        job(1),
        vec![],
        false,
        String::new(),
    );
    let context = Context::Background();
    let control = step.import_control(&context);
    let options = control.options.clone();
    let cancel = context.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        // Consume the single-byte burst before the cancellable long wait.
        let limiter = options.write_limiter.unwrap();
        limiter.WaitN(&options.context, 1, 1).unwrap();
        started_tx.send(()).unwrap();
        done_tx
            .send(limiter.WaitN(&options.context, 1, 4096))
            .unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.Cancel();
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        let native_cancelled = control.options.context.is_cancelled();
        let framework_cancelled = context.Done();
        drop(control);
        worker.join().unwrap();
        panic!(
            "framework cancellation must wake the actual store write limiter: framework={framework_cancelled}, kv={native_cancelled}"
        );
    }
    assert!(result.unwrap().is_err());
    worker.join().unwrap();
    domain.close();
}

#[test]
fn cloud_external_backend_imports_native_keyspace_files_at_subtask_timestamp() {
    use astersql_ingestor_globalsort as sort;
    use sort::Storage;
    let domain = domain();
    let prefix = vec![b'r', 0, 0, 9];
    let raw = vec![
        (b"cloud-index-a".to_vec(), vec![1]),
        (b"cloud-index-b".to_vec(), vec![2]),
        (b"cloud-index-c".to_vec(), vec![3]),
    ];
    let store = Arc::new(NativeMemory(Default::default()));
    let mut data = Vec::new();
    let mut boundaries = Vec::new();
    for (key, value) in &raw {
        let key = [prefix.as_slice(), key].concat();
        boundaries.push(key.clone());
        data.extend_from_slice(&(key.len() as u64).to_be_bytes());
        data.extend_from_slice(&(value.len() as u64).to_be_bytes());
        data.extend_from_slice(&key);
        data.extend_from_slice(value);
    }
    let size = raw
        .iter()
        .map(|(key, value)| prefix.len() + key.len() + value.len())
        .sum::<usize>();
    store.write("data", data).unwrap();
    store.write("stat", Vec::new()).unwrap();
    let end = [prefix.as_slice(), b"cloud-index-d"].concat();
    boundaries.push(end.clone());
    let ts = domain
        .storage()
        .with_storage(|store| store.CurrentVersion(kv::GlobalTxnScope))
        .unwrap()
        .Ver;
    let first_context = kv::Context::new();
    let backend = import_sst::Backend::new_with_key_prefix(
        domain.clone(),
        960610,
        kv::SSTImportOptions {
            context: first_context.clone(),
            ..Default::default()
        },
        prefix.clone(),
        2,
    )
    .unwrap();
    let new_engine = || {
        sort::engine::NewExternalEngine(
            store.clone(),
            vec!["data".into()],
            vec!["stat".into()],
            boundaries[0].clone(),
            end.clone(),
            boundaries.clone(),
            vec![],
            2,
            ts,
            size as i64,
            0,
            false,
            1024 * 1024,
            sort::OnDuplicateKey::Error,
            "cloud-test".into(),
        )
        .unwrap()
    };
    let id = uuid::Uuid::new_v4();
    let source = backend
        .register_external(id, new_engine(), Default::default())
        .unwrap();
    let imported = backend.import_external(&Default::default(), id);
    imported.unwrap();
    assert_eq!(
        astersql_ingestor_ingestctrl::local::engineapi::Engine::ImportedStatistics(source.as_ref()),
        (size as i64, 3)
    );
    assert_eq!(source.GetTotalLoadedKVsCount(), 3);
    let snapshot = domain
        .storage()
        .with_storage(|store| store.GetSnapshot(kv::Version { Ver: ts }));
    for (key, value) in &raw {
        assert_eq!(
            snapshot
                .Get(&kv::Context::todo(), kv::Key(key.clone()), &[])
                .unwrap()
                .Value,
            *value
        );
        assert!(
            snapshot
                .Get(
                    &kv::Context::todo(),
                    kv::Key([prefix.as_slice(), key].concat()),
                    &[]
                )
                .is_err(),
            "cloud keyspace prefix must not be imported twice"
        );
    }
    backend.cleanup_external(id).unwrap();
    // A step owns one backend while subtask contexts have shorter lifetimes.
    // Completing the first subtask cancels its controls; the next registration
    // must use fresh options without retaining that cancelled context.
    first_context.cancel();
    backend.set_import_options(Default::default()).unwrap();
    let source = backend
        .register_external(id, new_engine(), Default::default())
        .unwrap();
    backend.import_external(&Default::default(), id).unwrap();
    assert_eq!(source.GetTotalLoadedKVsCount(), 3);
    assert_eq!(
        astersql_ingestor_ingestctrl::local::engineapi::Engine::ImportedStatistics(source.as_ref()),
        (size as i64, 3)
    );
    backend.cleanup_external(id).unwrap();
    drop(backend);
    domain.close();
}

use astersql_ingestor_globalsort as sort;
#[test]
fn cloud_external_backend_preserves_duplicate_key_and_value_error() {
    use sort::Storage;
    let domain = domain();
    let store = Arc::new(NativeMemory(Default::default()));
    let mut bytes = Vec::new();
    for value in [b"1", b"2"] {
        bytes.extend_from_slice(&1_u64.to_be_bytes());
        bytes.extend_from_slice(&1_u64.to_be_bytes());
        bytes.extend_from_slice(b"a");
        bytes.extend_from_slice(value);
    }
    store.write("data", bytes).unwrap();
    store.write("stat", Vec::new()).unwrap();
    let engine = sort::engine::NewExternalEngine(
        store,
        vec!["data".into()],
        vec!["stat".into()],
        b"a".to_vec(),
        b"b".to_vec(),
        vec![b"a".to_vec(), b"b".to_vec()],
        vec![],
        1,
        100,
        4,
        0,
        true,
        1024,
        sort::OnDuplicateKey::Error,
        "duplicate-test".into(),
    )
    .unwrap();
    let backend = import_sst::Backend::new(domain.clone(), 960611).unwrap();
    let id = uuid::Uuid::new_v4();
    backend
        .register_external(id, engine, Default::default())
        .unwrap();
    let result = backend.import_external_native(&Default::default(), id);
    backend.cleanup_external(id).unwrap();
    drop(backend);
    domain.close();
    assert!(
        matches!(result, Err(astersql_ingestor_ingestctrl::Error::Conflict { key, value })
        if key == b"a" && value == b"2"),
        "preserve the actual conflict payload for Go error conversion"
    );
}
struct NativeMemory(sort::MemoryStorage);
impl sort::Storage for NativeMemory {
    fn read(&self, path: &str) -> sort::Result<Vec<u8>> {
        self.0.read(path)
    }
    fn write(&self, path: &str, data: Vec<u8>) -> sort::Result<()> {
        self.0.write(path, data)
    }
    fn list_prefix(&self, prefix: &str) -> sort::Result<Vec<String>> {
        self.0.list_prefix(prefix)
    }
    fn delete_files(&self, paths: &[String]) -> sort::Result<()> {
        self.0.delete_files(paths)
    }
    fn record_format(&self) -> sort::RecordFormat {
        sort::RecordFormat::GoBigEndian64
    }
}
