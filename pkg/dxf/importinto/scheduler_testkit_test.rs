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

// Import Into 调度器 testkit 场景草稿与可执行冒烟测试。
//
// `_GO_SCHEDULER_TESTKIT_DRAFT` 保留 local sort、prepare mode、取消复原表模式、
// global sort 状态流转与 job 状态更新等集成测试草稿；
// 可执行部分断言任务元数据可序列化，并在分发子任务前能解出一致的 TaskMeta。

const _GO_SCHEDULER_TESTKIT_DRAFT: &str = r###"
// 这段逻辑覆盖 local sort、prepare mode、取消时表模式复原、global sort 状态流转和 job 状态更新。

// importTestSessionPool 对应 Go struct，把 pools.ResourcePool 包装成 DestroyableSessionPool。
pub struct importTestSessionPool {
    pub ResourcePool: pools::ResourcePool,
}

impl importTestSessionPool {
    // Destroy 对应 Go 方法，测试 session pool 归还资源时直接 Close。
    pub fn Destroy(&self, resource: pools::Resource) {
        resource.Close();
    }
}

// new_import_test_runtime 对应 Go helper，为 scheduler/task executor 提供 mock runtime。
pub fn new_import_test_runtime(
    ctrl: &gomock::Controller,
    store: kv::Storage,
    sess_pool: Option<pools::ResourcePool>,
) -> sqlsvrapimock::MockRuntime {
    let destroyable = sess_pool.map(|pool| importTestSessionPool { ResourcePool: pool });
    let runtime = sqlsvrapimock::NewMockRuntime(ctrl);
    runtime.EXPECT().Store().Return(store).AnyTimes();
    runtime.EXPECT().SysSessionPool().Return(destroyable).AnyTimes();
    runtime
}

