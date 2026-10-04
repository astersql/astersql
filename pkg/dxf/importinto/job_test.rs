// Copyright 2026 AsterSQL.

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_meta_model as model;
use astersql_parser_ast as ast;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::job::{
    ClassicTableModeChanger, FormatSecondAsTime, GetJobLastUpdateTime, GetRuntimeInfoForJob,
    RuntimeInfo, StorageRuntimeInfoProvider, StorageSessionTableModeChanger,
    StorageTaskSubmissionService, SubmitTask, SubmittedTask, TaskKey, TaskSubmissionService,
    convertToMySQLTime, speed_window,
};

static DEPLOY_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

struct NoUpdateProvider;

struct EmptyErrorProvider;

impl crate::job::RuntimeInfoProvider for EmptyErrorProvider {
    fn GetTaskRuntime(
        &self,
        _task_key: &str,
    ) -> Result<crate::job::TaskRuntimeSnapshot, errors::SharedError> {
        Ok(crate::job::TaskRuntimeSnapshot {
            State: proto::TaskStateFailed,
            Step: proto::ImportStepImport,
            Meta: crate::proto::TaskMeta::default().Marshal()?,
            ErrorMessage: Some(String::new()),
            Subtasks: vec![crate::job::SubtaskRuntimeSummary {
                Processed: 5,
                RowCount: 2,
                Speed: 1,
                UpdateTime: SystemTime::UNIX_EPOCH,
            }],
        })
    }
    fn GetJobLastUpdateTime(
        &self,
        _task_key: &str,
    ) -> Result<Option<SystemTime>, errors::SharedError> {
        Ok(None)
    }
}

impl crate::job::RuntimeInfoProvider for NoUpdateProvider {
    fn GetTaskRuntime(
        &self,
        _task_key: &str,
    ) -> Result<crate::job::TaskRuntimeSnapshot, errors::SharedError> {
        Err(errors::New("unused"))
    }
    fn GetJobLastUpdateTime(
        &self,
        _task_key: &str,
    ) -> Result<Option<SystemTime>, errors::SharedError> {
        Ok(None)
    }
}

struct RecordingBackend(Arc<Mutex<Vec<String>>>);

struct RuntimeBackend {
    meta: Vec<u8>,
    summary: String,
    fail_summary: bool,
}

struct CapturingSubmissionService(Arc<Mutex<Vec<(usize, i32, proto::PrepareMode)>>>);

impl TaskSubmissionService for CapturingSubmissionService {
    fn CreateJobAndTask(
        &self,
        logical_plan: &crate::planner::LogicalPlan,
        _task_key: &str,
        thread_count: i32,
        max_node_count: i32,
    ) -> Result<SubmittedTask, errors::SharedError> {
        self.0.lock().unwrap().push((
            logical_plan.Plan.ThreadCnt,
            max_node_count,
            logical_plan.PrepareMode,
        ));
        assert_eq!(
            thread_count,
            if logical_plan.PrepareMode == proto::PrepareModeRequired {
                1
            } else {
                logical_plan.Plan.ThreadCnt as i32
            }
        );
        Ok(SubmittedTask {
            JobID: 11,
            TaskID: 42,
            TaskKey: TaskKey(11),
        })
    }
}

impl storage::SQLBackend for RuntimeBackend {
    fn execute(
        &self,
        sql: &str,
        _args: Vec<storage::Value>,
    ) -> Result<storage::SQLResult, storage::Error> {
        let rows = if sql.contains("from mysql.tidb_global_task t where task_key = %?") {
            vec![storage::chunk::Row::new(vec![
                42_i64.into(),
                TaskKey(11).into(),
                "ImportInto".into(),
                "pending".into(),
                proto::ImportStepImport.into(),
                0_i64.into(),
                1_i64.into(),
                storage::Value::Time(SystemTime::UNIX_EPOCH),
                "scope".into(),
                1_i64.into(),
                storage::Value::Json("{}".into()),
                "".into(),
                storage::Value::Null,
                storage::Value::Null,
                self.meta.clone().into(),
                "".into(),
                storage::Value::Null,
                storage::Value::Null,
            ])]
        } else if sql.starts_with("select summary from mysql.tidb_background_subtask") {
            if self.fail_summary {
                return Err(storage::Error::new("summary failure"));
            }
            vec![storage::chunk::Row::new(vec![storage::Value::Json(
                self.summary.clone(),
            )])]
        } else {
            vec![]
        };
        Ok(storage::SQLResult {
            rows,
            affected_rows: 0,
        })
    }
}

impl storage::SQLBackend for RecordingBackend {
    fn alter_table_mode_for_import(
        &self,
        database_id: i64,
        table_id: i64,
    ) -> Result<(), storage::Error> {
        self.0
            .lock()
            .unwrap()
            .push(format!("alter mode {database_id} {table_id}"));
        Ok(())
    }

