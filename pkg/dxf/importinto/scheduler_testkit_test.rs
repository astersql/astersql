// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Import Into 调度器 testkit 场景测试。
// 可执行测试覆盖任务元数据序列化、调度阶段和取消处理。

use crate::{TaskMeta, getStepOfEncode};
use astersql_dxf_framework_proto::ImportStepEncodeAndSort;

/// 分发子任务前 TaskMeta 必须能 Marshal/Unmarshal，且 global sort 编码步为 EncodeAndSort。
#[test]
fn scheduler_persists_task_meta_before_distributing_subtasks() {
    let meta = TaskMeta::default();
    let bytes = meta.Marshal().unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(getStepOfEncode(true), ImportStepEncodeAndSort);
    let decoded = TaskMeta::Unmarshal(&bytes).unwrap();
    assert_eq!(decoded.Stmt, meta.Stmt);
}

/// Go TestSchedulerExtLocalSort and TestSchedulerOnDoneCancelResetsTableMode
/// both pass the literal worker error "cancelled by user" to OnDone.
#[test]
fn scheduler_on_done_treats_go_cancel_error_as_cancellation() {
    use crate::scheduler::{
        ImportSchedulerRuntime, TaskRegistration, doneImportTask, importScheduler,
        resetClassicTableMode,
    };
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
    use astersql_meta_model as model;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    struct Runtime;
    impl ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _: i64,
            _: Duration,
        ) -> Result<Box<dyn TaskRegistration>, SharedError> {
            Err(New("unused"))
        }
        fn switch_to_import_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
        fn switch_to_normal_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
    }
    struct Backend(Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<&'static str>>>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push(sql.to_owned());
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
        fn alter_table_mode_for_import(&self, _: i64, _: i64) -> Result<(), storage::Error> {
            self.1.lock().unwrap().push("import");
            Ok(())
        }
        fn alter_table_mode_for_normal(&self, _: i64, _: i64) -> Result<(), storage::Error> {
            self.1.lock().unwrap().push("normal");
            Ok(())
        }
    }

    let statements = Arc::new(Mutex::new(Vec::new()));
    let modes = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let statements = statements.clone();
        let modes = modes.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(statements.clone(), modes.clone()),
            )))
        }
    }));
    let task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: 41,
            Key: String::new(),
            Type: proto::ImportInto,
            State: proto::TaskStateReverting,
            Step: proto::StepInit,
            Priority: proto::NormalPriority,
            RequiredSlots: 1,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: proto::ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: TaskMeta {
            JobID: 41,
            Plan: astersql_executor_importer::Plan {
                DBID: 7,
                TableInfo: Some(Arc::new(model::TableInfo {
                    ID: 8,
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        }
        .Marshal()
        .unwrap(),
        Error: Some("cancelled by user".into()),
        ModifyParam: proto::ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    };
    let scheduler = importScheduler::new(Arc::new(Runtime), &task).unwrap();
    doneImportTask(
        &astersql_dxf_framework_scheduler::Context::default(),
        &scheduler,
        &manager,
        &task,
        &|_, _| unreachable!(),
    )
    .unwrap();
    let statements = statements.lock().unwrap();
    assert!(
        statements
            .iter()
            .any(|sql| sql.contains("error_message = 'cancelled by user'")),
        "cancel SQL missing: {statements:?}"
    );
    assert!(
        !statements
            .iter()
            .any(|sql| sql.contains("end_time = CURRENT_TIMESTAMP(6)")),
        "failure SQL used for cancellation: {statements:?}"
    );
    drop(statements);
    // Exercise the classic branch directly because the test dependency graph
    // enables nextgen even for this crate's default cargo test invocation.
    modes.lock().unwrap().clear();
    resetClassicTableMode(&manager, &TaskMeta::Unmarshal(&task.Meta).unwrap());
    assert_eq!(*modes.lock().unwrap(), ["normal"]);
    scheduler.Close(task.ID);
}

#[test]
fn scheduler_local_sort_runs_go_import_validate_done_and_revert_phases() {
    use crate::scheduler::{
        ImportSchedulerRuntime, ImportSchedulerServices, TaskRegistration, doneImportTask,
        importScheduler, nextImportSubtasksBatch,
    };
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_scheduler as framework;
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
    use astersql_executor_importer as importer;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    struct Runtime;
    impl ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _: i64,
            _: Duration,
        ) -> Result<Box<dyn TaskRegistration>, SharedError> {
            Err(New("unused"))
        }
        fn switch_to_import_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
        fn switch_to_normal_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
    }
    struct Backend(
        Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>,
        storage::sessionctx::Context,
    );
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push((sql.to_owned(), args.clone()));
            let rows = self.1.GetSQLExecutor().execute(sql.into(), args)?;
            Ok(storage::SQLResult {
                rows,
                affected_rows: self.1.GetSessionVars().StmtCtx.AffectedRows(),
            })
        }
    }
    struct Handle(Mutex<HashMap<proto::Step, Vec<Vec<u8>>>>);
    impl framework::TaskHandle for Handle {
        fn previous_subtask_metas(
            &self,
            _: i64,
            step: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&step)
                .cloned()
                .unwrap_or_default())
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            Ok(vec![])
        }
    }
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let real_manager = session.ImportTaskManager().unwrap();
    real_manager.ExecuteSQLWithNewSession((),
        "INSERT INTO mysql.tidb_import_jobs(id,table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step) VALUES(%?,%?,%?,%?,%?,%?,%?,%?,%?)",
        vec![41_i64.into(), "test".into(), "t".into(), 8_i64.into(), "root@%".into(), r#"{"format":"csv","file-location":"gs://test-load/1.csv"}"#.into(), 2_i64.into(), "pending".into(), "".into()],
    ).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let calls = calls.clone();
        move || {
            let mut real_session = None;
            real_manager.WithNewSession(|session| {
                real_session = Some(session);
                Ok(())
            })?;
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(calls.clone(), real_session.unwrap()),
            )))
        }
    }));
    let meta = TaskMeta {
        JobID: 41,
        Plan: importer::Plan {
            DBName: "test".into(),
            TotalFileSize: 2,
            ..Default::default()
        },
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "gs://test-load/1.csv".into(),
                FileSize: 2,
                EndOffset: 2,
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    let mut task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: 41,
            Key: String::new(),
            Type: proto::ImportInto,
            State: proto::TaskStatePending,
            Step: proto::StepInit,
            Priority: proto::NormalPriority,
            RequiredSlots: 1,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: proto::ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: meta.Marshal().unwrap(),
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    };
    let scheduler = importScheduler::new(Arc::new(Runtime), &task).unwrap();
    let services = ImportSchedulerServices {
        ControllerServices: Arc::new(|| unreachable!()),
        ResourceCalculatorWithContext: None,
        ImporterService: Arc::new(|| unreachable!()),
        KVCodec: vec![],
        Table: None,
        SortStore: None,
        CheckImportTableEmpty: Some(Arc::new(|_| Ok(()))),
        PlanContext: Arc::new(|_, _| crate::planner::PlanCtx::default()),
        FlushStatsBestEffort: Some(Arc::new(|_, _| Ok(()))),
    };
    let handle = Handle(Mutex::new(HashMap::new()));
    let context = framework::Context::default();
    let import = scheduler.GetNextStep(&task.TaskBase);
    assert_eq!(import, proto::ImportStepImport);
    let metas = nextImportSubtasksBatch(
        &context,
        &scheduler,
        &manager,
        &handle,
        &mut task,
        &[":4000".into()],
        import,
        &services,
    )
    .unwrap();
    assert_eq!(metas.len(), 1);
    handle
        .0
        .lock()
        .unwrap()
        .insert(import, vec![br#"{}"#.to_vec()]);
    task.Step = import;
    let validating = scheduler.GetNextStep(&task.TaskBase);
    assert_eq!(validating, proto::ImportStepPostProcess);
    let metas = nextImportSubtasksBatch(
        &context,
        &scheduler,
        &manager,
        &handle,
        &mut task,
        &[":4000".into()],
        validating,
        &services,
    )
    .unwrap();
    assert_eq!(metas.len(), 1);
    task.Step = validating;
    assert_eq!(scheduler.GetNextStep(&task.TaskBase), proto::StepDone);
    assert!(
        nextImportSubtasksBatch(
            &context,
            &scheduler,
            &manager,
            &handle,
            &mut task,
            &[":4000".into()],
            proto::StepDone,
            &services
        )
        .unwrap()
        .is_empty()
    );
    task.State = proto::TaskStateSucceed;
    doneImportTask(&context, &scheduler, &manager, &task, &|_, _| Ok(())).unwrap();
    {
        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(
            |(sql, args)| sql.contains("start_time = CURRENT_TIMESTAMP(6)")
                && args[1] == storage::Value::String(importer::JobStepImporting.into())
        ));
        assert!(calls.iter().any(|(sql, args)| {
            sql.contains("SET update_time = CURRENT_TIMESTAMP(6), step = %?")
                && args[0] == storage::Value::String(importer::JobStepValidating.into())
        }));
        assert!(calls.iter().any(
            |(sql, args)| sql.contains("end_time = CURRENT_TIMESTAMP(6)")
                && args[0] == storage::Value::String(importer::JobStatusFinished.into())
        ));
    }
    task.Step = proto::StepInit;
    task.State = proto::TaskStateReverting;
    task.Error = Some("precheck failed".into());
    doneImportTask(
        &context,
        &scheduler,
        &manager,
        &task,
        &|_, _| unreachable!(),
    )
    .unwrap();
    let recorded = calls.lock().unwrap();
    assert!(
        recorded
            .iter()
            .any(|(sql, args)| sql.contains("status IN (%?, %?)")
                && args[0] == storage::Value::String("failed".into())
                && args[1] == storage::Value::String("precheck failed".into()))
    );
    drop(recorded);
    scheduler.Close(task.ID);

    // Go prepare-mode job has already started in OnPrepare. The first
    // business phase updates only its step, preserving the original start_time.
    manager.ExecuteSQLWithNewSession((),
        "INSERT INTO mysql.tidb_import_jobs(id,table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step) VALUES(%?,%?,%?,%?,%?,%?,%?,%?,%?)",
        vec![42_i64.into(), "test".into(), "t".into(), 8_i64.into(), "root@%".into(), r#"{"format":"csv","file-location":"gs://test-load/2.csv"}"#.into(), 2_i64.into(), "pending".into(), "".into()],
    ).unwrap();
    crate::scheduler::withImportJobSession(&manager, |executor| {
        importer::StartJob(executor, 42, importer::JobStepPreparing)
    })
    .unwrap();
    let mut prepared_meta = meta;
    prepared_meta.JobID = 42;
    prepared_meta.Plan.CloudStorageURI = "memstore://scheduler-prepared-testkit".into();
    prepared_meta.ChunkMap.insert(
        2,
        vec![importer::Chunk {
            Path: "gs://test-load/2.csv".into(),
            FileSize: 2,
            EndOffset: 2,
            ..Default::default()
        }],
    );
    task.ID = 42;
    task.Step = proto::StepPrepared;
    task.State = proto::TaskStatePending;
    task.ExtraParams.PrepareMode = proto::PrepareModeRequired;
    task.Error = None;
    task.Meta = prepared_meta.Marshal().unwrap();
    let prepared_scheduler = importScheduler::new(Arc::new(Runtime), &task).unwrap();
    assert!(prepared_scheduler.GlobalSort);
    let encode = prepared_scheduler.GetNextStep(&task.TaskBase);
    assert_eq!(encode, proto::ImportStepEncodeAndSort);
    let before = calls.lock().unwrap().len();
    let prepared = nextImportSubtasksBatch(
        &context,
        &prepared_scheduler,
        &manager,
        &handle,
        &mut task,
        &[":4000".into()],
        encode,
        &services,
    )
    .unwrap();
    assert_eq!(prepared.len(), 2);
    let calls = calls.lock().unwrap();
    let phase = &calls[before..];
    assert!(phase.iter().any(|(sql, args)| {
        sql.contains("SET update_time = CURRENT_TIMESTAMP(6), step = %?")
            && args[0] == storage::Value::String(importer::JobStepGlobalSorting.into())
    }));
    assert!(
        !phase
            .iter()
            .any(|(sql, _)| sql.contains("start_time = CURRENT_TIMESTAMP(6)"))
    );
    drop(calls);
    prepared_scheduler.Close(task.ID);
}

/// Execute the global-sort fixture used by Go TestSchedulerExtGlobalSort against
/// the real Rust planner, including its forced data merge and unmerged index.
#[test]
fn scheduler_global_sort_runs_go_seven_stage_subtask_matrix() {
    use crate::{
        ImportStepMeta, LogicalPlan, MergeSortStepMeta, MultipleFilesStat, PlanCtx, SortedKVMeta,
    };
    use astersql_dxf_framework_proto as proto;
    use astersql_executor_importer as importer;
    use astersql_ingestor_globalsort as globalsort;
    use astersql_ingestor_simplesst as simplesst;
    use astersql_objstore as objstore;
    use std::collections::HashMap;

    let storage_ctx = objstore::storage::Context::background();
    let store =
        objstore::storage::NewFromURL(&storage_ctx, "memstore://scheduler-testkit-global").unwrap();
    let mut plan = LogicalPlan::default();
    plan.JobID = 41;
    plan.Plan.CloudStorageURI = "memstore://scheduler-testkit-global".into();
    plan.ChunkMap = HashMap::from([
        (
            1,
            vec![importer::Chunk {
                Path: "gs://test-load/1.csv".into(),
                ..Default::default()
            }],
        ),
        (
            2,
            vec![importer::Chunk {
                Path: "gs://test-load/2.csv".into(),
                ..Default::default()
            }],
        ),
    ]);
    let mut ctx = PlanCtx {
        TaskID: 41,
        GlobalSort: true,
        ThreadCnt: 16,
        ExecuteNodesCnt: 1,
        ObjectStore: Some(store.clone()),
        StorageContext: storage_ctx.clone(),
        ..Default::default()
    };
    let stage = |plan: &mut LogicalPlan, ctx: &PlanCtx, step| {
        let mut input = ctx.clone();
        input.NextTaskStep = step;
        let physical = plan.ToPhysicalPlan(input.clone()).unwrap();
        physical.ToSubtaskMetas(&input, step).unwrap()
    };
    let encode = stage(&mut plan, &ctx, proto::ImportStepEncodeAndSort);
    assert_eq!(encode.len(), 2);

    for file in ["data/1", "index/1", "merged/data"] {
        store
            .WriteFile(
                &storage_ctx,
                file,
                &globalsort::encode_kvs(&[globalsort::KvPair {
                    key: b"a".to_vec(),
                    value: vec![1],
                }]),
            )
            .unwrap();
        store
            .WriteFile(
                &storage_ctx,
                &format!("{file}.stat"),
                &simplesst::codec::encode_multi_props(&[simplesst::codec::RangeProperty {
                    FirstKey: b"a".to_vec(),
                    LastKey: b"a".to_vec(),
                    Size: 2,
                    Keys: 1,
                    Offset: 0,
                }])
                .unwrap(),
            )
            .unwrap();
    }
    let sorted = |file: &str| SortedKVMeta {
        StartKey: b"a".to_vec(),
        EndKey: b"z".to_vec(),
        TotalKVSize: 12,
        MultipleFilesStats: vec![MultipleFilesStat {
            Filenames: vec![[file.into(), format!("{file}.stat")]],
            MinKey: b"a".to_vec(),
            MaxKey: b"z".to_vec(),
            MaxOverlappingNum: 1,
        }],
        ..Default::default()
    };
    let import_meta = ImportStepMeta {
        SortedDataMeta: Some(sorted("data/1")),
        SortedIndexMetas: HashMap::from([(1, sorted("index/1"))]),
        ..Default::default()
    };
    ctx.PreviousSubtaskMetas.insert(
        proto::ImportStepEncodeAndSort,
        vec![import_meta.Marshal().unwrap()],
    );
    ctx.PreviousImportMetas = vec![import_meta];
    ctx.ForceMergeGroup = Some("data".into());
    let merge = stage(&mut plan, &ctx, proto::ImportStepMergeSort);
    assert_eq!(merge.len(), 1);
    ctx.PreviousMergeMetas = vec![MergeSortStepMeta {
        KVGroup: "data".into(),
        SortedKVMeta: sorted("merged/data"),
        ..Default::default()
    }];
    // Go finishes merge subtasks with a new result meta; the scheduled input
    // meta above is not that result. Keep the explicit completed result below.
    ctx.CommitTS = Some(123);
    let ingest = stage(&mut plan, &ctx, proto::ImportStepWriteAndIngest);
    assert_eq!(ingest.len(), 2);
    ctx.PreviousSubtaskMetas
        .insert(proto::ImportStepWriteAndIngest, ingest);
    assert!(stage(&mut plan, &ctx, proto::ImportStepCollectConflicts).is_empty());
    assert!(stage(&mut plan, &ctx, proto::ImportStepConflictResolution).is_empty());
    assert_eq!(
        stage(&mut plan, &ctx, proto::ImportStepPostProcess).len(),
        1
    );
    assert!(stage(&mut plan, &ctx, proto::StepDone).is_empty());
}

/// Go prepare-mode scenario discovers source files before scheduling. Reuse
/// the crate's real controller/session/object-store fixture, then the phase
/// assertion in `scheduler_local_sort_runs_go_import_validate_done_and_revert_phases`.
#[test]
fn scheduler_prepare_mode_runs_real_file_discovery_and_persists_chunks() {
    crate::scheduler_test::prepare_import_task_persists_real_file_controller_result();
}

#[cfg(feature = "nextgen")]
#[test]
fn cancelled_job_stops_prepare_before_starting_work() {
    use crate::scheduler::{ImportSchedulerServices, frameworkTaskToImportTask, prepareImportTask};
    use astersql_dxf_framework_storage as storage;
    use std::sync::{Arc, Mutex};
    struct Backend(storage::TaskManager, Arc<Mutex<Vec<String>>>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.1.lock().unwrap().push(sql.into());
            let mut result = storage::SQLResult {
                rows: vec![],
                affected_rows: 0,
            };
            self.0.WithNewSession(|session| {
                result.rows = session.GetSQLExecutor().execute(sql.into(), args)?;
                result.affected_rows = session.GetSessionVars().StmtCtx.AffectedRows();
                Ok(())
            })?;
            Ok(result)
        }
    }
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let real = session.ImportTaskManager().unwrap();
    real.ExecuteSQLWithNewSession((),
        "INSERT INTO mysql.tidb_import_jobs(id,table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step,error_message) VALUES(%?,%?,%?,%?,%?,%?,%?,%?,%?,%?)",
        vec![41_i64.into(), "test".into(), "t".into(), 8_i64.into(), "root@%".into(), r#"{"format":"csv","file-location":"s3://bucket/file.csv"}"#.into(), 123_i64.into(), "cancelled".into(), "".into(), "cancelled by user".into()],
    ).unwrap();
    let statements = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let statements = statements.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(real.clone(), statements.clone()),
            )))
        }
    }));
    let services = ImportSchedulerServices {
        ControllerServices: Arc::new(|| panic!("cancelled job reached file controller")),
        ResourceCalculatorWithContext: None,
        ImporterService: Arc::new(|| panic!("cancelled job reached importer")),
        KVCodec: vec![],
        Table: None,
        SortStore: None,
        CheckImportTableEmpty: None,
        PlanContext: Arc::new(|_, _| panic!("cancelled job reached planner")),
        FlushStatsBestEffort: None,
    };
    let mut task = astersql_dxf_framework_scheduler::Task::default();
    task.base.task_type = astersql_dxf_framework_proto::ImportInto.to_string();
    task.meta = TaskMeta {
        JobID: 41,
        Plan: astersql_executor_importer::Plan {
            TableInfo: Some(Arc::new(astersql_meta_model::TableInfo {
                ID: 8,
                Name: astersql_parser_ast::NewCIStr("t"),
                ..Default::default()
            })),
            ..Default::default()
        },
        Stmt: "IMPORT INTO test.t FROM 's3://bucket/file.csv'".into(),
        ..Default::default()
    }
    .Marshal()
    .unwrap();
    let mut task = frameworkTaskToImportTask(&task).unwrap();
    let error = prepareImportTask(&Default::default(), &manager, &mut task, &services).unwrap_err();
    assert_eq!(error.to_string(), "import job 41 cancelled by user");
    let statements = statements.lock().unwrap();
    assert_eq!(statements.len(), 1);
    assert!(statements[0].contains("FROM mysql.tidb_import_jobs"));
}