// test_scheduler_ext_local_sort 对应 Go 的 TestSchedulerExtLocalSort。
#[test]
pub fn test_scheduler_ext_local_sort() {
    let ctrl = gomock::NewController();
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, time::Second);
    let mut ctx = context::WithValue(context::Background(), "etcd", true);
    ctx = util::WithInternalSourceType(ctx, "taskManager");
    let mgr = storage::NewTaskManager(pool.clone());
    storage::SetTaskManager(mgr.clone());
    let sch = scheduler::NewManager(util::WithInternalSourceType(ctx.clone(), "scheduler"), store.clone(), mgr.clone(), "host:port", proto::NodeResourceForTest);

    // create job：Go 在系统表中创建 pending job，后续 OnNextSubtasksBatch 会推进状态。
    let conn = tk.Session().GetSQLExecutor();
    let mut job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", 1, "root", "", &importer::ImportParameters::default(), 123).expect("create job");
    let got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("get pending job");
    assert_eq!(got_job_info.Status, "pending");

    let mut logical_plan = importinto::LogicalPlan {
        JobID: job_id,
        Plan: importer::Plan {
            DBName: "test".to_owned(),
            TableInfo: Some(model::TableInfo { Name: ast::NewCIStr("t"), State: model::StatePublic, ..Default::default() }),
            DisableTiKVImportMode: true,
            ..Default::default()
        },
        Stmt: "IMPORT INTO db.tb FROM 'gs://test-load/*.csv?endpoint=xxx'".to_owned(),
        EligibleInstances: vec![serverinfo::ServerInfo { StaticInfo: serverinfo::StaticInfo { ID: "1".to_owned() } }],
        ChunkMap: hashmap! { 1_i32 => vec![importer::Chunk { Path: "gs://test-load/1.csv".to_owned() }] },
        ..Default::default()
    };
    let mut bs = logical_plan.ToTaskMeta().expect("task meta");
    let mut task = proto::Task {
        TaskBase: proto::TaskBase { Type: proto::TaskTypeExample, Step: proto::StepInit, State: proto::TaskStatePending, ..Default::default() },
        Meta: bs.clone(),
        StateUpdateTime: time::Now(),
        ..Default::default()
    };
    let manager = storage::GetTaskManager().expect("task manager");
    let task_id = manager.CreateTask(ctx.clone(), importinto::TaskKey(job_id), proto::ImportInto, "", 1, "", 1, proto::ExtraParams::default(), bs).expect("create task");
    task.ID = task_id;

    let d = sch.MockScheduler(&task);
    let ext = importinto::NewImportSchedulerForTest(false, &task, scheduler::NewParamForTest(manager.clone(), new_import_test_runtime(&ctrl, store.clone(), Some(pool.clone()))));
    let mut subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("import subtasks");
    assert_eq!(subtask_metas.len(), 1);
    let mut next_step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(next_step, proto::ImportStepImport);
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Status, "running");

    // update task/subtask, and finish subtask, so we can go to next stage.
    let subtasks = subtask_metas.iter().enumerate().map(|(i, m)| proto::NewSubtask(next_step, task.ID, task.Type, "", 1, m.clone(), i + 1)).collect();
    manager.SwitchTaskStep(ctx.clone(), &mut task, proto::TaskStateRunning, next_step, subtasks).expect("switch to import");
    task.Step = next_step;
    for subtask in manager.GetSubtasksWithHistory(ctx.clone(), task_id, proto::ImportStepImport).expect("history subtasks") {
        manager.FinishSubtask(ctx.clone(), subtask.ExecID, subtask.ID, b"{}".to_vec()).expect("finish import subtask");
    }

    // to post-process stage, job should be running and in validating step.
    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("post-process subtasks");
    assert_eq!(subtask_metas.len(), 1);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::ImportStepPostProcess);
    let got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("validating job");
    assert_eq!(got_job_info.Status, "running");
    assert_eq!(got_job_info.Step, "validating");

    // on next stage, job should be finished.
    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("done stage");
    assert_eq!(subtask_metas.len(), 0);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::StepDone);
    ext.OnDone(ctx.clone(), &d, &task).expect("done");
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Status, "finished");

    // create another job, fail it before start；Go 要求 init 阶段回滚也把 job 标成 failed。
    job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", 1, "root", "", &importer::ImportParameters::default(), 123).expect("create reverted job");
    logical_plan.JobID = job_id;
    bs = logical_plan.ToTaskMeta().expect("failed task meta");
    task.Meta = bs;
    task.Step = proto::StepInit;
    task.State = proto::TaskStateReverting;
    task.Error = errors::New("precheck failed");
    ext.OnDone(ctx.clone(), &d, &task).expect("precheck failed done");
    let got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("failed job");
    assert_eq!(got_job_info.Status, "failed");
    assert_eq!(importer::GetActiveJobCnt(ctx.clone(), conn.clone(), got_job_info.TableSchema, got_job_info.TableName).unwrap(), 0);

    // create another job, start it, and fail it.
    job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", 1, "root", "", &importer::ImportParameters::default(), 123).expect("create running failed job");
    logical_plan.JobID = job_id;
    task.Meta = logical_plan.ToTaskMeta().expect("running failed meta");
    importer::StartJob(ctx.clone(), conn.clone(), job_id, importer::JobStepImporting).expect("start job");
    task.State = proto::TaskStateReverting;
    task.Error = errors::New("met error");
    ext.OnDone(ctx.clone(), &d, &task).expect("running failed done");
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Status, "failed");

    // create another job, start it, and cancel it.
    job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", 1, "root", "", &importer::ImportParameters::default(), 123).expect("create cancelled job");
    logical_plan.JobID = job_id;
    task.Meta = logical_plan.ToTaskMeta().expect("cancelled meta");
    importer::StartJob(ctx.clone(), conn.clone(), job_id, importer::JobStepImporting).expect("start cancelled job");
    task.State = proto::TaskStateReverting;
    task.Error = errors::New("cancelled by user");
    ext.OnDone(ctx.clone(), &d, &task).expect("cancel done");
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Status, "cancelled");
    pool.Close();
}