    fn execute(
        &self,
        sql: &str,
        _args: Vec<storage::Value>,
    ) -> Result<storage::SQLResult, storage::Error> {
        self.0.lock().unwrap().push(sql.to_owned());
        let rows = if sql == "SELECT LAST_INSERT_ID();" {
            vec![storage::chunk::Row::new(vec![11_i64.into()])]
        } else if sql == "select @@last_insert_id" {
            vec![storage::chunk::Row::new(vec![42_i64.into()])]
        } else if sql.starts_with("select host, role, cpu_count") {
            vec![storage::chunk::Row::new(vec![
                "host".into(),
                "background".into(),
                8_i64.into(),
            ])]
        } else if sql.contains("from mysql.tidb_global_task t where id = %?") {
            vec![storage::chunk::Row::new(vec![
                42_i64.into(),
                TaskKey(11).into(),
                "ImportInto".into(),
                "pending".into(),
                0_i64.into(),
                0_i64.into(),
                1_i64.into(),
                storage::Value::Time(SystemTime::UNIX_EPOCH),
                "scope".into(),
                1_i64.into(),
                storage::Value::Json("{}".into()),
                "".into(),
            ])]
        } else {
            vec![]
        };
        Ok(storage::SQLResult {
            rows,
            affected_rows: 1,
        })
    }
}

struct RecordingTableMode(Arc<Mutex<Vec<String>>>);

impl ClassicTableModeChanger for RecordingTableMode {
    fn AlterTableModeForImport(
        &self,
        _session: &storage::sessionctx::Context,
        database_id: i64,
        table_id: i64,
    ) -> Result<(), errors::SharedError> {
        self.0
            .lock()
            .unwrap()
            .push(format!("alter mode {database_id} {table_id}"));
        Ok(())
    }
}

struct FailingTableMode;

impl ClassicTableModeChanger for FailingTableMode {
    fn AlterTableModeForImport(
        &self,
        _session: &storage::sessionctx::Context,
        _database_id: i64,
        _table_id: i64,
    ) -> Result<(), errors::SharedError> {
        Err(errors::New("DDL table mode failed"))
    }
}

#[test]
fn percent_preserves_go_negative_processed_value() {
    let info = RuntimeInfo {
        Step: proto::ImportStepImport,
        Processed: -5,
        Total: 10,
        ..Default::default()
    };
    assert_eq!(info.Percent(), "-50");
}

#[test]
fn byte_sizes_follow_go_docker_units_format() {
    let info = RuntimeInfo {
        Step: proto::ImportStepImport,
        Total: 1536,
        Processed: -1024,
        Speed: 1024,
        ..Default::default()
    };
    assert_eq!(info.TotalSize(), "1.5KiB");
    assert_eq!(info.ProcessedSize(), "-1024B");
    assert_eq!(info.SpeedStr(), "1KiB/s");
}

#[test]
fn exported_time_formatter_preserves_go_negative_duration() {
    assert_eq!(FormatSecondAsTime(-1), "00:00:-1");
}

#[test]
fn classic_submission_creates_job_changes_table_mode_and_task_in_one_transaction() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session =
        storage::sessionctx::Context::with_backend(Arc::new(RecordingBackend(calls.clone())));
    let manager = storage::NewTaskManager(storage::util::SessionPool::new(session));
    let service = StorageTaskSubmissionService::WithManagers(
        false,
        Arc::new(StorageSessionTableModeChanger),
        manager.clone(),
        manager,
        "scope".into(),
        true,
    );
    let table = model::TableInfo {
        ID: 7,
        Name: ast::NewCIStr("t"),
        ..Default::default()
    };
    let mut plan = importer::Plan {
        DBName: "db".into(),
        DBID: 3,
        TableInfo: Some(Arc::new(table)),
        ThreadCnt: 1,
        MaxNodeCnt: 1,
        ..Default::default()
    };
    let submitted = SubmitTask(&service, &mut plan, "IMPORT INTO db.t FROM 'file'").unwrap();
    assert_eq!(
        (submitted.JobID, submitted.TaskID, submitted.TaskKey),
        (11, 42, TaskKey(11))
    );
    let calls = calls.lock().unwrap();
    let begin = calls.iter().position(|sql| sql == "begin").unwrap();
    let job = calls
        .iter()
        .position(|sql| sql.starts_with("INSERT INTO mysql.tidb_import_jobs"))
        .unwrap();
    let mode = calls
        .iter()
        .position(|sql| sql == "alter mode 3 7")
        .unwrap();
    let task = calls
        .iter()
        .position(|sql| sql.starts_with("insert into mysql.tidb_global_task"))
        .unwrap();
    let commit = calls.iter().position(|sql| sql == "commit").unwrap();
    assert!(begin < job && job < mode && mode < task && task < commit);
    assert_eq!(calls.iter().filter(|sql| *sql == "begin").count(), 1);
}

