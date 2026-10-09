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

// Import Into 调度器单元测试与 Go 草稿存档。
//
// `_GO_SCHEDULER_TEST_DRAFT` 保留 eligible instances、当前任务缓存、初始化、
// next step、可重试错误与 TiKV 导入状态判断等端到端草稿；
// 可执行部分目前覆盖 `getStepOfEncode` 按排序模式选择编码步骤。

const _GO_SCHEDULER_TEST_DRAFT: &str = r###"
// 这段逻辑覆盖 scheduler extension 的 eligible instances、当前任务缓存、初始化、next step、retryable error 和 TiKV 导入状态判断。

// import_into_suite 对应 Go 的 importIntoSuite；suite.Suite 嵌入在这里以空结构表达。
pub struct import_into_suite;

// test_import_into 对应 Go 的 TestImportInto，入口由 testify suite 分发到下方方法。
#[test]
pub fn test_import_into() {
    let mut suite = import_into_suite;
    suite.test_scheduler_get_eligible_instances();
    suite.test_update_current_task();
    suite.test_scheduler_init();
    suite.test_get_task_mgr_for_accessing_import_job_uses_task_runtime();
    suite.test_get_next_step();
    suite.test_get_step_of_encode();
    suite.test_is_retryable();
}

// new_mock_runtime 对应 Go 的同名 helper：为 scheduler.Param 提供 Store/SysSessionPool 期望。
pub fn new_mock_runtime(ctrl: &gomock::Controller, store: kv::Storage, se_pool: tidbutil::DestroyableSessionPool) -> sqlsvrapimock::MockRuntime {
    let runtime = sqlsvrapimock::NewMockRuntime(ctrl);
    runtime.EXPECT().Store().Return(store).AnyTimes();
    runtime.EXPECT().SysSessionPool().Return(se_pool).AnyTimes();
    runtime
}

// new_scheduler_param_for_test 对应 Go helper：没有传入 session pool 时创建一个 1 容量 pool，并在测试清理时关闭。
pub fn new_scheduler_param_for_test(
    store: kv::Storage,
    task_mgr: scheduler::TaskManager,
    mut se_pool: tidbutil::DestroyableSessionPool,
) -> scheduler::Param {
    let ctrl = gomock::NewController();
    if se_pool.is_none() {
        se_pool = tidbutil::NewSessionPool(1, || {
            let mut se = utilmock::NewContext();
            se.Store = store.clone();
            Ok(se)
        }, None, None, None);
        // Go 用 t.Cleanup(sePool.Close) 释放 session pool；保留资源收尾语义。
        defer_cleanup(|| se_pool.Close());
    }
    scheduler::NewParamForTest(task_mgr, new_mock_runtime(&ctrl, store, se_pool))
}

impl import_into_suite {
    // enable_fail_point 对应 Go suite helper；真实 failpoint 只在 Go 测试运行时生效。
    pub fn enable_fail_point(&mut self, path: &str, term: &str) {
        failpoint::Enable(path, term).expect("enable failpoint");
        defer_cleanup(|| { let _ = failpoint::Disable(path); });
    }

    // test_scheduler_get_eligible_instances 对应 TestSchedulerGetEligibleInstances。
    pub fn test_scheduler_get_eligible_instances(&mut self) {
        let sch = importScheduler::default();
        let mut task = proto::Task { Meta: b"{}".to_vec(), ..Default::default() };
        let ctx = context::WithValue(context::Background(), "etcd", true);
        let eligible = sch.GetEligibleInstances(ctx.clone(), &task).expect("empty eligible");
        // Go 注释说明 slice 顺序不稳定；空列表直接断言。
        assert!(eligible.is_empty());

        task.Meta = br#"{"EligibleInstances":[{"ip": "1.1.1.1", "listening_port": 4000}]}"#.to_vec();
        let eligible = sch.GetEligibleInstances(ctx, &task).expect("eligible instances");
        assert_eq!(eligible, vec!["1.1.1.1:4000"]);
    }

    // test_update_current_task 对应 TestUpdateCurrentTask，验证 currTaskID 和 disableTiKVImportMode 原子字段。
    pub fn test_update_current_task(&mut self) {
        let task_meta = TaskMeta { Plan: importer::Plan { DisableTiKVImportMode: true, ..Default::default() }, ..Default::default() };
        let bs = json::Marshal(&task_meta).expect("marshal task meta");
        let sch = importScheduler::default();
        assert_eq!(sch.currTaskID.Load(), 0);
        assert!(!sch.disableTiKVImportMode.Load());

        sch.updateCurrentTask(&proto::Task { TaskBase: proto::TaskBase { ID: 1, ..Default::default() }, Meta: bs.clone(), ..Default::default() });
        assert_eq!(sch.currTaskID.Load(), 1);
        assert!(sch.disableTiKVImportMode.Load());

        // 同一 task 再次更新不应改变缓存状态。
        sch.updateCurrentTask(&proto::Task { TaskBase: proto::TaskBase { ID: 1, ..Default::default() }, Meta: bs, ..Default::default() });
        assert_eq!(sch.currTaskID.Load(), 1);
        assert!(sch.disableTiKVImportMode.Load());
    }

    // test_scheduler_init 对应 TestSchedulerInit，覆盖 local sort/global sort 和 nextgen keyspace mismatch。
    pub fn test_scheduler_init(&mut self) {
        let mut meta = TaskMeta { Plan: importer::Plan { CloudStorageURI: "".to_owned(), ..Default::default() }, ..Default::default() };
        let mut bytes = json::Marshal(&meta).expect("marshal local meta");
        let task_ks = if kerneltype::IsNextGen() { "user_keyspace" } else { "" };
        let mut sch = importScheduler {
            BaseScheduler: scheduler::NewBaseScheduler(
                context::Background(),
                &proto::Task { TaskBase: proto::TaskBase { Keyspace: task_ks.to_owned(), ..Default::default() }, Meta: bytes, ..Default::default() },
                new_scheduler_param_for_test(StoreWithKS { ks: task_ks.to_owned(), ..Default::default() }, None, None),
            ),
            ..Default::default()
        };
        sch.Init().expect("local init");
        assert!(!sch.Extension.downcast_ref::<importScheduler>().unwrap().GlobalSort);

        meta.Plan.CloudStorageURI = "s3://test".to_owned();
        bytes = json::Marshal(&meta).expect("marshal global meta");
        sch = importScheduler {
            BaseScheduler: scheduler::NewBaseScheduler(
                context::Background(),
                &proto::Task { TaskBase: proto::TaskBase { Keyspace: task_ks.to_owned(), ..Default::default() }, Meta: bytes.clone(), ..Default::default() },
                new_scheduler_param_for_test(StoreWithKS { ks: task_ks.to_owned(), ..Default::default() }, None, None),
            ),
            ..Default::default()
        };
        sch.Init().expect("global init");
        assert!(sch.Extension.downcast_ref::<importScheduler>().unwrap().GlobalSort);

        if kerneltype::IsNextGen() {
            let bad_sch = importScheduler {
                BaseScheduler: scheduler::NewBaseScheduler(
                    context::Background(),
                    &proto::Task { TaskBase: proto::TaskBase { Keyspace: task_ks.to_owned(), ..Default::default() }, Meta: bytes, ..Default::default() },
                    new_scheduler_param_for_test(StoreWithKS::default(), None, None),
                ),
                ..Default::default()
            };
            assert!(bad_sch.Init().unwrap_err().to_string().contains("store keyspace mismatch with task"));
        }
    }

