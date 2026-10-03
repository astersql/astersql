// Copyright 2026 AsterSQL.

use astersql_dxf_importinto as import;
use astersql_executor_importer as importer;
use astersql_executor_importer::EncodeReader;
use astersql_lightning_backend_encode as encode;
use std::sync::Arc;

fn encoded_input() -> (
    Arc<astersql_domain::Domain>,
    astersql_session::runtime::ConcreteSession,
    importer::EncodedKVGroupBatch,
    Vec<String>,
    Arc<dyn astersql_kv::Storage + Send + Sync>,
) {
    let mut config = astersql_domain::DomainConfig::default();
    config.schema_lease = std::time::Duration::ZERO;
    config.stats_lease = std::time::Duration::ZERO;
    let (domain, bound_store): (
        Arc<astersql_domain::Domain>,
        Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) = if let Ok(pd) = std::env::var("REAL_TIKV_PD") {
        let store = astersql_store_driver::TiKVDriver::default()
            .Open(&format!("tikv://{pd}?disableGC=true"))
            .expect("REAL_TIKV_PD requires a running TiKV playground");
        let bound = Arc::new(store.clone());
        (
            Arc::new(astersql_domain::Domain::new(
                store,
                Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
                config,
            )),
            bound,
        )
    } else {
        let shared = astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO();
        let owned = Arc::try_unwrap(
            astersql_store_mockstore_mockstorage::NewMockStorage(shared.clone(), None).unwrap(),
        )
        .ok()
        .unwrap();
        let bound = astersql_store_mockstore_mockstorage::NewMockStorage(shared, None).unwrap();
        (
            Arc::new(astersql_domain::Domain::new(
                owned,
                Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
                config,
            )),
            bound,
        )
    };
    domain.init().unwrap();
    let session = astersql_session::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();
    session
        .execute("drop database if exists recorded_summary")
        .unwrap();
    session.execute("create database recorded_summary").unwrap();
    session.execute("use recorded_summary").unwrap();
    session
        .execute("create table t(a bigint primary key, b varchar(100), key(b), key(a,b), key(b,a))")
        .unwrap();
    let table = domain.table_by_name("recorded_summary", "t").unwrap();
    let config = encode::EncodingConfig {
        // Go GlobalSystemVariableInitialValue initializes new installs with
        // row format V2; ImportInto forwards it through ImportantSysVars.
        SessionOptions: encode::SessionOptions {
            SysVars: [("tidb_row_format_version".into(), "2".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        Table: Some(Arc::new(
            importer::NewTableDefinitionFromMeta(&table).unwrap(),
        )),
        ..Default::default()
    };
    let mut encoder = importer::NewTableKVEncoderFromMeta(
        &config,
        &table,
        Arc::new(importer::CanonicalImportDatumConverter(
            astersql_types::StrictContext.Flags(),
        )),
    )
    .unwrap();
    let mut batch = importer::NewEncodedKVGroupBatch(&[], 10_000);
    let mut files = Vec::new();
    for file in 0..10 {
        let mut input = String::new();
        for offset in 0..1000 {
            let idx = file * 1000 + offset;
            let text = format!("test-{idx}");
            input.push_str(&format!("{idx},{text}\n"));
        }
        let parser = astersql_lightning_mydump::NewCSVParser(
            &Default::default(),
            Box::new(astersql_lightning_mydump::NewStringReader(&input)),
            false,
            None,
        )
        .unwrap();
        let mut reader = importer::parserEncodeReader(
            Box::new(parser),
            input.len() as i64,
            format!("t.{file}.csv"),
        );
        while let Some(row) = reader.ReadRow(Vec::new()).unwrap() {
            batch
                .Add(&encoder.Encode(&row.row, row.row_id).unwrap())
                .unwrap();
        }
        files.push(input);
    }
    encoder.BaseKVEncoder.SessionCtx.Close();
    (domain, session, batch, files, bound_store)
}

#[test]
fn real_encoder_matches_recorded_summary_input() {
    let (_domain, _session, batch, files, _bound_store) = encoded_input();
    assert_eq!(files.iter().map(String::len).sum::<usize>(), 147_780);
    assert_eq!(batch.data_kvs.len(), 10_000);
    assert_eq!(batch.index_kvs.len(), 3);
    let (data, indexes) = batch.group_checksum.DataAndIndexSumSize();
    assert_eq!(data + indexes, 2_622_604);
}

fn sort_groups(
    batch: &importer::EncodedKVGroupBatch,
    store: astersql_objstore_storeapi::StorageRef,
) -> import::ImportStepMeta {
    use astersql_ingestor_simplesst::writer::WriterBuilder;
    fn sorted_meta(
        pairs: &[(Vec<u8>, Vec<u8>)],
        group: &str,
        store: astersql_objstore_storeapi::StorageRef,
    ) -> import::SortedKVMeta {
        let sink = Arc::new(import::ObjectStoreWriterSink::new(
            store,
            encode::Context::default(),
        ));
        let mut builder = WriterBuilder::new();
        if astersql_config_kerneltype::IsNextGen() {
            // Transactional API V2: mode byte followed by the 24-bit keyspace ID.
            builder.set_key_prefix(vec![b'x', 0, 0, 0]);
        }
        let mut writer = builder.build_with_sink(sink, "encoded", group);
        for (key, value) in pairs {
            writer.write_row(key, value).unwrap();
        }
        let summary = writer.close().unwrap();
        import::new_sorted_kv_meta(&import::WriterSummary {
            Min: Some(summary.Min),
            Max: Some(summary.Max),
            TotalSize: summary.TotalSize,
            TotalCnt: summary.TotalCnt,
            MultipleFilesStats: summary
                .MultipleFilesStats
                .into_iter()
                .map(|stat| import::MultipleFilesStat {
                    MinKey: stat.MinKey,
                    MaxKey: stat.MaxKey,
                    Filenames: stat.Filenames,
                    MaxOverlappingNum: stat.MaxOverlappingNum,
                })
                .collect(),
            ..Default::default()
        })
    }
    import::ImportStepMeta {
        SortedDataMeta: Some(sorted_meta(
            &batch
                .data_kvs
                .iter()
                .map(|p| (p.key.clone(), p.val.clone()))
                .collect::<Vec<_>>(),
            "data",
            store.clone(),
        )),
        SortedIndexMetas: batch
            .index_kvs
            .iter()
            .map(|(id, pairs)| {
                (
                    *id,
                    sorted_meta(
                        &pairs
                            .iter()
                            .map(|p| (p.key.clone(), p.val.clone()))
                            .collect::<Vec<_>>(),
                        &id.to_string(),
                        store.clone(),
                    ),
                )
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn real_manager_failure_persists_after_explicit_node_initialization() {
    use astersql_dxf_framework_scheduler as scheduler;
    use astersql_dxf_framework_storage as storage;
    struct InitFailure(scheduler::Task);
    impl scheduler::Scheduler for InitFailure {
        fn init(&self) -> scheduler::Result<()> {
            Err(scheduler::SchedulerError::new("mock scheduler init error"))
        }
        fn schedule_once(&self) -> scheduler::Result<bool> {
            panic!("failed initialization must not schedule")
        }
        fn close(&self) {}
        fn task(&self) -> scheduler::Task {
            self.0.clone()
        }
        fn extension(&self) -> Arc<dyn scheduler::Extension> {
            panic!("failed initialization has no extension")
        }
    }
    // Independent Domain has no DXF background manager competing for these tasks.
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let table = session.ImportTaskManager().unwrap();
    storage::SetNodeResource(storage::proto::NewNodeResource(1, 0, 0));
    table.InitMeta((), ":4000".into(), "".into()).unwrap();
    let manager = scheduler::Manager::new(
        Arc::new(scheduler::StorageTaskManagerAdapter::new(table.clone())),
        ":4000",
        None,
    );
    manager.start().unwrap();
    let create = |key: &str, ty| {
        table
            .CreateTask(
                (),
                key.into(),
                ty,
                "".into(),
                1,
                "".into(),
                1,
                Default::default(),
                b"{}".to_vec(),
            )
            .unwrap()
    };
    let unknown = create("recorded-unknown", "task90-unknown");
    manager.tick().unwrap();
    let task = table.GetTaskByIDWithHistory((), unknown).unwrap();
    assert_eq!(task.State, storage::proto::TaskStateFailed);
    assert!(
        task.Error
            .unwrap()
            .to_string()
            .contains("unknown task type")
    );
    scheduler::RegisterSchedulerFactory(
        "task90-init-failure",
        Arc::new(|task, _| Arc::new(InitFailure(task))),
    );
    let failed = create("recorded-init-failure", "task90-init-failure");
    manager.tick().unwrap();
    assert_eq!(manager.scheduler_count(), 0);
    let task = table.GetTaskByIDWithHistory((), failed).unwrap();
    assert_eq!(task.State, storage::proto::TaskStateFailed);
    assert!(
        task.Error
            .unwrap()
            .to_string()
            .contains("mock scheduler init error")
    );
    manager.cleanup_finished_tasks().unwrap();
    manager.stop();
    let independent = astersql_session::runtime::ConcreteSession::new(domain.clone());
    let history = independent.ImportTaskManager().unwrap();
    for id in [unknown, failed] {
        assert_eq!(
            history.GetTaskByIDWithHistory((), id).unwrap().State,
            storage::proto::TaskStateFailed
        );
    }
    domain.close();
}

use astersql_dxf_framework_taskexecutor as node;
use astersql_ingestor_engineapi as engineapi;
use astersql_ingestor_ingestctrl as local;
use import::write_ingest_backend::{RegionImportTransport, RegisterGlobalSortImportExecutor};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct RegionRpc {
    domain: Arc<astersql_domain::Domain>,
    pending: Mutex<std::collections::HashMap<Vec<u8>, Vec<(Vec<u8>, Vec<u8>)>>>,
    retry_data_key: Vec<u8>,
    retry: AtomicBool,
    writes: AtomicUsize,
}
impl RegionImportTransport for RegionRpc {
    fn Scan(
        &self,
        token: &local::CancellationToken,
        range: &local::KeyRange,
    ) -> local::Result<Vec<local::region_job::LocatedRegion>> {
        token.check()?;
        Ok(vec![local::region_job::LocatedRegion {
            region: local::job_worker::RegionInfo {
                id: 1,
                leader_store_id: 1,
                peer_store_ids: vec![1],
            },
            key_range: range.clone(),
        }])
    }
    fn Write(
        &self,
        token: &local::CancellationToken,
        job: &local::job_worker::RegionJob,
        data: &dyn engineapi::IngestData,
    ) -> local::Result<local::job_worker::TikvWriteResult> {
        token.check()?;
        let mut pool = local::local::membuf::NewPool(Vec::new());
        let mut iter = data.NewIter(
            &engineapi::Context::background(),
            &job.key_range.start,
            &job.key_range.end,
            Arc::get_mut(&mut pool).unwrap(),
        );
        let mut pairs = Vec::new();
        let mut bytes = 0;
        let mut valid = iter.First();
        while valid {
            let key = iter.Key().to_vec();
            let value = iter.Value().to_vec();
            bytes += (key.len() + value.len()) as i64;
            let key = if astersql_config_kerneltype::IsNextGen() {
                key.strip_prefix(&[b'x', 0, 0, 0])
                    .expect("API V2 keyspace prefix")
                    .to_vec()
            } else {
                key
            };
            pairs.push((key, value));
            valid = iter.Next();
        }
        let error = iter.Error().map(|e| e.to_string());
        iter.Close()
            .map_err(|e| local::Error::InvalidData(e.to_string()))?;
        if let Some(error) = error {
            return Err(local::Error::InvalidData(error));
        }
        let count = pairs.len() as i64;
        self.pending
            .lock()
            .unwrap()
            .insert(job.key_range.start.clone(), pairs);
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(local::job_worker::TikvWriteResult {
            total_bytes: bytes,
            count,
            ..Default::default()
        })
    }
    fn Ingest(
        &self,
        token: &local::CancellationToken,
        job: &local::job_worker::RegionJob,
    ) -> local::Result<()> {
        token.check()?;
        if job.key_range.start <= self.retry_data_key
            && self.retry_data_key < job.key_range.end
            && self.retry.swap(false, Ordering::SeqCst)
        {
            return Err(local::Error::Retryable("KVIngestFailed".into()));
        }
        let pairs = self
            .pending
            .lock()
            .unwrap()
            .remove(&job.key_range.start)
            .unwrap();
        let physical = self
            .domain
            .storage()
            .with_storage(|store| store.ImportSST(job.timestamp, pairs))
            .map_err(|e| local::Error::InvalidData(e.to_string()))?;
        if std::env::var_os("REAL_TIKV_PD").is_some() {
            assert!(
                physical.write_rpcs > 0 && physical.ingest_rpcs > 0,
                "actual TiKV SST RPCs: {physical:?}"
            );
        }
        Ok(())
    }
    fn Close(&self) {}
}

struct NativeHost(Arc<dyn astersql_kv::Storage + Send + Sync>);
impl import::ImportStepHost for NativeHost {
    fn TaskStore(&self) -> Arc<dyn astersql_kv::Storage + Send + Sync> {
        self.0.clone()
    }
    fn NewWriteIngestBackend(
        &self,
        _: &astersql_dxf_framework_proto::Task,
        _: &import::TaskMeta,
        _: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<Arc<dyn import::WriteIngestBackend>, astersql_kv::errors::SharedError> {
        unreachable!("native global sort host supplies this backend")
    }
    fn NewStepExecutor(
        &self,
        _: i64,
        _: &astersql_dxf_framework_proto::Task,
        _: &import::TaskMeta,
        _: Arc<dyn astersql_kv::Storage + Send + Sync>,
    ) -> Result<
        Box<dyn astersql_dxf_framework_taskexecutor_execute::StepExecutor>,
        astersql_kv::errors::SharedError,
    > {
        unreachable!("only merge and ingest stages are requested")
    }
}

struct RetryPolicy;
impl node::Extension for RetryPolicy {
    fn IsIdempotent(&self, _: &node::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(&self, _: &node::Task) -> node::Result<Arc<dyn node::StepExecutor>> {
        Err(node::ExecutorError(
            "import factory must install its extension".into(),
        ))
    }
    fn IsRetryableError(&self, _: &node::ExecutorError) -> bool {
        false
    }
}

fn drive_registered_step(
    session: &astersql_session::runtime::ConcreteSession,
    manager: &astersql_dxf_framework_storage::TaskManager,
    id: i64,
    step: i64,
) {
    let context = node::Context::Background();
    let table = session.ImportNodeTaskTable().unwrap();
    let task = table.GetTaskByID(&context, id).unwrap();
    let param = node::NewParamForTest(
        table,
        Arc::new(node::newSlotManager(1)),
        node::NodeResource {
            TotalCPU: 1,
            TotalMem: 128 * 1024 * 1024,
            TotalDisk: 0,
        },
        ":4000",
        Arc::new(RetryPolicy),
    );
    let executor =
        node::GetTaskExecutorFactory("ImportInto").unwrap()(context.clone(), task, param);
    executor.Init(&context).unwrap();
    let running = executor.clone();
    let worker = std::thread::spawn(move || running.Run());
    let started = std::time::Instant::now();
    let complete = loop {
        let rows = manager
            .GetSubtasksWithHistory((), id, step)
            .unwrap()
            .unwrap();
        if rows
            .iter()
            .all(|row| row.State == astersql_dxf_framework_storage::proto::SubtaskStateSucceed)
        {
            break true;
        }
        if rows
            .iter()
            .any(|row| row.State == astersql_dxf_framework_storage::proto::SubtaskStateFailed)
        {
            break false;
        }
        if started.elapsed() > std::time::Duration::from_secs(60) {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    executor.Cancel();
    worker.join().unwrap();
    executor.Close();
    if !complete {
        eprintln!(
            "subtasks: {:?}",
            manager.GetSubtasksWithHistory((), id, step).unwrap()
        );
        eprintln!(
            "SQL errors: {:?}",
            sql_rows(
                session,
                "select error from mysql.tidb_background_subtask where state = 'failed'"
            )
        );
    }
    assert!(
        complete,
        "step {step} did not finish through registered import executor"
    );
}

fn cloud_store(sdk: Arc<dyn object_store::ObjectStore>) -> astersql_objstore_storeapi::StorageRef {
    Arc::new(
        astersql_objstore::gcs::GCSStorage::with_store(
            astersql_objstore::gcs::GCSConfig {
                bucket: "task90".into(),
                ..Default::default()
            },
            sdk,
            Some(Arc::new(astersql_objstore::gcs::AccessRecorder::default())),
        )
        .unwrap(),
    )
}

fn install_stage(
    manager: &astersql_dxf_framework_storage::TaskManager,
    id: i64,
    step: i64,
    meta: &import::TaskMeta,
    metas: Vec<Vec<u8>>,
) {
    use astersql_dxf_framework_storage::proto;
    let mut task = manager.GetTaskByID((), id).unwrap();
    task.Meta = meta.Marshal().unwrap();
    manager
        .SwitchTaskStep(
            (),
            task,
            proto::TaskStateRunning,
            step,
            metas
                .into_iter()
                .enumerate()
                .map(|(ordinal, meta)| proto::Subtask {
                    SubtaskBase: proto::SubtaskBase {
                        TaskID: id,
                        Step: step,
                        Type: proto::ImportInto,
                        ExecID: ":4000".into(),
                        Ordinal: (ordinal + 1) as i32,
                        Concurrency: 1,
                        ..Default::default()
                    },
                    Meta: meta,
                    ..Default::default()
                })
                .collect(),
        )
        .unwrap();
}

fn sql_rows(session: &astersql_session::runtime::ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    use astersql_session::testutil::TestRecordSet;
    let mut sets = session.execute(sql).unwrap();
    let mut set = sets.pop().unwrap();
    let mut rows = Vec::new();
    while let Some(row) = set.Next().unwrap() {
        rows.push(row);
    }
    rows
}

pub fn run_recorded_step_summary() {
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_storage as storage;
    use astersql_objstore_storeapi::Storage;
    let (domain, session, batch, files, bound_store) = encoded_input();
    let table = domain.table_by_name("recorded_summary", "t").unwrap();
    let source_bytes = files.iter().map(String::len).sum::<usize>();
    assert_eq!(source_bytes, 147_780);
    let directory = std::env::temp_dir().join(format!(
        "astersql-task90-sort-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    struct SortFiles(std::path::PathBuf);
    impl Drop for SortFiles {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _files = SortFiles(directory.clone());
    // Both APIs read the same real objects. The SDK filesystem replaces only cloud transport.
    let sdk: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new_with_prefix(&directory).unwrap());
    let planner_store: astersql_objstore::storage::StorageRef =
        Arc::new(astersql_objstore::local::NewLocalStorage(&directory).unwrap());
    let store = cloud_store(sdk.clone());
    let encoded = sort_groups(&batch, store.clone());
    store
        .WriteFile(
            &Default::default(),
            "encoded/meta.json",
            &encoded.Marshal().unwrap(),
        )
        .unwrap();
    // Planning consumes the durable encoder result, including all four actual KV groups.
    let encoded = import::ImportStepMeta::Unmarshal(
        &store
            .ReadFile(&Default::default(), "encoded/meta.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(encoded.SortedDataMeta.as_ref().unwrap().TotalKVCnt, 10_000);
    assert_eq!(store.AccessRequestSnapshot(), Some((1, 9)));
    let expected_logical_bytes = if astersql_config_kerneltype::IsNextGen() {
        2_782_604
    } else {
        2_622_604
    };
    let mut plan = import::LogicalPlan::default();
    plan.Plan = importer::Plan {
        DBName: "recorded_summary".into(),
        TableInfo: Some(table),
        ThreadCnt: 1,
        CloudStorageURI: "gs://task90/sorted".into(),
        ForceMergeStep: true,
        ..Default::default()
    };
    let manager = session.ImportTaskManager().unwrap();
    storage::SetNodeResource(storage::proto::NewNodeResource(1, 128 * 1024 * 1024, 0));
    manager.InitMeta((), ":4000".into(), "".into()).unwrap();
    let mut task_meta = import::TaskMeta {
        Plan: plan.Plan.clone(),
        ..Default::default()
    };
    let id = manager
        .CreateTask(
            (),
            "recorded-native-import".into(),
            storage::proto::ImportInto,
            "".into(),
            1,
            "".into(),
            1,
            Default::default(),
            task_meta.Marshal().unwrap(),
        )
        .unwrap();
    let transport = Arc::new(RegionRpc {
        domain: domain.clone(),
        pending: Default::default(),
        retry_data_key: encoded.SortedDataMeta.as_ref().unwrap().StartKey.clone(),
        retry: AtomicBool::new(true),
        writes: AtomicUsize::new(0),
    });
    let runtime = Arc::new(import::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| {
            panic!("CSV preparation does not construct an importer controller")
        }),
        ImporterService: Arc::new(|| panic!("merge/ingest do not construct a local importer")),
        SharedImporterService: Default::default(),
        ObjectStore: store.clone(),
        ObjectStoreFactory: Some(Arc::new(move || Ok(cloud_store(sdk.clone())))),
        LoggerFactory: Arc::new(|| panic!("merge/ingest have no encode logger")),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    RegisterGlobalSortImportExecutor(
        runtime,
        Arc::new(NativeHost(bound_store)),
        transport.clone(),
        Arc::new(RetryPolicy),
    );
    let mut ctx = import::PlanCtx {
        TaskID: id,
        ThreadCnt: 1,
        GlobalSort: true,
        NextTaskStep: proto::ImportStepMergeSort,
        ExecuteNodesCnt: 1,
        PreviousImportMetas: vec![encoded],
        ObjectStore: Some(planner_store),
        CommitTS: Some(
            domain
                .storage()
                .with_storage(|store| store.CurrentVersion("global"))
                .unwrap()
                .Ver,
        ),
        NodeMemPerCore: 128 * 1024 * 1024,
        ..Default::default()
    };
    let merge = plan.ToPhysicalPlan(ctx.clone()).unwrap();
    let metas = merge
        .ToSubtaskMetas(&ctx, proto::ImportStepMergeSort)
        .unwrap();
    assert_eq!(metas.len(), 4);
    install_stage(&manager, id, proto::ImportStepMergeSort, &task_meta, metas);
    drive_registered_step(&session, &manager, id, proto::ImportStepMergeSort);
    ctx.PreviousSubtaskMetas.insert(
        proto::ImportStepMergeSort,
        manager
            .GetSubtasksWithHistory((), id, proto::ImportStepMergeSort)
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|row| row.Meta)
            .collect(),
    );
    ctx.NextTaskStep = proto::ImportStepWriteAndIngest;
    plan.summary = Default::default();
    let ingest = plan.ToPhysicalPlan(ctx.clone()).unwrap();
    assert_eq!(plan.summary.RowCnt, 10_000);
    assert_eq!(plan.summary.Bytes, expected_logical_bytes);
    let mut logical_task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: id,
            Key: "recorded-native-import".into(),
            Type: proto::ImportInto,
            State: proto::TaskStateRunning,
            Step: proto::ImportStepWriteAndIngest,
            Priority: 1,
            RequiredSlots: 1,
            TargetScope: String::new(),
            CreateTime: std::time::UNIX_EPOCH,
            MaxNodeCount: 1,
            ExtraParams: Default::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: std::time::UNIX_EPOCH,
        StateUpdateTime: std::time::UNIX_EPOCH,
        Meta: task_meta.Marshal().unwrap(),
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: proto::TaskStateRunning,
            Modifications: Vec::new(),
        },
    };
    import::updateTaskSummary(
        &mut logical_task,
        &mut task_meta,
        proto::ImportStepWriteAndIngest,
        &plan.summary,
        None,
    )
    .unwrap();
    let metas = ingest
        .ToSubtaskMetas(&ctx, proto::ImportStepWriteAndIngest)
        .unwrap();
    assert_eq!(metas.len(), 4);
    install_stage(
        &manager,
        id,
        proto::ImportStepWriteAndIngest,
        &task_meta,
        metas,
    );
    drive_registered_step(&session, &manager, id, proto::ImportStepWriteAndIngest);
    assert!(
        transport.writes.load(Ordering::SeqCst) > 4,
        "physical data region was actually rewritten"
    );
    manager.SucceedTask((), id).unwrap();
    manager
        .TransferTasks2History((), vec![manager.GetTaskByID((), id).unwrap()])
        .unwrap();
    let independent = astersql_session::runtime::ConcreteSession::new(domain.clone());
    let history = independent.ImportTaskManager().unwrap();
    assert!(history.GetTaskByID((), id).is_err());
    let task = history.GetTaskByIDWithHistory((), id).unwrap();
    assert_eq!(task.State, storage::proto::TaskStateSucceed);
    let meta = import::TaskMeta::Unmarshal(&task.Meta).unwrap();
    assert_eq!(meta.Summary.IngestSummary.RowCnt, 10_000);
    assert_eq!(meta.Summary.IngestSummary.Bytes, expected_logical_bytes);
    let summaries = sql_rows(
        &independent,
        &format!(
            "select summary from mysql.tidb_background_subtask_history where task_key = '{}' and step = {}",
            storage::TaskIDToKey(id),
            proto::ImportStepWriteAndIngest
        ),
    );
    assert_eq!(summaries.len(), 4);
    let total = summaries
        .into_iter()
        .map(|row| serde_json::from_str::<serde_json::Value>(&row[0]).unwrap())
        .fold((0, 0, 0, 0), |(rows, bytes, get, put), value| {
            (
                rows + value["row_count"].as_i64().unwrap(),
                bytes + value["bytes"].as_i64().unwrap(),
                get + value["get_request_count"].as_u64().unwrap(),
                put + value["put_request_count"].as_u64().unwrap(),
            )
        });
    assert!(total.0 >= 10_000);
    assert!(total.1 >= expected_logical_bytes);
    assert_eq!((total.2, total.3), (20, 0));
    let merge_summaries = sql_rows(
        &independent,
        &format!(
            "select summary from mysql.tidb_background_subtask_history where task_key = '{}' and step = {}",
            storage::TaskIDToKey(id),
            proto::ImportStepMergeSort
        ),
    );
    assert_eq!(merge_summaries.len(), 4);
    let requests = merge_summaries
        .iter()
        .map(|row| serde_json::from_str::<serde_json::Value>(&row[0]).unwrap())
        .fold((0, 0), |(get, put), value| {
            (
                get + value["get_request_count"].as_u64().unwrap(),
                put + value["put_request_count"].as_u64().unwrap(),
            )
        });
    assert_eq!(requests, (12, 12));
    let rows = sql_rows(&session, "select a,b from t order by a");
    assert_eq!(rows.len(), 10_000);
    for (idx, row) in rows.iter().enumerate() {
        assert_eq!(row, &vec![idx.to_string(), format!("test-{idx}")]);
    }
    session.execute("admin check table t").unwrap();
    assert!(transport.pending.lock().unwrap().is_empty());
    domain.close();
}