#[cfg(feature = "nextgen")]
#[test]
fn cancelled_and_missing_jobs_stop_scheduler_on_real_sql() {
    use crate::scheduler::{ImportJobJsonCodec, checkImportJobNotCancelled, withImportJobSession};
    use astersql_executor_importer as importer;
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    let mut id = 0;
    withImportJobSession(&manager, |executor| {
        id = importer::CreateJob(
            executor,
            &ImportJobJsonCodec,
            "test",
            "t",
            8,
            "root@%",
            "",
            &importer::ImportParameters {
                Format: "csv".into(),
                ..Default::default()
            },
            123,
        )?;
        Ok(())
    })
    .unwrap();
    checkImportJobNotCancelled(&Default::default(), &manager, id).unwrap();
    withImportJobSession(&manager, |executor| {
        importer::StartJob(executor, id, importer::JobStepPreparing)
    })
    .unwrap();
    checkImportJobNotCancelled(&Default::default(), &manager, id).unwrap();
    withImportJobSession(&manager, |executor| importer::CancelJob(executor, id)).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        checkImportJobNotCancelled(&Default::default(), &manager, id)
            .unwrap_err()
            .to_string(),
        format!("import job {id} cancelled by user")
    );
    assert_eq!(
        checkImportJobNotCancelled(&Default::default(), &manager, id + 1)
            .unwrap_err()
            .to_string(),
        format!("import job {} not found", id + 1)
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "terminal errors must not back off"
    );
    withImportJobSession(&manager, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, id, "", true)?;
        assert_eq!(job.Step, importer::JobStepPreparing);
        assert!(job.IsCancelled());
        Ok(())
    })
    .unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn cancelled_jobs_stop_both_admission_hooks_before_any_side_effects() {
    use crate::scheduler::{
        ImportJobJsonCodec, ImportSchedulerRuntime, ImportSchedulerServices, TaskRegistration,
        frameworkTaskToImportTask, importScheduler, nextImportSubtasksBatch, prepareImportTask,
        withImportJobSession,
    };
    use astersql_dxf_framework_scheduler as framework;
    use astersql_executor_importer as importer;
    use std::sync::Arc;
    struct Runtime;
    impl ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _: i64,
            _: std::time::Duration,
        ) -> Result<Box<dyn TaskRegistration>, astersql_errors::SharedError> {
            panic!("cancelled job registered")
        }
        fn switch_to_import_mode(&self) -> Result<(), astersql_errors::SharedError> {
            panic!("cancelled job switched mode")
        }
        fn switch_to_normal_mode(&self) -> Result<(), astersql_errors::SharedError> {
            Ok(())
        }
    }
    struct Handle;
    impl framework::TaskHandle for Handle {
        fn previous_subtask_metas(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            panic!("cancelled job read subtasks")
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            panic!("cancelled job read summaries")
        }
    }
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    let services = ImportSchedulerServices {
        ControllerServices: Arc::new(|| panic!("cancelled job reached file controller")),
        ResourceCalculatorWithContext: None,
        ImporterService: Arc::new(|| panic!("cancelled job reached importer")),
        KVCodec: vec![],
        Table: None,
        SortStore: None,
        CheckImportTableEmpty: None,
        PlanContext: Arc::new(|_, _| panic!("cancelled job reached planner")),
        FlushStatsBestEffort: None,
    };
    for preparing in [false, true] {
        let mut id = 0;
        withImportJobSession(&manager, |executor| {
            id = importer::CreateJob(
                executor,
                &ImportJobJsonCodec,
                "test",
                "t",
                8,
                "root@%",
                "",
                &importer::ImportParameters {
                    Format: "csv".into(),
                    ..Default::default()
                },
                123,
            )?;
            if preparing {
                importer::StartJob(executor, id, importer::JobStepPreparing)?;
            }
            importer::CancelJob(executor, id)
        })
        .unwrap();
        let mut task = framework::Task::default();
        task.base.task_type = astersql_dxf_framework_proto::ImportInto.into();
        task.meta = TaskMeta {
            JobID: id,
            ..Default::default()
        }
        .Marshal()
        .unwrap();
        let mut task = frameworkTaskToImportTask(&task).unwrap();
        let before = task.Meta.clone();
        let scheduler = importScheduler::new(Arc::new(Runtime), &task).unwrap();
        assert!(
            prepareImportTask(&Default::default(), &manager, &mut task, &services)
                .unwrap_err()
                .to_string()
                .contains("cancelled by user")
        );
        assert!(
            nextImportSubtasksBatch(
                &Default::default(),
                &scheduler,
                &manager,
                &Handle,
                &mut task,
                &[":4000".into()],
                astersql_dxf_framework_proto::ImportStepEncodeAndSort,
                &services
            )
            .unwrap_err()
            .to_string()
            .contains("cancelled by user")
        );
        assert_eq!(task.Meta, before);
        withImportJobSession(&manager, |executor| {
            let job = importer::GetJob(executor, &ImportJobJsonCodec, id, "", true)?;
            assert!(job.IsCancelled());
            assert_eq!(
                job.Step,
                if preparing {
                    importer::JobStepPreparing
                } else {
                    ""
                }
            );
            Ok(())
        })
        .unwrap();
    }
}