    // test_get_task_mgr_for_accessing_import_job_uses_task_runtime 对应 nextgen keyspace 场景下的 task runtime manager 选择。
    pub fn test_get_task_mgr_for_accessing_import_job_uses_task_runtime(&mut self) {
        if !kerneltype::IsNextGen() {
            // Go 使用 s.T().Skip；以早退保留 classic kernel 不覆盖此路径。
            return;
        }
        let ctrl = gomock::NewController();
        let task_mgr = mock::NewMockTaskManager(&ctrl);
        let sess_pool = tidbutil::NewSessionPool(1, || Err(errors::New("unexpected session pool use")), None, None, None);
        let task_ks = "user_keyspace";
        let param = new_scheduler_param_for_test(StoreWithKS { ks: task_ks.to_owned(), ..Default::default() }, task_mgr.clone(), sess_pool);
        let sch = importScheduler {
            BaseScheduler: scheduler::NewBaseScheduler(
                context::Background(),
                &proto::Task { TaskBase: proto::TaskBase { Keyspace: task_ks.to_owned(), ..Default::default() }, ..Default::default() },
                param,
            ),
            ..Default::default()
        };
        let got = sch.getTaskMgrForAccessingImportJob().expect("task runtime manager");
        assert!(got.is_some());
        assert!(std::ptr::eq(got.unwrap(), sch.taskKSTaskMgr.unwrap()));
    }

    // test_get_next_step 对应 TestGetNextStep，覆盖 local sort 与 global sort 两套状态机。
    pub fn test_get_next_step(&mut self) {
        let mut task = proto::TaskBase { Step: proto::StepInit, ..Default::default() };
        let ext = importScheduler::default();
        for next_step in [proto::ImportStepImport, proto::ImportStepPostProcess, proto::StepDone] {
            assert_eq!(ext.GetNextStep(&task), next_step);
            task.Step = next_step;
        }

        task.Step = proto::StepInit;
        let ext = importScheduler { GlobalSort: true, ..Default::default() };
        for next_step in [
            proto::ImportStepEncodeAndSort,
            proto::ImportStepMergeSort,
            proto::ImportStepWriteAndIngest,
            proto::ImportStepCollectConflicts,
            proto::ImportStepConflictResolution,
            proto::ImportStepPostProcess,
            proto::StepDone,
        ] {
            assert_eq!(ext.GetNextStep(&task), next_step);
            task.Step = next_step;
        }
    }

    // test_get_step_of_encode 对应 TestGetStepOfEncode。
    pub fn test_get_step_of_encode(&mut self) {
        assert_eq!(getStepOfEncode(false), proto::ImportStepImport);
        assert_eq!(getStepOfEncode(true), proto::ImportStepEncodeAndSort);
    }

    // test_is_retryable 对应 TestIsRetryable，区分 region/cross-keyspace 临时错误和 load data precheck 业务错误。
    pub fn test_is_retryable(&mut self) {
        let ext = importScheduler::default();
        assert!(ext.IsRetryableErr(drivererr::ErrRegionUnavailable));
        assert!(ext.IsRetryableErr(errors::Annotatef(errGetCrossKSSessionPool, "test")));
        assert!(!ext.IsRetryableErr(exeerrors::ErrLoadDataPreCheckFailed.FastGenByArgs("target table is not empty")));
    }
}

// test_is_importing_2_tikv 对应 Go 的 TestIsImporting2TiKV，验证仅 import/write-ingest 两步算导入 TiKV。
#[test]
pub fn test_is_importing_2_tikv() {
    let ext = importScheduler::default();
    assert!(!ext.isImporting2TiKV(&proto::Task { TaskBase: proto::TaskBase { Step: proto::ImportStepEncodeAndSort, ..Default::default() }, ..Default::default() }));
    assert!(!ext.isImporting2TiKV(&proto::Task { TaskBase: proto::TaskBase { Step: proto::ImportStepMergeSort, ..Default::default() }, ..Default::default() }));
    assert!(!ext.isImporting2TiKV(&proto::Task { TaskBase: proto::TaskBase { Step: proto::ImportStepPostProcess, ..Default::default() }, ..Default::default() }));
    assert!(ext.isImporting2TiKV(&proto::Task { TaskBase: proto::TaskBase { Step: proto::ImportStepImport, ..Default::default() }, ..Default::default() }));
    assert!(ext.isImporting2TiKV(&proto::Task { TaskBase: proto::TaskBase { Step: proto::ImportStepWriteAndIngest, ..Default::default() }, ..Default::default() }));
}
"###;

use crate::getStepOfEncode;
use astersql_dxf_framework_proto::{ImportStepEncodeAndSort, ImportStepImport};

#[test]
fn post_process_summary_preserves_existing_rows_and_go_arithmetic() {
    use crate::proto::{CollectConflictsStepMeta, TaskMeta};
    use crate::scheduler::{PostProcessSummaryInput, updateTaskSummary};
    use astersql_dxf_framework_proto::{
        ExtraParams, ImportStepPostProcess, ModifyParam, NormalPriority, StepInit, Task, TaskBase,
        TaskStatePending, TaskTypeExample,
    };
    use astersql_executor_importer::StepSummary;
    use std::time::SystemTime;

    let mut task = Task {
        TaskBase: TaskBase {
            ID: 1,
            Key: String::new(),
            Type: TaskTypeExample,
            State: TaskStatePending,
            Step: StepInit,
            Priority: NormalPriority,
            RequiredSlots: 0,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: vec![],
        Error: None,
        ModifyParam: ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    };
    let mut meta = TaskMeta::default();
    meta.Summary.ImportedRows = 7;
    updateTaskSummary(
        &mut task,
        &mut meta,
        ImportStepPostProcess,
        &StepSummary::default(),
        Some(PostProcessSummaryInput {
            encoded_row_counts: &[3, 4],
            conflict_metas: &[],
        }),
    )
    .unwrap();
    assert_eq!(meta.Summary.ImportedRows, 14);

    meta.Plan.CloudStorageURI = "s3://bucket".into();
    meta.Summary.ImportedRows = 2;
    let conflict = CollectConflictsStepMeta {
        ConflictedRowCount: 5,
        ..Default::default()
    };
    updateTaskSummary(
        &mut task,
        &mut meta,
        ImportStepPostProcess,
        &StepSummary::default(),
        Some(PostProcessSummaryInput {
            encoded_row_counts: &[1],
            conflict_metas: &[conflict],
        }),
    )
    .unwrap();
    assert_eq!(meta.Summary.ImportedRows, -2);
    assert_eq!(meta.Summary.ConflictRowCnt, 5);
}

#[test]
fn failed_mode_switch_still_throttles_and_normal_switch_clears_timestamp() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{ImportSchedulerRuntime, TaskRegistration, importScheduler};
    use astersql_dxf_framework_proto::{
        ExtraParams, ImportStepImport, ModifyParam, NormalPriority, Task, TaskBase,
        TaskStatePending, TaskTypeExample,
    };
    use astersql_errors::{New, SharedError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    struct Runtime(AtomicUsize);
    impl ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _task_id: i64,
            _ttl: Duration,
        ) -> Result<Box<dyn TaskRegistration>, SharedError> {
            Err(New("registration unused"))
        }
        fn switch_to_import_mode(&self) -> Result<(), SharedError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(New("import switch failed"))
        }
        fn switch_to_normal_mode(&self) -> Result<(), SharedError> {
            Err(New("normal switch failed"))
        }
    }

    let runtime = Arc::new(Runtime(AtomicUsize::new(0)));
    let task = Task {
        TaskBase: TaskBase {
            ID: 1,
            Key: String::new(),
            Type: TaskTypeExample,
            State: TaskStatePending,
            Step: ImportStepImport,
            Priority: NormalPriority,
            RequiredSlots: 0,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: TaskMeta::default().Marshal().unwrap(),
        Error: None,
        ModifyParam: ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    };
    let scheduler = importScheduler::new(runtime.clone(), &task).unwrap();
    scheduler.switchTiKVMode(&task);
    scheduler.switchTiKVMode(&task);
    assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    scheduler.switchTiKV2NormalMode(&task);
    scheduler.switchTiKVMode(&task);
    assert_eq!(runtime.0.load(Ordering::SeqCst), 2);
    scheduler.Close(task.ID);
}