#[test]
fn classic_ddl_failure_rolls_back_job_without_creating_task() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session =
        storage::sessionctx::Context::with_backend(Arc::new(RecordingBackend(calls.clone())));
    let manager = storage::NewTaskManager(storage::util::SessionPool::new(session));
    let service = StorageTaskSubmissionService::WithManagers(
        false,
        Arc::new(FailingTableMode),
        manager.clone(),
        manager,
        "scope".into(),
        true,
    );
    let table = model::TableInfo {
        ID: 7,
        Name: ast::NewCIStr("t"),
        ..Default::default()
    };
    let mut plan = importer::Plan {
        DBName: "db".into(),
        DBID: 3,
        TableInfo: Some(Arc::new(table)),
        ThreadCnt: 1,
        MaxNodeCnt: 1,
        ..Default::default()
    };
    let error = SubmitTask(&service, &mut plan, "import").unwrap_err();
    assert!(error.to_string().contains("DDL table mode failed"));
    let calls = calls.lock().unwrap();
    assert!(calls.iter().any(|sql| sql == "rollback"));
    assert!(
        !calls
            .iter()
            .any(|sql| sql.starts_with("insert into mysql.tidb_global_task"))
    );
}

#[test]
fn user_keyspace_submission_commits_job_before_dxf_service_transaction() {
    let local_calls = Arc::new(Mutex::new(Vec::new()));
    let dxf_calls = Arc::new(Mutex::new(Vec::new()));
    let local_session =
        storage::sessionctx::Context::with_backend(Arc::new(RecordingBackend(local_calls.clone())));
    let dxf_session =
        storage::sessionctx::Context::with_backend(Arc::new(RecordingBackend(dxf_calls.clone())));
    let local_manager = storage::NewTaskManager(storage::util::SessionPool::new(local_session));
    let dxf_manager = storage::NewTaskManager(storage::util::SessionPool::new(dxf_session));
    let service = StorageTaskSubmissionService::WithManagers(
        true,
        Arc::new(RecordingTableMode(local_calls.clone())),
        local_manager,
        dxf_manager,
        "dxf_service".into(),
        false,
    );
    let table = model::TableInfo {
        ID: 7,
        Name: ast::NewCIStr("t"),
        ..Default::default()
    };
    let mut plan = importer::Plan {
        DBName: "db".into(),
        DBID: 3,
        TableInfo: Some(Arc::new(table)),
        ThreadCnt: 1,
        MaxNodeCnt: 1,
        ..Default::default()
    };
    let submitted = SubmitTask(&service, &mut plan, "import").unwrap();
    assert_eq!((submitted.JobID, submitted.TaskID), (11, 42));
    let local = local_calls.lock().unwrap();
    let dxf = dxf_calls.lock().unwrap();
    assert!(
        local
            .iter()
            .any(|sql| sql.starts_with("INSERT INTO mysql.tidb_import_jobs"))
    );
    assert!(
        !local
            .iter()
            .any(|sql| sql.starts_with("insert into mysql.tidb_global_task"))
    );
    assert!(
        dxf.iter()
            .any(|sql| sql.starts_with("insert into mysql.tidb_global_task"))
    );
    assert_eq!(local.iter().filter(|sql| *sql == "begin").count(), 1);
    assert_eq!(dxf.iter().filter(|sql| *sql == "begin").count(), 1);
    assert!(local.iter().any(|sql| sql == "commit"));
    assert!(dxf.iter().any(|sql| sql == "commit"));
}

#[test]
fn storage_runtime_provider_decodes_go_progress_history_and_speed() {
    let now = chrono::Utc::now();
    let first = (now - chrono::Duration::seconds(10)).to_rfc3339();
    let last = (now - chrono::Duration::seconds(5)).to_rfc3339();
    let summary = serde_json::json!({
        "row_count": 9, "bytes": 100,
        "progresses": [
            { "bytes": 0, "update_time": first },
            { "bytes": 100, "update_time": last }
        ]
    })
    .to_string();
    let meta = crate::proto::TaskMeta::default().Marshal().unwrap();
    let backend = Arc::new(RuntimeBackend {
        meta,
        summary,
        fail_summary: false,
    });
    let session = storage::sessionctx::Context::with_backend(backend);
    let manager = storage::NewTaskManager(storage::util::SessionPool::new(session));
    let provider = StorageRuntimeInfoProvider::WithManager(manager);
    let info = GetRuntimeInfoForJob(&provider, chrono_tz::UTC, 11).unwrap();
    assert_eq!(info.Processed, 100);
    assert_eq!(info.ImportRows, 9);
    assert!(info.Speed > 0);
    assert!(info.UpdateTime.is_some());
}

