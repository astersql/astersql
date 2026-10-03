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
// `IMPORT INTO` 列赋值表达式可选属性校验的单元测试。
//
// `IMPORT INTO`：将外部数据批量导入表；列赋值中的标量函数
// 只能依赖编码上下文已提供的可选属性位。

use crate::import_into::{ImportExpression, checkExprWithProvidedProps};
#[derive(Clone)]
/// 测试用表达式树节点，实现 `ImportExpression`。
struct Expr {
    name: Option<&'static str>,
    required: u64,
    children: Vec<Expr>,
}
impl ImportExpression for Expr {
    fn scalar_function_name(&self) -> Option<&str> {
        self.name
    }
    fn required_optional_properties(&self) -> u64 {
        self.required
    }
    fn children(&self) -> &[Self] {
        &self.children
    }
}
#[test]
/// 验证属性位不足时递归报错指向子函数，位足够时通过。
fn import_assignments_reject_unsupported_optional_properties_recursively() {
    let expr = Expr {
        name: Some("parent"),
        required: 1,
        children: vec![Expr {
            name: Some("child"),
            required: 2,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(3, &expr, 3), Ok(()));
    let error = checkExprWithProvidedProps(3, &expr, 1).unwrap_err();
    assert_eq!(error.function_name, "child");
    assert_eq!(error.assignment_index, 3);
}

#[test]
/// Mirrors Go's assignment matrix: supported functions pass, while the first
/// unsupported scalar reports its exact normalized name and assignment index.
fn import_assignment_validation_matches_go_error_contract() {
    let cases = [
        ("setvar", 0usize),
        ("current_user", 0),
        ("current_role", 0),
        ("connection_id", 0),
        ("tidb_is_ddl_owner", 1),
        ("sleep", 0),
        ("last_insert_id", 0),
    ];

    for (name, index) in cases {
        let expression = Expr {
            name: Some(name),
            required: 1,
            children: vec![],
        };
        let error = checkExprWithProvidedProps(index, &expression, 0).unwrap_err();
        assert_eq!(error.function_name, name);
        assert_eq!(error.assignment_index, index);
        assert_eq!(
            error.to_string(),
            format!(
                "FUNCTION {name} is not supported in IMPORT INTO column assignment, index {index}"
            )
        );
    }

    let supported = Expr {
        name: Some("concat"),
        required: 0,
        children: vec![Expr {
            name: Some("getvar"),
            required: 0,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(0, &supported, 0), Ok(()));
}

#[test]
/// Go intentionally descends only through ScalarFunction nodes; constants and
/// other expression kinds have no scalar arguments to validate.
fn non_scalar_expression_does_not_descend_into_children() {
    let expression = Expr {
        name: None,
        required: 0,
        children: vec![Expr {
            name: Some("setvar"),
            required: 1,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(0, &expression, 0), Ok(()));
}

#[cfg(feature = "nextgen")]
#[test]
fn dangling_import_job_cancellation_preserves_state_guards_on_real_sql() {
    use astersql_dxf_importinto::scheduler::{ImportJobJsonCodec, withImportJobSession};
    use astersql_executor_importer as importer;
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    let create = || {
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
                    FileLocation: "s3://bucket/file.csv".into(),
                    ..Default::default()
                },
                123,
            )?;
            Ok(())
        })
        .unwrap();
        id
    };
    let pending = create();
    crate::import_into::cancelAndWaitImportJobInStorage(
        &Default::default(),
        pending,
        &manager,
        &manager,
    )
    .unwrap();
    withImportJobSession(&manager, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, pending, "root@%", true)?;
        assert!(job.IsCancelled());
        assert_eq!(job.ErrorMessage, "cancelled by user");
        assert!(job.StartTime.IsZero());
        assert!(job.EndTime.IsZero());
        assert_eq!(importer::GetActiveJobCnt(executor, "test", "t")?, 0);
        Ok(())
    })
    .unwrap();
    assert!(
        crate::import_into::cancelDanglingImportJob(&manager, pending)
            .unwrap_err()
            .to_string()
            .contains("job state changed during cancel")
    );
    let running = create();
    withImportJobSession(&manager, |executor| {
        importer::StartJob(executor, running, importer::JobStepImporting)
    })
    .unwrap();
    assert!(
        crate::import_into::cancelAndWaitImportJobInStorage(
            &Default::default(),
            running,
            &manager,
            &manager
        )
        .unwrap_err()
        .to_string()
        .contains("job state changed during cancel")
    );
    withImportJobSession(&manager, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, running, "root@%", true)?;
        assert_eq!(job.Status, importer::JobStatusRunning);
        assert_eq!(job.Step, importer::JobStepImporting);
        assert!(!job.IsCancelled());
        assert!(job.ErrorMessage.is_empty());
        assert_eq!(importer::GetActiveJobCnt(executor, "test", "t")?, 1);
        Ok(())
    })
    .unwrap();
    manager
        .WithNewSession(|session| {
            let mut executor = astersql_dxf_importinto::scheduler::ImportJobStorageSession {
                executor: session.GetSQLExecutor(),
            };
            importer::CancelJob(&mut executor, running)
                .map_err(astersql_dxf_framework_storage::Error::new)?;
            assert_eq!(session.GetSessionVars().StmtCtx.AffectedRows(), 1);
            importer::CancelJob(&mut executor, running)
                .map_err(astersql_dxf_framework_storage::Error::new)?;
            assert_eq!(session.GetSessionVars().StmtCtx.AffectedRows(), 0);
            assert_eq!(
                importer::GetActiveJobCnt(&mut executor, "test", "t").unwrap(),
                0
            );
            Ok(())
        })
        .unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn user_keyspace_job_cancelled_before_task_commit_stops_admission() {
    use astersql_dxf_importinto::job::{
        StorageSessionTableModeChanger, StorageTaskSubmissionService, SubmitTask,
    };
    use astersql_dxf_importinto::scheduler::{
        ImportJobJsonCodec, checkImportJobNotCancelled, withImportJobSession,
    };
    use astersql_executor_importer as importer;
    use std::sync::Arc;
    let (_user_domain, user) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let (_sys_domain, system) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let user_manager = user.ImportTaskManager().unwrap();
    let sys_manager = system.ImportTaskManager().unwrap();
    sys_manager
        .InitMeta((), ":4000".into(), "dxf_service".into())
        .unwrap();
    let service = StorageTaskSubmissionService::WithManagers(
        true,
        Arc::new(StorageSessionTableModeChanger),
        user_manager.clone(),
        sys_manager.clone(),
        "dxf_service".into(),
        false,
    )
    .WithAfterUserJobCreated(Arc::new({
        let user_manager = user_manager.clone();
        let sys_manager = sys_manager.clone();
        move |id| {
            assert_eq!(
                sys_manager
                    .GetTaskBaseByKeyWithHistory((), astersql_dxf_importinto::TaskKey(id))
                    .unwrap_err(),
                astersql_dxf_framework_storage::ErrTaskNotFound
            );
            crate::import_into::cancelAndWaitImportJobInStorage(
                &Default::default(),
                id,
                &sys_manager,
                &user_manager,
            )
            .unwrap();
        }
    }));
    let mut plan = importer::Plan {
        DBName: "test".into(),
        DBID: 7,
        User: "root@%".into(),
        Keyspace: "keyspace_cancel".into(),
        ThreadCnt: 1,
        MaxNodeCnt: 1,
        TableInfo: Some(Arc::new(astersql_meta_model::TableInfo {
            ID: 8,
            Name: astersql_parser_ast::NewCIStr("t"),
            ..Default::default()
        })),
        Parameters: Some(importer::ImportParameters {
            Format: "csv".into(),
            FileLocation: "gs://bucket/data.csv".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let task = SubmitTask(
        &service,
        &mut plan,
        "IMPORT INTO test.t FROM 'gs://bucket/data.csv'",
    )
    .unwrap();
    assert!(
        sys_manager
            .GetTaskBaseByKeyWithHistory((), task.TaskKey.clone())
            .is_ok()
    );
    assert_eq!(
        user_manager
            .GetTaskBaseByKeyWithHistory((), task.TaskKey)
            .unwrap_err(),
        astersql_dxf_framework_storage::ErrTaskNotFound
    );
    assert_eq!(
        checkImportJobNotCancelled(&Default::default(), &user_manager, task.JobID)
            .unwrap_err()
            .to_string(),
        format!("import job {} cancelled by user", task.JobID)
    );
    withImportJobSession(&user_manager, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, task.JobID, "", true)?;
        assert!(job.IsCancelled());
        assert!(job.Step.is_empty());
        assert_eq!(importer::GetActiveJobCnt(executor, "test", "t")?, 0);
        Ok(())
    })
    .unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn task_started_after_probe_miss_cannot_cancel_a_running_job() {
    use astersql_dxf_framework_storage as storage;
    use astersql_dxf_importinto::scheduler::{ImportJobJsonCodec, withImportJobSession};
    use astersql_executor_importer as importer;
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    manager.InitMeta((), ":4000".into(), "".into()).unwrap();
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
    let key = astersql_dxf_importinto::TaskKey(id);
    let error = crate::import_into::cancelImportJobWithFallbackHook(
        &Default::default(),
        id,
        &manager,
        &manager,
        || {
            manager
                .CreateTask(
                    (),
                    key.clone(),
                    storage::proto::ImportInto,
                    "".into(),
                    1,
                    "".into(),
                    1,
                    storage::proto::ExtraParams::default(),
                    b"{}".to_vec(),
                )
                .unwrap();
            let task = manager.GetTaskByKeyWithHistory((), key.clone()).unwrap();
            manager
                .SwitchTaskStep(
                    (),
                    task,
                    storage::proto::TaskStateRunning,
                    astersql_dxf_framework_proto::ImportStepEncodeAndSort,
                    vec![],
                )
                .unwrap();
            withImportJobSession(&manager, |executor| {
                importer::StartJob(executor, id, importer::JobStepGlobalSorting)
            })
            .unwrap();
        },
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "job state changed during cancel, please try again later"
    );
    assert_eq!(
        manager.GetTaskBaseByKeyWithHistory((), key).unwrap().State,
        storage::proto::TaskStateRunning
    );
    withImportJobSession(&manager, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, id, "", true)?;
        assert_eq!(job.Status, importer::JobStatusRunning);
        assert_eq!(job.Step, importer::JobStepGlobalSorting);
        assert!(job.ErrorMessage.is_empty());
        assert_eq!(importer::GetActiveJobCnt(executor, "test", "t")?, 1);
        Ok(())
    })
    .unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn failed_task_probe_does_not_cancel_import_job() {
    use astersql_dxf_framework_storage as storage;
    use std::sync::Arc;
    struct Failure;
    impl storage::SQLBackend for Failure {
        fn execute(
            &self,
            _: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            Err(storage::Error::new("task lookup unavailable"))
        }
    }
    let tasks = storage::NewTaskManager(storage::util::SessionPool::with_factory(|| {
        Ok(storage::sessionctx::Context::with_backend(Arc::new(
            Failure,
        )))
    }));
    let jobs = storage::NewTaskManager(storage::util::SessionPool::with_factory(|| {
        panic!("lookup failure touched job")
    }));
    assert_eq!(
        crate::import_into::cancelAndWaitImportJobInStorage(&Default::default(), 41, &tasks, &jobs)
            .unwrap_err()
            .to_string(),
        "task lookup unavailable"
    );
}

#[cfg(feature = "nextgen")]
#[test]
fn archived_reverted_task_does_not_cancel_pending_user_job() {
    use astersql_dxf_framework_storage as storage;
    use astersql_dxf_importinto::scheduler::{ImportJobJsonCodec, withImportJobSession};
    use astersql_executor_importer as importer;
    let (_user_domain, user) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let (_sys_domain, system) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let jobs = user.ImportTaskManager().unwrap();
    let tasks = system.ImportTaskManager().unwrap();
    tasks
        .InitMeta((), ":4000".into(), "dxf_service".into())
        .unwrap();
    let mut id = 0;
    withImportJobSession(&jobs, |executor| {
        id = importer::CreateJob(
            executor,
            &ImportJobJsonCodec,
            "test",
            "t",
            8,
            "root@%",
            "",
            &importer::ImportParameters::default(),
            123,
        )?;
        Ok(())
    })
    .unwrap();
    let key = astersql_dxf_importinto::TaskKey(id);
    let task_id = tasks
        .CreateTask(
            (),
            key.clone(),
            storage::proto::ImportInto,
            "".into(),
            1,
            "".into(),
            0,
            Default::default(),
            b"{}".to_vec(),
        )
        .unwrap();
    // As in Go, bypass scheduler OnDone to isolate existence from cancellability.
    tasks
        .RevertTask(
            (),
            task_id,
            storage::proto::TaskStatePending,
            storage::Error::new("already reverted"),
        )
        .unwrap();
    tasks.RevertedTask((), task_id).unwrap();
    tasks
        .TransferTasks2History((), vec![tasks.GetTaskByID((), task_id).unwrap()])
        .unwrap();
    assert_eq!(
        tasks.GetTaskByKey((), key.clone()).unwrap_err(),
        storage::ErrTaskNotFound
    );
    crate::import_into::cancelAndWaitImportJobInStorage(&Default::default(), id, &tasks, &jobs)
        .unwrap();
    assert_eq!(
        tasks.GetTaskBaseByKeyWithHistory((), key).unwrap().State,
        storage::proto::TaskStateReverted
    );
    withImportJobSession(&jobs, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, id, "", true)?;
        assert_eq!(job.Status, importer::jobStatusPending);
        assert!(job.ErrorMessage.is_empty());
        Ok(())
    })
    .unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn task_committed_after_lookup_miss_does_not_delay_pending_job_cancel() {
    use astersql_dxf_framework_storage as storage;
    use astersql_dxf_importinto::scheduler::{ImportJobJsonCodec, withImportJobSession};
    use astersql_executor_importer as importer;
    let (_user_domain, user) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let (_sys_domain, system) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let jobs = user.ImportTaskManager().unwrap();
    let tasks = system.ImportTaskManager().unwrap();
    tasks
        .InitMeta((), ":4000".into(), "dxf_service".into())
        .unwrap();
    let mut id = 0;
    withImportJobSession(&jobs, |executor| {
        id = importer::CreateJob(
            executor,
            &ImportJobJsonCodec,
            "test",
            "t",
            8,
            "root@%",
            "",
            &importer::ImportParameters::default(),
            123,
        )?;
        Ok(())
    })
    .unwrap();
    let key = astersql_dxf_importinto::TaskKey(id);
    assert_eq!(
        tasks
            .GetTaskBaseByKeyWithHistory((), key.clone())
            .unwrap_err(),
        storage::ErrTaskNotFound
    );
    let context = astersql_dxf_framework_handle::Context::background();
    let (done, receive) = std::sync::mpsc::channel();
    let watchdog_context = context.clone();
    let watchdog = std::thread::spawn(move || {
        if receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_err()
        {
            watchdog_context.cancel();
        }
    });
    let result =
        crate::import_into::cancelImportJobWithFallbackHook(&context, id, &tasks, &jobs, || {
            tasks
                .CreateTask(
                    (),
                    key.clone(),
                    storage::proto::ImportInto,
                    "".into(),
                    1,
                    "".into(),
                    0,
                    Default::default(),
                    b"{}".to_vec(),
                )
                .unwrap();
        });
    let _ = done.send(());
    watchdog.join().unwrap();
    result.expect("cancel must not wait for the task committed after the probe miss");
    assert_eq!(
        tasks
            .GetTaskBaseByKeyWithHistory((), key.clone())
            .unwrap()
            .State,
        storage::proto::TaskStatePending
    );
    assert_eq!(
        jobs.GetTaskBaseByKeyWithHistory((), key).unwrap_err(),
        storage::ErrTaskNotFound
    );
    withImportJobSession(&jobs, |executor| {
        let job = importer::GetJob(executor, &ImportJobJsonCodec, id, "", true)?;
        assert!(job.IsCancelled());
        assert_eq!(job.ErrorMessage, "cancelled by user");
        Ok(())
    })
    .unwrap();
}