#[test]
fn scheduler_close_releases_local_registration_without_revoking_lease() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{ImportSchedulerRuntime, TaskRegistration, importScheduler};
    use astersql_dxf_framework_proto::{
        ExtraParams, ModifyParam, NormalPriority, StepInit, Task, TaskBase, TaskStatePending,
        TaskTypeExample,
    };
    use astersql_errors::{New, SharedError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    struct Registration {
        closed: Arc<AtomicUsize>,
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl TaskRegistration for Registration {
        fn register_once(&mut self, _: Duration) -> Result<(), SharedError> {
            Ok(())
        }
        fn close(&mut self, _: Duration) -> Result<(), SharedError> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    struct Runtime {
        closed: Arc<AtomicUsize>,
        dropped: Arc<AtomicUsize>,
    }
    impl ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _: i64,
            _: Duration,
        ) -> Result<Box<dyn TaskRegistration>, SharedError> {
            Ok(Box::new(Registration {
                closed: self.closed.clone(),
                dropped: self.dropped.clone(),
            }))
        }
        fn switch_to_import_mode(&self) -> Result<(), SharedError> {
            Err(New("unused"))
        }
        fn switch_to_normal_mode(&self) -> Result<(), SharedError> {
            Err(New("unused"))
        }
    }

    let closed = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let task = Task {
        TaskBase: TaskBase {
            ID: 279,
            Key: String::new(),
            Type: TaskTypeExample,
            State: TaskStatePending,
            Step: StepInit,
            Priority: NormalPriority,
            RequiredSlots: 0,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: TaskMeta::default().Marshal().unwrap(),
        Error: None,
        ModifyParam: ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    };
    let scheduler = importScheduler::new(
        Arc::new(Runtime {
            closed: closed.clone(),
            dropped: dropped.clone(),
        }),
        &task,
    )
    .unwrap();
    scheduler.registerTask(task.ID);

    scheduler.Close(task.ID);

    assert_eq!(closed.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    scheduler.Close(task.ID);
}

#[test]
fn scheduler_framework_task_bridge_keeps_import_state_and_modifications() {
    use crate::scheduler::frameworkTaskToImportTask;
    use astersql_dxf_framework_scheduler as framework;
    let mut task = framework::Task::default();
    task.base.id = 41;
    task.base.task_type = "ImportInto".into();
    task.base.step = astersql_dxf_framework_proto::ImportStepWriteAndIngest;
    task.base.required_slots = 8;
    task.base.keyspace = "user_keyspace".into();
    task.base.extra_params.max_runtime_slots = 3;
    task.base.extra_params.target_steps = vec![task.base.step];
    task.meta = br#"{"JobID":41}"#.to_vec();
    task.modifications.push(framework::Modification {
        kind: "modify_concurrency".into(),
        to: 6,
    });
    let mapped = frameworkTaskToImportTask(&task).unwrap();
    assert_eq!(mapped.ID, 41);
    assert_eq!(mapped.Step, task.base.step);
    assert_eq!(mapped.GetRuntimeSlots(), 3);
    assert_eq!(mapped.Keyspace, "user_keyspace");
    assert_eq!(mapped.Meta, task.meta);
    assert_eq!(
        mapped.ModifyParam.Modifications[0].Type,
        "modify_concurrency"
    );
    assert_eq!(mapped.ModifyParam.Modifications[0].To, 6);
    task.modifications[0].kind = "unknown".into();
    assert!(frameworkTaskToImportTask(&task).is_err());
}

#[test]
fn production_import_job_sql_adapter_runs_start_job_with_typed_bindings() {
    use crate::scheduler::ImportJobSqlSession;
    use astersql_executor_importer::StartJob;
    use astersql_util_sqlexec as sql;
    use std::any::Any;

    #[derive(Default)]
    struct Session {
        sql: String,
        args: Vec<String>,
    }
    impl sql::SQLExecutor for Session {
        fn Execute(
            &mut self,
            _ctx: &sql::context::Context,
            _sql: &str,
        ) -> Result<Vec<Box<dyn sql::RecordSet>>, sql::GoError> {
            unreachable!()
        }
        fn ExecuteInternal(
            &mut self,
            _ctx: &sql::context::Context,
            statement: &str,
            args: Vec<Box<dyn Any>>,
        ) -> Result<Option<Box<dyn sql::RecordSet>>, sql::GoError> {
            self.sql = statement.into();
            self.args = args
                .iter()
                .map(|arg| {
                    if let Some(value) = arg.downcast_ref::<String>() {
                        value.clone()
                    } else if let Some(value) = arg.downcast_ref::<i64>() {
                        value.to_string()
                    } else {
                        "unexpected binding".into()
                    }
                })
                .collect();
            Ok(None)
        }
        fn ExecuteStmt(
            &mut self,
            _ctx: &sql::context::Context,
            _stmt: sql::ast::NodeRef,
        ) -> Result<Option<Box<dyn sql::RecordSet>>, sql::GoError> {
            unreachable!()
        }
    }

    let mut session = Session::default();
    let mut adapter = ImportJobSqlSession {
        context: sql::context::Context::new(),
        executor: &mut session,
    };
    StartJob(&mut adapter, 41, "importing").unwrap();
    assert!(session.sql.contains("UPDATE mysql.tidb_import_jobs"));
    assert_eq!(session.args, ["running", "importing", "41", "pending"]);
}

#[test]
fn import_job_storage_session_uses_bound_backend_and_returns_session() {
    use crate::scheduler::withImportJobSession;
    use astersql_dxf_framework_storage as storage;
    use astersql_executor_importer::StartJob;
    use std::sync::{Arc, Mutex};

    struct Backend(Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push((sql.into(), args));
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
    }
    let statements = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let statements = statements.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(statements.clone()),
            )))
        }
    }));
    withImportJobSession(&manager, |executor| StartJob(executor, 41, "importing")).unwrap();
    let records = statements.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].0.contains("UPDATE mysql.tidb_import_jobs"));
    assert_eq!(
        records[0].1,
        [
            storage::Value::String("running".into()),
            storage::Value::String("importing".into()),
            storage::Value::Int(41),
            storage::Value::String("pending".into()),
        ]
    );
}