// test_scheduler_prepare_enabled_job_transitions_from_preparing_to_first_business_phase 对应 prepare mode 测试。
#[test]
pub fn test_scheduler_prepare_enabled_job_transitions_from_preparing_to_first_business_phase() {
    let ctrl = gomock::NewController();
    if !kerneltype::IsNextGen() {
        // prepare mode only applies in nextgen kernel.
        return;
    }

    // fake-gcs-server 提供源文件和 sort bucket；保留 bucket/object/endpoint 参数，不启动真实 HTTP 服务。
    let host = "127.0.0.1";
    let opt = fakestorage::Options { Scheme: "http".to_owned(), Host: host.to_owned(), Port: 0, PublicHost: host.to_owned(), ..Default::default() };
    let server = fakestorage::NewServerWithOptions(opt).expect("fake gcs");
    let gcs_endpoint = format!("{}/storage/v1/", server.URL());
    let sort_storage_uri = format!("gs://sort-bucket/import?endpoint={}&access-key=aaaaaa&secret-access-key=bbbbbb", gcs_endpoint);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts { Name: "sort-bucket".to_owned(), ..Default::default() });
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts { Name: "test-load".to_owned(), ..Default::default() });
    server.CreateObject(fakestorage::Object { ObjectAttrs: fakestorage::ObjectAttrs { BucketName: "test-load".to_owned(), Name: "1.csv".to_owned(), ..Default::default() }, Content: b"1\n".to_vec() });

    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int)");
    let tbl = domain::GetDomain(tk.Session()).InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t")).expect("table");
    let tbl_info = tbl.Meta().Clone();
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, time::Second);
    let mut ctx = util::WithInternalSourceType(context::WithValue(context::Background(), "etcd", true), "taskManager");
    let mgr = storage::NewTaskManager(pool.clone());
    storage::SetTaskManager(mgr.clone());
    let sch = scheduler::NewManager(util::WithInternalSourceType(ctx.clone(), "scheduler"), store.clone(), mgr.clone(), "host:port", proto::NodeResourceForTest);
    let keyspace = store.GetKeyspace();
    let scope = handle::GetTargetScope();
    mgr.InitMeta(ctx.clone(), ":4000", scope).expect("init meta");

    let conn = tk.Session().GetSQLExecutor();
    let job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", tbl_info.ID, "root", "", &importer::ImportParameters::default(), 0).expect("create prepare job");
    let default_charset = "utf8mb4".to_owned();
    let logical_plan = importinto::LogicalPlan {
        JobID: job_id,
        Plan: importer::Plan {
            Path: format!("gs://test-load/*.csv?endpoint={}&access-key=aaaaaa&secret-access-key=bbbbbb", gcs_endpoint),
            Format: importer::DataFormatAuto,
            DBName: "test".to_owned(),
            TableInfo: Some({
                let mut c = tbl_info.Clone();
                c.Name = ast::NewCIStr("t");
                c.State = model::StatePublic;
                c
            }),
            DisableTiKVImportMode: true,
            CloudStorageURI: sort_storage_uri,
            InImportInto: true,
            Charset: Some(default_charset),
            FieldNullDef: vec![r"\N".to_owned()],
            LineFieldsInfo: plannercore::LineFieldsInfo {
                FieldsTerminatedBy: ",".to_owned(),
                FieldsEnclosedBy: "\"".to_owned(),
                FieldsEscapedBy: "\\".to_owned(),
                LinesStartingBy: "".to_owned(),
                LinesTerminatedBy: "".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        },
        Stmt: "IMPORT INTO test.t FROM 'gs://test-load/*.csv?endpoint=xxx'".to_owned(),
        ..Default::default()
    };
    assert!(importinto::ShouldUseAsyncPrepare(&logical_plan.Plan));
    let bs = logical_plan.ToTaskMeta().expect("prepare task meta");
    let mut task = proto::Task {
        TaskBase: proto::TaskBase { Type: proto::ImportInto, Step: proto::StepInit, State: proto::TaskStatePending, ExtraParams: proto::ExtraParams { PrepareMode: proto::PrepareModeRequired, ..Default::default() }, ..Default::default() },
        Meta: bs.clone(),
        StateUpdateTime: time::Now(),
        ..Default::default()
    };
    task.ID = mgr.CreateTask(ctx.clone(), importinto::TaskKey(job_id), proto::ImportInto, keyspace, 1, scope, 1, proto::ExtraParams { PrepareMode: proto::PrepareModeRequired, ..Default::default() }, bs).expect("create prepare task");
    let d = sch.MockScheduler(&task);
    let ext = importinto::NewImportSchedulerForTest(true, &task, scheduler::NewParamForTest(mgr.clone(), new_import_test_runtime(&ctrl, store.clone(), Some(pool.clone()))));

    ext.OnPrepare(ctx.clone(), &d, &task).expect("on prepare");
    let got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("prepared job");
    assert_eq!(got_job_info.Status, importer::JobStatusRunning);
    assert_eq!(got_job_info.Step, importer::JobStepPreparing);
    assert_eq!(got_job_info.Parameters.Format, importer::DataFormatCSV);
    assert_eq!(got_job_info.SourceFileSize, 2);
    assert!(!got_job_info.StartTime.IsZero());
    let start_time = got_job_info.StartTime;

    task.Step = proto::StepPrepared;
    let next_step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(next_step, proto::ImportStepEncodeAndSort);
    let subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], next_step).expect("first business subtasks");
    assert!(!subtask_metas.is_empty());
    let got_job_info = importer::GetJob(ctx, conn, job_id, "root", true).expect("global sorting job");
    assert_eq!(got_job_info.Status, importer::JobStatusRunning);
    assert_eq!(got_job_info.Step, importer::JobStepGlobalSorting);
    assert_eq!(got_job_info.StartTime, start_time);
    pool.Close();
    server.Stop();
}