#[test]
fn invalid_task_meta_error_precedes_subtask_summary_error_like_go() {
    let backend = Arc::new(RuntimeBackend {
        meta: b"not-json".to_vec(),
        summary: String::new(),
        fail_summary: true,
    });
    let session = storage::sessionctx::Context::with_backend(backend);
    let manager = storage::NewTaskManager(storage::util::SessionPool::new(session));
    let provider = StorageRuntimeInfoProvider::WithManager(manager);
    let error = GetRuntimeInfoForJob(&provider, chrono_tz::UTC, 11).unwrap_err();
    assert!(!error.to_string().contains("summary failure"));
}

#[test]
fn async_prepare_keeps_logical_plan_copy_and_mutates_caller_plan_like_go() {
    let _guard = DEPLOY_MODE_TEST_LOCK.lock().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = CapturingSubmissionService(calls.clone());
    let table = model::TableInfo {
        ID: 7,
        Name: ast::NewCIStr("t"),
        ..Default::default()
    };
    let mut plan = importer::Plan {
        TableInfo: Some(Arc::new(table)),
        ThreadCnt: 8,
        MaxNodeCnt: 4,
        CloudStorageURI: "s3://bucket/path".into(),
        ..Default::default()
    };
    let async_prepare = crate::job::ShouldUseAsyncPrepare(&plan);
    SubmitTask(&service, &mut plan, "import").unwrap();
    let captured = calls.lock().unwrap()[0];
    assert_eq!(captured.0, 8);
    if async_prepare {
        assert_eq!((plan.ThreadCnt, plan.MaxNodeCnt), (1, 1));
        assert_eq!(captured.1, 1);
        assert_eq!(captured.2, proto::PrepareModeRequired);
    } else {
        assert_eq!((plan.ThreadCnt, plan.MaxNodeCnt), (8, 4));
        assert_eq!(captured.1, 4);
        assert_eq!(captured.2, proto::PrepareModeDisabled);
    }
}

#[test]
fn starter_global_sort_uses_synchronous_prepare() {
    if astersql_config_kerneltype::IsClassic() {
        return;
    }

    let _guard = DEPLOY_MODE_TEST_LOCK.lock().unwrap();
    let original_mode = astersql_config_deploymode::Get();
    struct RestoreDeployMode(astersql_config_deploymode::Mode);
    impl Drop for RestoreDeployMode {
        fn drop(&mut self) {
            astersql_config_deploymode::Set(self.0).unwrap();
        }
    }
    let _restore = RestoreDeployMode(original_mode);

    let mut global_sort_plan = importer::Plan {
        TableInfo: Some(Arc::new(model::TableInfo {
            ID: 7,
            Name: ast::NewCIStr("t"),
            ..Default::default()
        })),
        ThreadCnt: 4,
        MaxNodeCnt: 2,
        CloudStorageURI: "s3://bucket/path".into(),
        ..Default::default()
    };

    astersql_config_deploymode::Set(astersql_config_deploymode::Premium).unwrap();
    assert!(crate::job::ShouldUseAsyncPrepare(&global_sort_plan));
    astersql_config_deploymode::Set(astersql_config_deploymode::PremiumReserved).unwrap();
    assert!(crate::job::ShouldUseAsyncPrepare(&global_sort_plan));
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    assert!(!crate::job::ShouldUseAsyncPrepare(&global_sort_plan));

    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = CapturingSubmissionService(calls.clone());
    SubmitTask(&service, &mut global_sort_plan, "import").unwrap();
    assert_eq!(
        (global_sort_plan.ThreadCnt, global_sort_plan.MaxNodeCnt),
        (4, 2)
    );
    assert_eq!(calls.lock().unwrap()[0], (4, 2, proto::PrepareModeDisabled));
}

#[test]
fn runtime_update_time_truncates_fraction_and_uses_requested_mysql_location() {
    let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(600);
    assert_eq!(
        convertToMySQLTime(time, chrono_tz::Asia::Shanghai)
            .unwrap()
            .String(),
        "1970-01-01 08:00:00"
    );
}

#[test]
fn job_without_subtask_updates_returns_go_mysql_zero_time() {
    assert_eq!(
        GetJobLastUpdateTime(&NoUpdateProvider, 11).unwrap(),
        astersql_types::time::ZeroTime
    );
}

#[test]
fn speed_window_matches_go_five_three_second_summary_intervals() {
    assert_eq!(speed_window(), std::time::Duration::from_secs(15));
}

#[test]
fn task_error_object_short_circuits_progress_even_with_empty_message() {
    let info = GetRuntimeInfoForJob(&EmptyErrorProvider, chrono_tz::UTC, 11).unwrap();
    assert_eq!(info.Processed, 0);
    assert_eq!(info.ImportRows, 0);
}