#[test]
fn import_job_session_retry_recovers_and_cancel_interrupts_backoff() {
    use crate::scheduler::withImportJobSessionRetry;
    use astersql_dxf_framework_scheduler::Context;
    use astersql_dxf_framework_storage as storage;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Backend(Arc<AtomicUsize>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            _: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            let attempt = self.0.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(storage::Error::new("transient"));
            }
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
    }
    let attempts = Arc::new(AtomicUsize::new(0));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let attempts = attempts.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(attempts.clone()),
            )))
        }
    }));
    withImportJobSessionRetry(&Context::default(), &manager, |executor| {
        astersql_executor_importer::StartJob(executor, 41, "importing")
    })
    .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    attempts.store(0, Ordering::SeqCst);
    let context = Context::default();
    context.cancel();
    let error = withImportJobSessionRetry(&context, &manager, |executor| {
        astersql_executor_importer::StartJob(executor, 41, "importing")
    })
    .unwrap_err();
    assert!(error.to_string().contains("context canceled"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn production_classic_table_empty_check_uses_transactional_rows() {
    use crate::proto::TaskMeta;
    use crate::scheduler::ProductionCheckImportTableEmpty;
    use astersql_dxf_framework_storage as storage;
    use astersql_executor_importer as importer;
    use astersql_meta_model as model;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Backend(Arc<AtomicBool>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            let rows = if sql.starts_with("select 1 from") && self.0.load(Ordering::SeqCst) {
                vec![storage::chunk::Row::default()]
            } else {
                vec![]
            };
            Ok(storage::SQLResult {
                rows,
                affected_rows: 1,
            })
        }
    }
    let occupied = Arc::new(AtomicBool::new(false));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let occupied = occupied.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(occupied.clone()),
            )))
        }
    }));
    let mut table = model::TableInfo::default();
    table.Name.O = "t`x".into();
    let meta = TaskMeta {
        Plan: importer::Plan {
            DBName: "d`b".into(),
            TableInfo: Some(Arc::new(table)),
            ..Default::default()
        },
        ..Default::default()
    };
    let check = ProductionCheckImportTableEmpty(manager);
    check(&meta).unwrap();
    occupied.store(true, Ordering::SeqCst);
    assert!(
        check(&meta)
            .unwrap_err()
            .to_string()
            .contains("target table is not empty")
    );
}

#[test]
fn production_stats_flush_uses_txn_timestamp_and_imported_rows() {
    use crate::proto::TaskMeta;
    use crate::scheduler::FlushImportStatsProduction;
    use astersql_dxf_framework_storage as storage;
    use astersql_executor_importer as importer;
    use astersql_meta_model as model;
    use std::sync::{Arc, Mutex};

    struct Backend(Arc<Mutex<Vec<String>>>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push(sql.into());
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
        fn txn_start_ts(&self) -> Result<u64, storage::Error> {
            Ok(987)
        }
    }
    let statements = Arc::new(Mutex::new(Vec::new()));
    let session = storage::sessionctx::Context::with_backend(Arc::new(Backend(statements.clone())));
    let mut table = model::TableInfo::default();
    table.ID = 42;
    let meta = TaskMeta {
        Plan: importer::Plan {
            TableInfo: Some(Arc::new(table)),
            ..Default::default()
        },
        Summary: importer::Summary {
            ImportedRows: 7,
            ..Default::default()
        },
        ..Default::default()
    };
    FlushImportStatsProduction(&session, &meta).unwrap();
    let statements = statements.lock().unwrap();
    assert!(statements[0].contains("stats_meta") && statements[0].contains("for update"));
    assert!(statements[1].contains("(987,42,7,7)"));
}