// test_scheduler_on_done_cancel_resets_table_mode 对应 Go 的 TestSchedulerOnDoneCancelResetsTableMode。
#[test]
pub fn test_scheduler_on_done_cancel_resets_table_mode() {
    let ctrl = gomock::NewController();
    if !kerneltype::IsClassic() {
        // table mode is only set in classic kernel.
        return;
    }
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(id int)");

    let dom = domain::GetDomain(tk.Session());
    let is = dom.InfoSchema();
    let db_info = is.SchemaByName(ast::NewCIStr("test")).expect("schema");
    let tbl = is.TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t")).expect("table");
    let tbl_info = tbl.Meta().Clone();
    ddl::AlterTableMode(dom.DDLExecutor(), tk.Session(), model::TableModeImport, db_info.ID, tbl_info.ID).expect("set import mode");
    assert_eq!(dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t")).unwrap().Meta().Mode, model::TableModeImport);

    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, time::Second);
    let ctx = util::WithInternalSourceType(context::WithValue(context::Background(), "etcd", true), "taskManager");
    let mgr = storage::NewTaskManager(pool.clone());
    storage::SetTaskManager(mgr.clone());

    // Create a job to ensure onDone cancels it successfully.
    let conn = tk.Session().GetSQLExecutor();
    let job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", tbl_info.ID, "root", "", &importer::ImportParameters::default(), 123).expect("create cancel job");
    let logical_plan = importinto::LogicalPlan {
        JobID: job_id,
        Plan: importer::Plan {
            DBName: "test".to_owned(),
            DBID: db_info.ID,
            TableInfo: Some({
                let mut c = tbl_info.Clone();
                c.Name = ast::NewCIStr("t");
                c.State = model::StatePublic;
                c
            }),
            DisableTiKVImportMode: true,
            ..Default::default()
        },
        Stmt: "IMPORT INTO db.tb FROM 'gs://test-load/*.csv?endpoint=xxx'".to_owned(),
        ..Default::default()
    };
    let bs = logical_plan.ToTaskMeta().expect("cancel task meta");
    let task = proto::Task {
        TaskBase: proto::TaskBase { ID: 1, Type: proto::TaskTypeExample, Step: proto::StepInit, State: proto::TaskStateReverting, ..Default::default() },
        Meta: bs,
        Error: errors::New("cancelled by user"),
        ..Default::default()
    };
    let ext = importinto::NewImportSchedulerForTest(false, &task, scheduler::NewParamForTest(mgr.clone(), new_import_test_runtime(&ctrl, store.clone(), Some(pool.clone()))));
    ext.OnDone(ctx.clone(), None, &task).expect("cancel on done");
    assert_eq!(dom.InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t")).unwrap().Meta().Mode, model::TableModeNormal);
    pool.Close();
}

// test_scheduler_ext_global_sort 对应 Go 的 TestSchedulerExtGlobalSort，保留 global sort 全状态流转。
#[test]
pub fn test_scheduler_ext_global_sort() {
    let ctrl = gomock::NewController();
    let host = "127.0.0.1";
    let port = 4448_u16;
    let opt = fakestorage::Options { Scheme: "http".to_owned(), Host: host.to_owned(), Port: port, PublicHost: host.to_owned(), ..Default::default() };
    let gcs_endpoint = format!("http://{}:{}/storage/v1/", host, port);
    let sort_storage_uri = format!("gs://sort-bucket/import?endpoint={}&access-key=aaaaaa&secret-access-key=bbbbbb", gcs_endpoint);
    let server = fakestorage::NewServerWithOptions(opt).expect("fake gcs");
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts { Name: "sort-bucket".to_owned(), ..Default::default() });
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts { Name: "test-load".to_owned(), ..Default::default() });

    // Domain start scheduler manager automatically, we need to disable it as we test import task management in this case.
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");
    let store = testkit::CreateMockStore();
    let keyspace = store.GetKeyspace();
    let scope = handle::GetTargetScope();
    let tk = testkit::NewTestKit(&store);
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, time::Second);
    let ctx = util::WithInternalSourceType(context::WithValue(context::Background(), "etcd", true), "taskManager");
    let mgr = storage::NewTaskManager(pool.clone());
    storage::SetTaskManager(mgr.clone());
    let sch = scheduler::NewManager(util::WithInternalSourceType(ctx.clone(), "scheduler"), store.clone(), mgr.clone(), "host:port", proto::NodeResourceForTest);
    mgr.InitMeta(ctx.clone(), ":4000", scope).expect("init meta");

    let conn = tk.Session().GetSQLExecutor();
    let job_id = importer::CreateJob(ctx.clone(), conn.clone(), "test", "t", 1, "root", "", &importer::ImportParameters::default(), 123).expect("create global job");
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Status, "pending");
    let logical_plan = importinto::LogicalPlan {
        JobID: job_id,
        Plan: importer::Plan {
            Path: format!("gs://test-load/*.csv?endpoint={}&access-key=aaaaaa&secret-access-key=bbbbbb", gcs_endpoint),
            Format: "csv".to_owned(),
            DBName: "test".to_owned(),
            TableInfo: Some(model::TableInfo { Name: ast::NewCIStr("t"), State: model::StatePublic, ..Default::default() }),
            DisableTiKVImportMode: true,
            CloudStorageURI: sort_storage_uri,
            InImportInto: true,
            ..Default::default()
        },
        Stmt: "IMPORT INTO db.tb FROM 'gs://test-load/*.csv?endpoint=xxx'".to_owned(),
        EligibleInstances: vec![serverinfo::ServerInfo { StaticInfo: serverinfo::StaticInfo { ID: "1".to_owned() } }],
        ChunkMap: hashmap! {
            1_i32 => vec![importer::Chunk { Path: "gs://test-load/1.csv".to_owned() }],
            2_i32 => vec![importer::Chunk { Path: "gs://test-load/2.csv".to_owned() }],
        },
        ..Default::default()
    };
    let bs = logical_plan.ToTaskMeta().expect("global task meta");
    let mut task = proto::Task {
        TaskBase: proto::TaskBase { Type: proto::ImportInto, Step: proto::StepInit, State: proto::TaskStatePending, RequiredSlots: 16, ..Default::default() },
        Meta: bs,
        StateUpdateTime: time::Now(),
        ..Default::default()
    };
    let manager = storage::GetTaskManager().expect("task manager");
    let task_meta = json::Marshal(&task).expect("marshal task");
    let task_id = manager.CreateTask(ctx.clone(), importinto::TaskKey(job_id), proto::ImportInto, keyspace, 1, scope, 1, proto::ExtraParams::default(), task_meta).expect("create global task");
    task.ID = task_id;

    let d = sch.MockScheduler(&task);
    let ext = importinto::NewImportSchedulerForTest(true, &task, scheduler::NewParamForTest(manager.clone(), new_import_test_runtime(&ctrl, store.clone(), Some(pool.clone()))));
    let mut subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("encode-sort subtasks");
    assert_eq!(subtask_metas.len(), 2);
    let mut next_step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(next_step, proto::ImportStepEncodeAndSort);
    let mut got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("global sorting job");
    assert_eq!(got_job_info.Status, "running");
    assert_eq!(got_job_info.Step, "global-sorting");

    // update task/subtask, and finish subtask, so we can go to merge-sort stage.
    let mut subtasks = subtask_metas.iter().enumerate().map(|(i, m)| proto::NewSubtask(next_step, task.ID, task.Type, "", 1, m.clone(), i + 1)).collect();
    manager.SwitchTaskStep(ctx.clone(), &mut task, proto::TaskStatePending, next_step, subtasks).expect("switch encode");
    task.Step = next_step;
    let sort_step_meta = importinto::ImportStepMeta {
        SortedDataMeta: Some(globalsort::SortedKVMeta {
            StartKey: b"ta".to_vec(),
            EndKey: b"tc".to_vec(),
            TotalKVSize: 12,
            MultipleFilesStats: vec![simplesst::MultipleFilesStat { Filenames: vec![("gs://sort-bucket/data/1".to_owned(), "gs://sort-bucket/data/1.stat".to_owned())] }],
            ..Default::default()
        }),
        SortedIndexMetas: hashmap! {
            1_i64 => globalsort::SortedKVMeta {
                StartKey: b"ia".to_vec(),
                EndKey: b"ic".to_vec(),
                TotalKVSize: 12,
                MultipleFilesStats: vec![simplesst::MultipleFilesStat { Filenames: vec![("gs://sort-bucket/index/1".to_owned(), "gs://sort-bucket/index/1.stat".to_owned())] }],
                ..Default::default()
            }
        },
        ..Default::default()
    };
    let sort_step_meta_bytes = json::Marshal(&sort_step_meta).expect("marshal sort meta");
    for subtask in manager.GetSubtasksWithHistory(ctx.clone(), task_id, task.Step).expect("encode history") {
        manager.FinishSubtask(ctx.clone(), subtask.ExecID, subtask.ID, sort_step_meta_bytes.clone()).expect("finish encode");
    }

    // to merge-sort stage；forceMergeSort 只强制 data group 走 merge。
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/dxf/importinto/forceMergeSort", "return(\"data\")");
    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("merge subtasks");
    assert_eq!(subtask_metas.len(), 1);
    next_step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(next_step, proto::ImportStepMergeSort);
    got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("merge job");
    assert_eq!(got_job_info.Status, "running");
    assert_eq!(got_job_info.Step, "global-sorting");

    subtasks = subtask_metas.iter().enumerate().map(|(i, m)| proto::NewSubtask(next_step, task.ID, task.Type, "", 1, m.clone(), i + 1)).collect();
    manager.SwitchTaskStep(ctx.clone(), &mut task, proto::TaskStatePending, next_step, subtasks).expect("switch merge");
    task.Step = next_step;
    let merge_sort_step_meta = importinto::MergeSortStepMeta {
        KVGroup: "data".to_owned(),
        SortedKVMeta: globalsort::SortedKVMeta { StartKey: b"ta".to_vec(), EndKey: b"tc".to_vec(), TotalKVSize: 12, ..Default::default() },
        DataFiles: vec!["gs://sort-bucket/data/1".to_owned()],
        ..Default::default()
    };
    let merge_sort_step_meta_bytes = json::Marshal(&merge_sort_step_meta).expect("marshal merge meta");
    for subtask in manager.GetSubtasksWithHistory(ctx.clone(), task_id, task.Step).expect("merge history") {
        manager.FinishSubtask(ctx.clone(), subtask.ExecID, subtask.ID, merge_sort_step_meta_bytes.clone()).expect("finish merge");
    }

    // to write-and-ingest stage；mockWriteIngestSpecs 避免真实 ingest 规划。
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/dxf/importinto/mockWriteIngestSpecs", "return(true)");
    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("write ingest subtasks");
    assert_eq!(subtask_metas.len(), 2);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::ImportStepWriteAndIngest);
    got_job_info = importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).expect("importing job");
    assert_eq!(got_job_info.Status, "running");
    assert_eq!(got_job_info.Step, "importing");

    // collect-conflicts and conflict-resolution are zero-subtask state transitions in this fixture.
    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("collect conflicts");
    assert_eq!(subtask_metas.len(), 0);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::ImportStepCollectConflicts);
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Step, "resolving-conflicts");

    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("conflict resolution");
    assert_eq!(subtask_metas.len(), 0);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::ImportStepConflictResolution);
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Step, "resolving-conflicts");

    subtask_metas = ext.OnNextSubtasksBatch(ctx.clone(), &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("post process");
    assert_eq!(subtask_metas.len(), 1);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::ImportStepPostProcess);
    assert_eq!(importer::GetJob(ctx.clone(), conn.clone(), job_id, "root", true).unwrap().Step, "validating");

    subtask_metas = ext.OnNextSubtasksBatch(ctx, &d, &task, vec![":4000"], ext.GetNextStep(&task.TaskBase)).expect("done");
    assert_eq!(subtask_metas.len(), 0);
    task.Step = ext.GetNextStep(&task.TaskBase);
    assert_eq!(task.Step, proto::StepDone);
    pool.Close();
    server.Stop();
}
"###;

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
    struct Backend(Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>);
    impl storage::SQLBackend for Backend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push((sql.to_owned(), args));
            Ok(storage::SQLResult {
                rows: vec![],
                affected_rows: 1,
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
    let calls = Arc::new(Mutex::new(Vec::new()));
    let manager = storage::NewTaskManager(storage::util::SessionPool::with_factory({
        let calls = calls.clone();
        move || {
            Ok(storage::sessionctx::Context::with_backend(Arc::new(
                Backend(calls.clone()),
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