#[cfg(feature = "nextgen")]
#[test]
fn admission_retries_transient_sql_errors_using_real_job_metadata() {
    use crate::scheduler::{ImportJobJsonCodec, checkImportJobNotCancelled, withImportJobSession};
    use astersql_dxf_framework_storage as storage;
    use astersql_executor_importer as importer;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let real = session.ImportTaskManager().unwrap();
    let mut id = 0;
    withImportJobSession(&real, |executor| {
        id = importer::CreateJob(
            executor,
            &ImportJobJsonCodec,
            "test",
            "t",
            8,
            "root@%",
            "",
            &importer::ImportParameters {
                Format: "csv".into(),
                ..Default::default()
            },
            123,
        )?;
        Ok(())
    })
    .unwrap();
    struct Backend {
        real: storage::TaskManager,
        calls: Arc<AtomicUsize>,
    }
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(storage::Error::new("transient SQL failure"));
            }
            let mut result = storage::SQLResult {
                rows: vec![],
                affected_rows: 0,
            };
            self.real.WithNewSession(|session| {
                result.rows = session.GetSQLExecutor().execute(sql.into(), args)?;
                result.affected_rows = session.GetSessionVars().StmtCtx.AffectedRows();
                Ok(())
            })?;
            Ok(result)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let calls = calls.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend {
                    real: real.clone(),
                    calls: calls.clone(),
                },
            )))
        }
    }));
    checkImportJobNotCancelled(&Default::default(), &manager, id).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[cfg(not(feature = "nextgen"))]
#[test]
fn classic_admission_does_not_open_an_import_job_session() {
    use astersql_dxf_framework_storage as storage;
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory(|| {
        panic!("Classic must leave cancellation to the DXF framework")
    }));
    crate::scheduler::checkImportJobNotCancelled(&Default::default(), &manager, 41).unwrap();
}