#[test]
fn user_keyspace_import_job_uses_system_session_pool() {
    use crate::scheduler::{
        GetImportJobTaskManager, ImportSchedulerRuntime, TaskRegistration, withImportJobSession,
    };
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Backend(Arc<AtomicUsize>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            _: &str,
            _: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
    }
    struct Runtime {
        user: bool,
        pool: Option<storage::util::SessionPool>,
    }
    impl ImportSchedulerRuntime for Runtime {
        fn is_user_keyspace(&self) -> bool {
            self.user
        }
        fn system_session_pool(&self) -> Option<storage::util::SessionPool> {
            self.pool.clone()
        }
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
    let fallback_calls = Arc::new(AtomicUsize::new(0));
    let system_calls = Arc::new(AtomicUsize::new(0));
    let fallback = storage::NewTaskManager(storage::util::SessionPool::new(
        storage::sessionctx::Context::with_backend(Arc::new(Backend(fallback_calls.clone()))),
    ));
    let runtime = Runtime {
        user: true,
        pool: Some(storage::util::SessionPool::new(
            storage::sessionctx::Context::with_backend(Arc::new(Backend(system_calls.clone()))),
        )),
    };
    let selected = GetImportJobTaskManager(&runtime, &fallback).unwrap();
    withImportJobSession(&selected, |executor| {
        astersql_executor_importer::StartJob(executor, 41, "importing")
    })
    .unwrap();
    assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    assert_eq!(system_calls.load(Ordering::SeqCst), 1);
    let runtime = Runtime {
        user: true,
        pool: None,
    };
    assert!(
        GetImportJobTaskManager(&runtime, &fallback)
            .err()
            .unwrap()
            .to_string()
            .contains("cross keyspace")
    );
}

#[test]
fn registered_import_scheduler_reuses_encode_runtime_services() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{
        ImportSchedulerRuntime, ImportSchedulerServices,
        RegisterImportSchedulerFactoryWithServices, TaskRegistration,
    };
    use astersql_dxf_framework_scheduler as framework;
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
    use astersql_executor_importer as importer;
    use astersql_meta_model as model;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

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
    struct UnusedManager;
    struct Handle;
    impl framework::TaskHandle for Handle {
        fn previous_subtask_metas(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            unreachable!()
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            unreachable!()
        }
    }
    impl framework::TaskManager for UnusedManager {
        fn top_unfinished_tasks(&self) -> framework::Result<Vec<framework::TaskBase>> {
            unreachable!()
        }
        fn top_no_need_resource_tasks(&self) -> framework::Result<Vec<framework::TaskBase>> {
            unreachable!()
        }
        fn all_tasks(&self) -> framework::Result<Vec<framework::TaskBase>> {
            unreachable!()
        }
        fn all_subtasks(&self) -> framework::Result<Vec<framework::SubtaskBase>> {
            unreachable!()
        }
        fn tasks_in_states(
            &self,
            _: &[framework::TaskState],
        ) -> framework::Result<Vec<framework::Task>> {
            unreachable!()
        }
        fn task_cleanup_info_by_ids(
            &self,
            _: &[i64],
        ) -> framework::Result<
            std::collections::HashMap<i64, astersql_dxf_framework_storage::TaskCleanupInfo>,
        > {
            unreachable!()
        }
        fn task_by_id(&self, _: i64) -> framework::Result<framework::Task> {
            unreachable!()
        }
        fn task_base_by_id(&self, _: i64) -> framework::Result<framework::TaskBase> {
            unreachable!()
        }
        fn all_nodes(&self) -> framework::Result<Vec<framework::ManagedNode>> {
            unreachable!()
        }
        fn delete_dead_nodes(&self, _: &[String]) -> framework::Result<()> {
            unreachable!()
        }
        fn transfer_tasks_to_history(&self, _: &[framework::Task]) -> framework::Result<()> {
            unreachable!()
        }
        fn gc_subtasks(&self) -> framework::Result<()> {
            unreachable!()
        }
        fn fail_task(
            &self,
            _: i64,
            _: framework::TaskState,
            _: framework::SchedulerError,
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn revert_task(
            &self,
            _: i64,
            _: framework::TaskState,
            _: framework::SchedulerError,
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn awaiting_resolve_task(
            &self,
            _: i64,
            _: framework::TaskState,
            _: framework::SchedulerError,
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn reverted_task(&self, _: i64) -> framework::Result<()> {
            unreachable!()
        }
        fn paused_task(&self, _: i64) -> framework::Result<()> {
            unreachable!()
        }
        fn pause_task_on_error(
            &self,
            _: i64,
            _: framework::TaskState,
            _: framework::Step,
            _: framework::SchedulerError,
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn resumed_task(&self, _: i64) -> framework::Result<()> {
            unreachable!()
        }
        fn modified_task(&self, _: &framework::Task) -> framework::Result<()> {
            unreachable!()
        }
        fn succeed_task(&self, _: i64) -> framework::Result<()> {
            unreachable!()
        }
        fn switch_task_step(
            &self,
            _: &framework::Task,
            _: framework::TaskState,
            _: framework::Step,
            _: &[framework::Subtask],
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn switch_task_step_in_batch(
            &self,
            _: &framework::Task,
            _: framework::TaskState,
            _: framework::Step,
            _: &[framework::Subtask],
        ) -> framework::Result<()> {
            unreachable!()
        }
        fn switch_task_step_after_prepare(&self, _: &framework::Task) -> framework::Result<bool> {
            unreachable!()
        }
        fn used_slots_on_nodes(&self) -> framework::Result<HashMap<String, i32>> {
            unreachable!()
        }
        fn active_subtasks(&self, _: i64) -> framework::Result<Vec<framework::SubtaskBase>> {
            unreachable!()
        }
        fn subtask_count_by_states(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<HashMap<framework::SubtaskState, i64>> {
            unreachable!()
        }
        fn subtask_errors(&self, _: i64) -> framework::Result<Vec<framework::SchedulerError>> {
            unreachable!()
        }
        fn resume_subtasks(&self, _: i64) -> framework::Result<()> {
            unreachable!()
        }
        fn update_subtask_exec_ids(&self, _: &[framework::SubtaskBase]) -> framework::Result<()> {
            unreachable!()
        }
        fn previous_subtask_metas(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            unreachable!()
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            unreachable!()
        }
    }
    let encode = Arc::new(crate::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(astersql_objstore::azblob::MemoryStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let mut services = ImportSchedulerServices::FromEncodeRuntime(
        encode.clone(),
        vec![],
        Arc::new(|_, _| crate::planner::PlanCtx::default()),
    );
    let table_info = Arc::new(model::TableInfo {
        ID: 42,
        Name: model::ast::NewCIStr("import_table"),
        ..Default::default()
    });
    let table = table_info.clone();
    services.Table = Some(Arc::new(move || {
        Arc::new(astersql_planner_core_operator_physicalop::MetadataTableAdapter::New(&table))
            as Arc<dyn astersql_table::Table>
    }));
    services.CheckImportTableEmpty = Some(Arc::new(|_| Ok(())));
    let services = Arc::new(services);
    let (_domain, manager) = real_pending_import_job_manager(41);
    RegisterImportSchedulerFactoryWithServices(Arc::new(Runtime), manager, services);
    let factory =
        framework::get_scheduler_factory(astersql_dxf_framework_proto::ImportInto).unwrap();
    let mut task = framework::Task::default();
    task.base.id = 41;
    task.base.task_type = astersql_dxf_framework_proto::ImportInto.into();
    task.meta = TaskMeta {
        JobID: 41,
        Plan: importer::Plan {
            DBName: "test".into(),
            TableInfo: Some(table_info),
            ..Default::default()
        },
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "data.csv".into(),
                FileSize: 10,
                EndOffset: 10,
                ..Default::default()
            }],
        )]),
        ..Default::default()
    }
    .Marshal()
    .unwrap();
    let param = framework::Param {
        task_manager: Arc::new(UnusedManager),
        node_manager: Arc::new(framework::NodeManager::new()),
        slot_manager: Arc::new(framework::SlotManager::new()),
        server_id: "test".into(),
        allocated_slots: true,
        node_resource: None,
    };
    let scheduler = factory(task.clone(), param);
    scheduler.init().unwrap();
    let mut task = task;
    let batch = scheduler
        .extension()
        .on_next_subtasks_batch(
            &Handle,
            &mut task,
            &["node-1".into()],
            astersql_dxf_framework_proto::ImportStepImport,
        )
        .unwrap();
    assert_eq!(batch.len(), 1);
    scheduler.close();
}

#[test]
fn import_job_json_codec_round_trips_go_named_fields() {
    use crate::scheduler::ImportJobJsonCodec;
    use astersql_executor_importer::{ImportJobCodec, ImportParameters, StepSummary, Summary};
    let codec = ImportJobJsonCodec;
    let parameters = ImportParameters {
        FileLocation: "s3://bucket/file.csv".into(),
        Format: "csv".into(),
        ..Default::default()
    };
    let encoded = codec.EncodeParameters(&parameters).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["file-location"], "s3://bucket/file.csv");
    assert!(value.get("columns-and-vars").is_none());
    assert!(value.get("options").is_none());
    assert_eq!(codec.DecodeParameters(&encoded).unwrap(), parameters);
    let summary = Summary {
        EncodeSummary: StepSummary {
            Bytes: 10,
            RowCnt: 4,
        },
        ImportedRows: 3,
        ConflictRowCnt: 1,
        ..Default::default()
    };
    let encoded = codec.EncodeSummary(&summary).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["encode-summary"]["input-bytes"], 10);
    assert_eq!(value["row-count"], 3);
    assert!(value.get("too-many-conflicts").is_none());
    assert_eq!(codec.DecodeSummary(&encoded).unwrap(), summary);
}

#[test]
fn import_done_callback_finishes_job_in_transaction_on_bound_backend() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{
        ImportSchedulerRuntime, TaskRegistration, doneImportTask, importScheduler,
    };
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
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
    struct Backend(Arc<Mutex<Vec<String>>>);
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
    }
    let statements = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let statements = statements.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(statements.clone()),
            )))
        }
    }));
    let meta = TaskMeta {
        JobID: 41,
        ..Default::default()
    };
    let mut task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: 41,
            Key: String::new(),
            Type: proto::ImportInto,
            State: proto::TaskStateSucceed,
            Step: proto::ImportStepPostProcess,
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
    doneImportTask(
        &astersql_dxf_framework_scheduler::Context::default(),
        &scheduler,
        &manager,
        &task,
        &|_, _| Ok(()),
    )
    .unwrap();
    {
        let statements = statements.lock().unwrap();
        assert_eq!(statements.len(), 3);
        assert_eq!(statements[0], "begin");
        assert!(statements[1].contains("UPDATE mysql.tidb_import_jobs"));
        assert!(statements[1].contains("status = %?"));
        assert_eq!(statements[2], "commit");
    }
    task.State = proto::TaskStateReverting;
    task.Error = Some("worker failed".into());
    doneImportTask(
        &astersql_dxf_framework_scheduler::Context::default(),
        &scheduler,
        &manager,
        &task,
        &|_, _| unreachable!(),
    )
    .unwrap();
    task.Error = Some(astersql_dxf_framework_scheduler::TASK_CANCEL_MESSAGE.into());
    doneImportTask(
        &astersql_dxf_framework_scheduler::Context::default(),
        &scheduler,
        &manager,
        &task,
        &|_, _| unreachable!(),
    )
    .unwrap();
    {
        let statements = statements.lock().unwrap();
        assert_eq!(statements.len(), 5);
        assert!(statements[3].contains("end_time = CURRENT_TIMESTAMP(6)"));
        assert!(statements[4].contains("error_message = 'cancelled by user'"));
    }
    scheduler.Close(task.ID);
}

#[test]
fn import_extension_dispatches_done_and_prepared_local_import_batches() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{
        ImportSchedulerExtension, ImportSchedulerRuntime, ImportSchedulerServices,
        TaskRegistration, frameworkTaskToImportTask, importScheduler,
    };
    use astersql_dxf_framework_scheduler as framework;
    use astersql_dxf_framework_storage as storage;
    use astersql_errors::{New, SharedError};
    use astersql_executor_importer as importer;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

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
    struct Handle;
    impl framework::TaskHandle for Handle {
        fn previous_subtask_metas(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            unreachable!()
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            unreachable!()
        }
    }
    let mut task = framework::Task::default();
    task.base.id = 1_000_000_041;
    task.base.task_type = "ImportInto".into();
    task.meta = TaskMeta {
        JobID: 41,
        Plan: importer::Plan {
            TotalFileSize: 10,
            ..Default::default()
        },
        ChunkMap: HashMap::from([(
            1,
            vec![importer::Chunk {
                Path: "data.csv".into(),
                FileSize: 10,
                EndOffset: 10,
                ..Default::default()
            }],
        )]),
        ..Default::default()
    }
    .Marshal()
    .unwrap();
    let imported = frameworkTaskToImportTask(&task).unwrap();
    let scheduler = Arc::new(importScheduler::new(Arc::new(Runtime), &imported).unwrap());
    let bytes_counter = scheduler
        .metrics
        .bytes_counter
        .with_label_values(&[astersql_lightning_metric::STATE_TOTAL_RESTORE]);
    let initial_bytes = bytes_counter.get();
    let services = Arc::new(ImportSchedulerServices {
        ResourceCalculatorWithContext: None,
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        KVCodec: vec![],
        Table: None,
        SortStore: None,
        CheckImportTableEmpty: Some(Arc::new(|_| Ok(()))),
        PlanContext: Arc::new(|_, _| crate::planner::PlanCtx::default()),
        FlushStatsBestEffort: Some(Arc::new(|_, _| unreachable!())),
    });
    let (_domain, manager) = real_pending_import_job_manager(41);
    let extension = ImportSchedulerExtension {
        scheduler: scheduler.clone(),
        manager,
        services,
    };
    let metas = framework::Extension::on_next_subtasks_batch(
        &extension,
        &Handle,
        &mut task,
        &[],
        framework::STEP_DONE,
    )
    .unwrap();
    assert!(metas.is_empty());
    let metas = framework::Extension::on_next_subtasks_batch(
        &extension,
        &Handle,
        &mut task,
        &["node-1".into()],
        astersql_dxf_framework_proto::ImportStepImport,
    )
    .unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(bytes_counter.get(), initial_bytes + 10.0);
    assert_eq!(TaskMeta::Unmarshal(&task.meta).unwrap().ChunkMap.len(), 1);
    assert_eq!(task.base.id, 1_000_000_041);
    scheduler.Close(1_000_000_041);
}

#[test]
fn import_scheduler_eligible_instances_and_step_matrix_match_go() {
    use crate::proto::{ServerInfo, TaskMeta};
    use crate::scheduler::{
        ImportSchedulerRuntime, TaskRegistration, frameworkTaskToImportTask, importScheduler,
    };
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_scheduler as framework;
    use astersql_errors::{New, SharedError};
    use std::sync::Arc;
    use std::time::Duration;
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
    let mut task = framework::Task::default();
    task.base.id = 41;
    task.base.task_type = "ImportInto".into();
    task.meta = TaskMeta {
        EligibleInstances: vec![
            ServerInfo {
                ip: "1.1.1.1".into(),
                listening_port: 4000,
                ..Default::default()
            },
            ServerInfo {
                ip: "2001:db8::1".into(),
                listening_port: 4000,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
    .Marshal()
    .unwrap();
    let imported = frameworkTaskToImportTask(&task).unwrap();
    let scheduler = importScheduler::new(Arc::new(Runtime), &imported).unwrap();
    assert_eq!(
        scheduler.GetEligibleInstances(&imported).unwrap(),
        ["1.1.1.1:4000", "[2001:db8::1]:4000"]
    );
    let mut base = task.base.clone();
    for next in [
        proto::ImportStepImport,
        proto::ImportStepPostProcess,
        proto::StepDone,
    ] {
        let imported = frameworkTaskToImportTask(&framework::Task {
            base: base.clone(),
            ..task.clone()
        })
        .unwrap();
        assert_eq!(scheduler.GetNextStep(&imported.TaskBase), next);
        base.step = next;
    }
    scheduler.Close(41);
}

#[test]
pub(crate) fn prepare_import_task_persists_real_file_controller_result() {
    use crate::proto::TaskMeta;
    use crate::scheduler::{ImportSchedulerServices, prepareImportTask};
    use astersql_dxf_framework_proto as proto;
    use astersql_dxf_framework_storage as storage;
    use astersql_executor_importer as importer;
    use astersql_lightning_mydump as mydump;
    use astersql_meta_model as model;
    use astersql_objstore_storeapi as storeapi;
    use astersql_planner_core_operator_physicalop::MetadataTableAdapter;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    #[derive(Clone)]
    struct ControllerServices {
        files: astersql_objstore::azblob::MemoryStorage,
    }
    impl importer::ColumnAssignmentFactory for ControllerServices {
        fn BuildAssignment(
            &self,
            _: &astersql_parser_ast::Assignment,
        ) -> Result<Arc<dyn importer::ColAssignExpressionBuilder>, String> {
            unreachable!()
        }
    }
    impl importer::ImportDatumConverter for ControllerServices {
        fn CastColumnValue(
            &self,
            _: astersql_lightning_backend_encode::Datum,
            _: &astersql_table::Column,
        ) -> Result<astersql_lightning_backend_encode::Datum, String> {
            unreachable!()
        }
        fn CurrentTime(
            &self,
            _: &astersql_table::Column,
        ) -> Result<astersql_lightning_backend_encode::Datum, String> {
            unreachable!()
        }
    }
    impl importer::ImportParserFactory for ControllerServices {
        fn NewParser(
            &self,
            _: &str,
            _: Box<dyn mydump::ReadSeekCloser>,
            _: &mydump::SourceFileMeta,
            _: &importer::Plan,
        ) -> Result<Box<dyn mydump::Parser>, String> {
            unreachable!()
        }
    }
    impl importer::ImportSizeEstimator for ControllerServices {
        fn EstimateRealSize(
            &self,
            _: &storeapi::Context,
            file: &mydump::SourceFileMeta,
            _: &dyn storeapi::Storage,
        ) -> Result<i64, String> {
            Ok(file.file_size)
        }
        fn ParquetExpansionRatio(
            &self,
            _: &storeapi::Context,
            _: &str,
            _: i64,
            _: &dyn storeapi::Storage,
        ) -> Result<f64, String> {
            Ok(1.0)
        }
    }
    impl importer::ImportStorageFactory for ControllerServices {
        fn Open(
            &self,
            _: &storeapi::Context,
            _: &str,
            _: &str,
        ) -> Result<importer::SharedStorage, String> {
            Ok(Arc::new(Mutex::new(Box::new(self.files.clone()))))
        }
    }
    impl importer::TiKVConfigProbe for ControllerServices {
        fn IsRaftKV2(&self) -> Result<bool, String> {
            Ok(false)
        }
    }
    impl importer::ImportResourceCalculator for ControllerServices {
        fn TargetNodeCPUCnt(&self) -> Result<usize, String> {
            Ok(2)
        }
        fn ScheduleTuneFactors(&self, _: &str) -> Result<importer::ScheduleTuneFactors, String> {
            Ok(importer::ScheduleTuneFactors::default())
        }
        fn SampleIndexSizeRatio(
            &self,
            _: &importer::LoadDataController,
            _: &[u8],
        ) -> Result<f64, String> {
            Ok(0.0)
        }
        fn Calculate(
            &self,
            _: i64,
            _: usize,
            _: f64,
            _: importer::ScheduleTuneFactors,
        ) -> importer::ResourceParams {
            importer::ResourceParams {
                ThreadCnt: 2,
                MaxNodeCnt: 1,
                ..Default::default()
            }
        }
    }
    struct Regions;
    impl importer::TableImporterService for Regions {
        fn RuntimeConfig(&self) -> importer::ImportRuntimeConfig {
            unreachable!()
        }
        fn NewEncodingTable(
            &self,
            _: &importer::LoadDataController,
        ) -> Result<Arc<dyn astersql_lightning_backend_encode::Table>, String> {
            unreachable!()
        }
        fn NewBackend(
            &self,
            _: &importer::LoadDataController,
            _: &std::path::Path,
        ) -> Result<Arc<dyn astersql_lightning_backend::Backend>, String> {
            unreachable!()
        }
        fn RegionSplitSizeKeys(&self) -> Result<(i64, i64), String> {
            unreachable!()
        }
        fn NewParser(
            &self,
            _: &importer::LoadDataController,
            _: &importer::Chunk,
        ) -> Result<Box<dyn mydump::Parser + Send>, String> {
            unreachable!()
        }
        fn EstimateParquetReaderMemory(
            &self,
            _: &importer::LoadDataController,
            _: &str,
        ) -> Result<i64, String> {
            unreachable!()
        }
        fn MakeTableRegions(
            &self,
            controller: &importer::LoadDataController,
            _: i64,
        ) -> Result<Vec<importer::TableRegion>, String> {
            assert_eq!(controller.DataFiles().len(), 1);
            Ok(vec![importer::TableRegion {
                EngineID: 1,
                File: controller.DataFiles()[0].clone(),
                Offset: 0,
                EndOffset: 2,
                RowIDMax: 1,
                ..Default::default()
            }])
        }
        fn EstimateCompactionThreshold(&self, _: i64) -> i64 {
            unreachable!()
        }
        fn ImportedKVCount(&self, _: &astersql_lightning_backend::ClosedEngine) -> i64 {
            unreachable!()
        }
        fn DiskCapacity(&self, _: &std::path::Path) -> Result<u64, String> {
            unreachable!()
        }
        fn CheckDiskQuota(
            &self,
            _: &dyn astersql_lightning_backend::Backend,
            _: i64,
        ) -> importer::DiskQuotaState {
            unreachable!()
        }
        fn FlushAndImportLargeEngines(
            &self,
            _: &dyn astersql_lightning_backend::Backend,
            _: &[i32],
        ) -> Result<(), String> {
            unreachable!()
        }
        fn RebaseAllocatorBases(
            &self,
            _: &std::collections::HashMap<astersql_lightning_backend_kv::AllocatorType, i64>,
            _: &importer::Plan,
        ) -> Result<(), String> {
            unreachable!()
        }
        fn RemoteChecksumTableBySQL(
            &self,
            _: &importer::Plan,
            _: usize,
            _: i32,
        ) -> Result<importer::RemoteChecksum, importer::RemoteChecksumError> {
            unreachable!()
        }
        fn FlushTableStats(&self, _: i64, _: i64) -> Result<(), String> {
            unreachable!()
        }
        fn AllocatorMaximums(
            &self,
        ) -> std::collections::HashMap<astersql_lightning_backend_kv::AllocatorType, i64> {
            unreachable!()
        }
    }

    let directory = std::env::temp_dir().join(format!(
        "astersql-scheduler-prepare-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir(&directory).unwrap();
    let data_file = directory.join("data.csv");
    std::fs::write(&data_file, b"1\n").unwrap();
    let files = astersql_objstore::azblob::MemoryStorage::default();
    let controller_services = Arc::new(ControllerServices { files });
    let make_services = {
        let controller_services = controller_services.clone();
        move || importer::LoadDataControllerServices {
            DatumConverter: controller_services.clone(),
            AssignmentFactory: controller_services.clone(),
            ParserFactory: controller_services.clone(),
            SizeEstimator: Arc::new(importer::HostImportSizeEstimator {
                ParquetEstimator: controller_services.clone(),
            }),
            StorageFactory: Arc::new(importer::HostImportStorageFactory {
                CloudFactory: controller_services.clone(),
            }),
            TiKVConfigProbe: controller_services.clone(),
            ResourceCalculator: controller_services.clone(),
        }
    };
    let sort_store = Arc::new(astersql_objstore::memstore::NewMemStorage());
    let mut metadata = model::TableInfo::default();
    metadata.Name = model::ast::NewCIStr("t");
    metadata.Columns = (0..1)
        .map(|index| model::ColumnInfo {
            ID: index + 1,
            Name: model::ast::NewCIStr("id"),
            Offset: index as isize,
            State: model::StatePublic,
            ..Default::default()
        })
        .collect();
    let table_info = Arc::new(metadata);
    let table = {
        let table_info = table_info.clone();
        Arc::new(move || {
            Arc::new(MetadataTableAdapter::New(&table_info)) as Arc<dyn astersql_table::Table>
        })
    };
    let services = Arc::new(ImportSchedulerServices {
        ResourceCalculatorWithContext: None,
        ControllerServices: Arc::new(make_services),
        ImporterService: Arc::new(|| {
            Arc::new(importer::HostTableImporterService {
                Host: Arc::new(Regions),
            })
        }),
        KVCodec: vec![],
        Table: Some(table),
        SortStore: Some(sort_store.clone()),
        CheckImportTableEmpty: Some(Arc::new(|_| Ok(()))),
        PlanContext: Arc::new(|_, _| crate::planner::PlanCtx::default()),
        FlushStatsBestEffort: Some(Arc::new(|_, _| Ok(()))),
    });
    let (_domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    manager.ExecuteSQLWithNewSession((),
        "INSERT INTO mysql.tidb_import_jobs(id,table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step) VALUES(%?,%?,%?,%?,%?,%?,%?,%?,%?)",
        vec![41_i64.into(), "db".into(), "t".into(), 8_i64.into(), "root@%".into(), r#"{"format":"auto","file-location":"file.csv"}"#.into(), 0_i64.into(), "pending".into(), "".into()],
    ).unwrap();
    let meta = TaskMeta {
        JobID: 41,
        Plan: importer::Plan {
            TableInfo: Some(table_info),
            Path: data_file.to_string_lossy().into_owned(),
            InImportInto: true,
            ..Default::default()
        },
        Stmt: format!("IMPORT INTO db.t FROM '{}'", data_file.display()),
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
    prepareImportTask(
        &astersql_dxf_framework_scheduler::Context::default(),
        &manager,
        &mut task,
        &services,
    )
    .unwrap();
    let updated = TaskMeta::Unmarshal(&task.Meta).unwrap();
    assert_eq!(updated.Plan.TotalFileSize, 2);
    assert_eq!(updated.Plan.Format, importer::DataFormatCSV);
    assert_eq!(task.RequiredSlots, 2);
    assert_eq!(task.MaxNodeCount, 1);
    assert_eq!(
        updated.PreparedMetaExternalPath,
        "41/plan/prepared/meta.json"
    );
    let encoded = astersql_objstore::storage::Storage::ReadFile(
        sort_store.as_ref(),
        &astersql_objstore::storage::Context::default(),
        &updated.PreparedMetaExternalPath,
    )
    .unwrap();
    assert_eq!(
        crate::proto::PreparedMeta::Unmarshal(&encoded)
            .unwrap()
            .ChunkMap[&0]
            .len(),
        1
    );

    use crate::scheduler::{
        ImportSchedulerExtension, ImportSchedulerRuntime, TaskRegistration,
        frameworkTaskToImportTask, importScheduler,
    };
    use astersql_dxf_framework_scheduler as framework;
    use astersql_errors::{New, SharedError};
    use std::time::Duration;
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
    struct Handle;
    impl framework::TaskHandle for Handle {
        fn previous_subtask_metas(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<Vec<u8>>> {
            unreachable!()
        }
        fn previous_subtask_summaries(
            &self,
            _: i64,
            _: framework::Step,
        ) -> framework::Result<Vec<framework::SubtaskSummary>> {
            unreachable!()
        }
    }
    let mut framework_task = framework::Task::default();
    framework_task.base.id = 42;
    framework_task.base.task_type = "ImportInto".into();
    framework_task.meta = meta.Marshal().unwrap();
    let imported = frameworkTaskToImportTask(&framework_task).unwrap();
    let scheduler = Arc::new(importScheduler::new(Arc::new(Runtime), &imported).unwrap());
    let extension = ImportSchedulerExtension {
        scheduler: scheduler.clone(),
        manager: manager.clone(),
        services: services.clone(),
    };
    framework::Extension::on_prepare(&extension, &Handle, &mut framework_task).unwrap();
    assert_eq!(framework_task.base.required_slots, 2);
    assert_eq!(framework_task.base.max_node_count, 1);
    assert_eq!(
        TaskMeta::Unmarshal(&framework_task.meta)
            .unwrap()
            .PreparedMetaExternalPath,
        "42/plan/prepared/meta.json"
    );
    scheduler.Close(42);

    let mut global_meta = meta.clone();
    global_meta.Plan.CloudStorageURI = "memstore://global-sort".into();
    framework_task.base.id = 43;
    framework_task.meta = global_meta.Marshal().unwrap();
    let imported = frameworkTaskToImportTask(&framework_task).unwrap();
    let scheduler = Arc::new(importScheduler::new(Arc::new(Runtime), &imported).unwrap());
    assert!(scheduler.GlobalSort);
    let extension = ImportSchedulerExtension {
        scheduler: scheduler.clone(),
        manager,
        services,
    };
    framework::Extension::on_prepare(&extension, &Handle, &mut framework_task).unwrap();
    let path = TaskMeta::Unmarshal(&framework_task.meta)
        .unwrap()
        .PreparedMetaExternalPath;
    assert_eq!(path, "43/plan/prepared/meta.json");
    assert!(
        astersql_objstore::storage::Storage::FileExists(
            sort_store.as_ref(),
            &astersql_objstore::storage::Context::default(),
            &path,
        )
        .unwrap()
    );
    scheduler.Close(43);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn scheduler_cancellation_reaches_both_import_object_store_contexts() {
    let scheduler = astersql_dxf_framework_scheduler::Context::default();
    let storeapi =
        astersql_objstore_storeapi::Context::from_cancellation_flag(scheduler.cancellation_flag());
    let sort_store =
        astersql_objstore::storage::Context::from_cancellation_flag(scheduler.cancellation_flag());
    assert!(!storeapi.is_cancelled());
    assert!(!sort_store.is_cancelled());
    scheduler.cancel();
    assert!(storeapi.is_cancelled());
    assert!(sort_store.is_cancelled());
    assert!(storeapi.check().is_err());
    assert!(sort_store.check_cancelled().is_err());
}

#[test]
fn scheduler_retryability_preserves_go_normalized_region_error_code() {
    use crate::scheduler::IsImportSchedulerRetryableError;
    use astersql_dxf_framework_scheduler::SchedulerError;
    let region = SchedulerError::new(astersql_store_driver_error::ErrRegionUnavailable.to_string());
    assert!(IsImportSchedulerRetryableError(&region));
    assert!(IsImportSchedulerRetryableError(&SchedulerError::new(
        "test: failed to get cross keyspace session pool",
    )));
    assert!(!IsImportSchedulerRetryableError(&SchedulerError::new(
        "target table is not empty",
    )));
}

/// global sort 应进入 EncodeAndSort；local sort 直接进入 Import。
#[test]
fn scheduler_selects_encode_step_from_sort_mode() {
    assert_eq!(getStepOfEncode(true), ImportStepEncodeAndSort);
    assert_eq!(getStepOfEncode(false), ImportStepImport);
}

/// Keep admission fixtures backed by the same canonical SQL/KV path as jobs.
pub(crate) fn real_pending_import_job_manager(
    job_id: i64,
) -> (
    std::sync::Arc<astersql_domain::Domain>,
    astersql_dxf_framework_storage::TaskManager,
) {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    manager.ExecuteSQLWithNewSession((),
        "INSERT INTO mysql.tidb_import_jobs(id,table_schema,table_name,table_id,created_by,parameters,source_file_size,status,step) VALUES(%?,%?,%?,%?,%?,%?,%?,%?,%?)",
        vec![job_id.into(), "test".into(), "t".into(), 8_i64.into(), "root@%".into(), r#"{"format":"auto","file-location":"file.csv"}"#.into(), 0_i64.into(), "pending".into(), "".into()],
    ).unwrap();
    (domain, manager)
}
