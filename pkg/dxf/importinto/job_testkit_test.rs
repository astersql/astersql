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

// IMPORT INTO job 相关 testkit 集成测试草稿与可执行边界用例。
//
// 大块字符串保留 Go 侧 SubmitTask/进度展示/分组查询流程；
// 可执行部分覆盖 `FormatSecondAsTime` 的天级与日内格式边界。

const _GO_JOB_TESTKIT_DRAFT: &str = r###"
// IMPORT INTO testkit 集成测试的关键流程，不启动真实 etcd 集群、不创建 TiDB store，也不执行业务 SQL。

// switch_task_step 对应 Go 的 switchTaskStep 测试辅助函数。
// 它先读取 task，再以 Running 状态切换到指定 step；错误检查仍按 require.NoError 语义保留。
fn switch_task_step(ctx: context::Context, manager: &storage::TaskManager, task_id: i64, step: proto::Step) {
    let task = manager.GetTaskByID(ctx, task_id).expect("Go require.NoError(t, err)");
    assert_ok!(manager.SwitchTaskStep(ctx, &task, proto::TaskStateRunning, step, None));
}

// test_submit_task_nextgen 对应 Go 的 TestSubmitTaskNextgen。
// 该测试依赖 nextgen keyspace、etcd failpoint 和 mock store；只保留准备顺序和断言矩阵。
#[test]
fn test_submit_task_nextgen() {
    if kerneltype::IsClassic() {
        skip!("This test is only for nextgen");
    }
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");

    integration::BeforeTestExternal();
    let cluster = integration::NewClusterV3(integration::ClusterConfig { Size: 10, ..Default::default() });
    defer(|| cluster.Terminate());
    let keyspace_ids = HashMap::from([
        (keyspace::System, 1_u32),
        ("ks", 2_u32),
    ]);
    testfailpoint::EnableCall(
        "github.com/pingcap/tidb/pkg/domain/crossks/injectETCDCli",
        |cli_p: &mut *mut clientv3::Client, ks: &str| {
            let id = *keyspace_ids.get(ks).expect("Go require.True(t, ok)");
            // 每个 keyspace 使用一个 etcd client；Go 测试会从 cluster 接管 client 生命周期。
            *cli_p = cluster.Client((id - 1) as i32);
            cluster.TakeClient((id - 1) as i32);
            let codec = tikv::NewCodecV2(tikv::ModeTxn, keyspacepb::KeyspaceMeta { Id: id, Name: ks.to_string() })
                .expect("Go require.NoError(t, err)");
            etcd::SetEtcdCliByNamespace(*cli_p, keyspace::MakeKeyspaceEtcdNamespace(codec));
        },
    );

    assert_ok!(kvstore::Register(config::StoreTypeUniStore, mockstore::EmbedUnistoreDriver::default()));
    let (sys_ks_store, _domain) = testkit::CreateMockStoreAndDomainForKS(keyspace::System);
    let sys_ks_tk = testkit::NewTestKit(sys_ks_store.clone());
    // Go 注释：uni-store 的 Store 实例彼此隔离，因此用 storeMap 和 failpoint mock GetStore。
    let mut store_map: HashMap<&str, kv::Storage> = HashMap::with_capacity(4);
    store_map.insert(keyspace::System, sys_ks_store.clone());
    testfailpoint::EnableCall(
        "github.com/pingcap/tidb/pkg/domain/crossks/beforeGetStore",
        |fn_p: &mut fn(&str) -> Result<kv::Storage, Error>| {
            *fn_p = |ks: &str| Ok(store_map[ks].clone());
        },
    );
    let (user_ks_store, _domain) = testkit::CreateMockStoreAndDomainForKS("ks");
    store_map.insert("ks", user_ks_store.clone());
    let user_ks_tk = testkit::NewTestKit(user_ks_store.clone());

    let ctx = util::WithInternalSourceType(context::Background(), kv::InternalDistTask);
    let manually_init_fn = |curr_ks_store: kv::Storage, sys_ks_store: kv::Storage| {
        // Go 在禁用 domain dist task 后手动初始化 task manager 和 framework meta。
        let get_pool_fn = |store: kv::Storage| -> tidbutil::SessionPool {
            let pool = pools::NewResourcePool(
                || Ok(testkit::NewTestKit(store.clone()).Session()),
                1,
                1,
                Duration::from_secs(1),
            );
            cleanup(|| pool.Close());
            pool
        };
        let task_mgr = storage::NewTaskManager(get_pool_fn(curr_ks_store.clone()));
        storage::SetTaskManager(task_mgr.clone());
        let mut sys_ks_task_mgr = task_mgr;
        if kv::IsUserKS(curr_ks_store) {
            sys_ks_task_mgr = storage::NewTaskManager(get_pool_fn(sys_ks_store));
            storage::SetDXFSvcTaskMgr(sys_ks_task_mgr.clone());
        }
        assert_ok!(sys_ks_task_mgr.InitMeta(ctx, "tidb", "dxf_service"));
    };

    subtest("submit task in system keyspace", || {
        config::UpdateGlobal(|conf| {
            conf.KeyspaceName = keyspace::System.to_string();
        });
        manually_init_fn(sys_ks_store.clone(), sys_ks_store.clone());
        let (job_id, task) = importinto::SubmitTask(
            ctx,
            importer::Plan {
                TableInfo: Some(model::TableInfo::default()),
                Parameters: Some(importer::ImportParameters::default()),
                ..Default::default()
            },
            "import into t from '/path/to/file'",
        ).expect("Go require.NoError(t, err)");
        // system keyspace 内同时创建 job 和 global task，user keyspace 不应可见。
        sys_ks_tk.MustQuery("select count(1) from mysql.tidb_import_jobs where id = ?", job_id).Check(rows!("1"));
        sys_ks_tk.MustQuery("select count(1) from mysql.tidb_global_task where id = ?", task.ID).Check(rows!("1"));
        user_ks_tk.MustQuery("select count(1) from mysql.tidb_import_jobs where id = ?", job_id).Check(rows!("0"));
        user_ks_tk.MustQuery("select count(1) from mysql.tidb_global_task where id = ?", task.ID).Check(rows!("0"));
    });

    subtest("submit task in user keyspace", || {
        sys_ks_tk.MustExec("delete from mysql.tidb_import_jobs");
        user_ks_tk.MustExec("delete from mysql.tidb_global_task");
        config::UpdateGlobal(|conf| {
            conf.KeyspaceName = "ks".to_string();
        });
        manually_init_fn(user_ks_store.clone(), sys_ks_store.clone());
        let (job_id, task) = importinto::SubmitTask(
            ctx,
            importer::Plan {
                TableInfo: Some(model::TableInfo::default()),
                Parameters: Some(importer::ImportParameters::default()),
                ..Default::default()
            },
            "import into t from '/path/to/file'",
        ).expect("Go require.NoError(t, err)");
        // job 留在用户 keyspace，global task 进入系统 keyspace；反向查询均应为 0。
        user_ks_tk.MustQuery("select count(1) from mysql.tidb_import_jobs where id = ?", job_id).Check(rows!("1"));
        sys_ks_tk.MustQuery("select count(1) from mysql.tidb_global_task where id = ?", task.ID).Check(rows!("1"));
        sys_ks_tk.MustQuery("select count(1) from mysql.tidb_import_jobs where id = ?", job_id).Check(rows!("0"));
        user_ks_tk.MustQuery("select count(1) from mysql.tidb_global_task where id = ?", task.ID).Check(rows!("0"));
    });

    subtest("submit global-sort task uses async prepare mode", || {
        sys_ks_tk.MustExec("delete from mysql.tidb_import_jobs");
        sys_ks_tk.MustExec("delete from mysql.tidb_global_task");
        config::UpdateGlobal(|conf| {
            conf.KeyspaceName = keyspace::System.to_string();
        });
        manually_init_fn(sys_ks_store.clone(), sys_ks_store.clone());
        let (job_id, task) = importinto::SubmitTask(
            ctx,
            importer::Plan {
                TableInfo: Some(model::TableInfo::default()),
                Parameters: Some(importer::ImportParameters::default()),
                ThreadCnt: 16,
                MaxNodeCnt: 8,
                CloudStorageURI: "local:///tmp/prepare-mode-sort".to_string(),
                ..Default::default()
            },
            "import into t from 's3://bucket/test.csv'",
        ).expect("Go require.NoError(t, err)");
        sys_ks_tk.MustQuery("select count(1) from mysql.tidb_import_jobs where id = ?", job_id).Check(rows!("1"));
        sys_ks_tk.MustQuery(
            "select concurrency, max_node_count, json_extract(extra_params, '$.prepare_mode') from mysql.tidb_global_task where id = ?",
            task.ID,
        ).Check(rows!("1 1 1"));
    });
}

// test_get_task_imported_rows 对应 Go 的 TestGetTaskImportedRows。
// 它分别覆盖 local sort 与 global sort 下 RuntimeInfo 的 ImportedRows 和 Percent 计算。
#[test]
fn test_get_task_imported_rows() {
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");

    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, Duration::from_secs(1));
    defer(|| pool.Close());
    let ctx = util::WithInternalSourceType(context::Background(), kv::InternalDistTask);

    let manager = storage::NewTaskManager(pool);
    storage::SetTaskManager(manager.clone());
    assert_ok!(manager.InitMeta(ctx, ":4000", ""));

    let mut task_meta = importinto::TaskMeta {
        Plan: importer::Plan::default(),
        Summary: importer::Summary {
            EncodeSummary: importer::StepSummary { Bytes: 10000, RowCnt: 1000 },
            IngestSummary: importer::StepSummary { Bytes: 10000, RowCnt: 1000 },
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bytes = json::Marshal(&task_meta).expect("Go require.NoError(t, err)");
    let mut task_id = manager.CreateTask(ctx, importinto::TaskKey(111), proto::ImportInto, "", 1, "", 0, proto::ExtraParams::default(), bytes.clone())
        .expect("Go require.NoError(t, err)");
    let import_step_summaries = vec![
        execute::SubtaskSummary { RowCnt: atomic::NewInt64(300), Processed: atomic::NewInt64(4000), ..Default::default() },
        execute::SubtaskSummary { RowCnt: atomic::NewInt64(400), Processed: atomic::NewInt64(4000), ..Default::default() },
    ];
    for summary in &import_step_summaries {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepImport, "", None, summary, proto::SubtaskStatePending, proto::ImportInto, 11);
    }

    switch_task_step(ctx, &manager, task_id, proto::ImportStepImport);
    let loc = tk.Session().GetSessionVars().Location();
    let mut run_info = importinto::GetRuntimeInfoForJob(ctx, loc, 111).expect("Go require.NoError(t, err)");
    assert_eq!(700, run_info.ImportRows);
    assert_eq!("80", run_info.Percent());

    // global sort：导入行数来自 WriteAndIngest step 的 subtask summary。
    task_meta = importinto::TaskMeta {
        Plan: importer::Plan {
            CloudStorageURI: "s3://test-bucket/test-path".to_string(),
            ..Default::default()
        },
        Summary: importer::Summary {
            IngestSummary: importer::StepSummary { Bytes: 10000, RowCnt: 1000 },
            ..Default::default()
        },
        ..Default::default()
    };
    bytes = json::Marshal(&task_meta).expect("Go require.NoError(t, err)");
    task_id = manager.CreateTask(ctx, importinto::TaskKey(222), proto::ImportInto, "", 1, "", 0, proto::ExtraParams::default(), bytes.clone())
        .expect("Go require.NoError(t, err)");
    let ingest_step_summaries = vec![
        execute::SubtaskSummary { RowCnt: atomic::NewInt64(100), Processed: atomic::NewInt64(1000), ..Default::default() },
        execute::SubtaskSummary { RowCnt: atomic::NewInt64(200), Processed: atomic::NewInt64(2000), ..Default::default() },
    ];
    for summary in &ingest_step_summaries {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepWriteAndIngest, "", Some(bytes.clone()), summary, proto::SubtaskStatePending, proto::ImportInto, 11);
    }

    switch_task_step(ctx, &manager, task_id, proto::ImportStepWriteAndIngest);
    run_info = importinto::GetRuntimeInfoForJob(ctx, tk.Session().GetSessionVars().Location(), 222)
        .expect("Go require.NoError(t, err)");
    assert_eq!(300, run_info.ImportRows);
    assert_eq!("30", run_info.Percent());
}

// test_show_import_progress 对应 Go 的 TestShowImportProgress。
// 该测试围绕 SHOW IMPORT JOB 展示字段，逐 step 验证 processed/total/percent/speed/ETA/imported rows。
#[test]
fn test_show_import_progress() {
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");
    let fmap = plannercore::ImportIntoFieldMap;

    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, Duration::from_secs(1));
    defer(|| pool.Close());
    let ctx = util::WithInternalSourceType(context::Background(), kv::InternalDistTask);

    let manager = storage::NewTaskManager(pool);
    storage::SetTaskManager(manager.clone());
    assert_ok!(manager.InitMeta(ctx, ":4000", ""));

    let task_meta = importinto::TaskMeta {
        Plan: importer::Plan {
            CloudStorageURI: "s3://test-bucket/test-path".to_string(),
            ..Default::default()
        },
        Summary: importer::Summary {
            EncodeSummary: importer::StepSummary { Bytes: 1000, RowCnt: 100 },
            MergeSummary: importer::StepSummary { Bytes: 0, RowCnt: 0 },
            IngestSummary: importer::StepSummary { Bytes: 1000, RowCnt: 100 },
            CollectConflictsSummary: importer::StepSummary { RowCnt: 1000, ..Default::default() },
            ResolveConflictsSummary: importer::StepSummary { RowCnt: 500, ..Default::default() },
            ImportedRows: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let bytes = json::Marshal(&task_meta).expect("Go require.NoError(t, err)");
    let conn = tk.Session().GetSQLExecutor();
    let job_id = importer::CreateJob(ctx, conn, "test", "t", 1, "root", "", importer::ImportParameters::default(), 1000)
        .expect("Go require.NoError(t, err)");
    let task_id = manager.CreateTask(ctx, importinto::TaskKey(job_id), proto::ImportInto, "", 1, "", 0, proto::ExtraParams::default(), bytes.clone())
        .expect("Go require.NoError(t, err)");

    let subtasks = vec![
        (
            execute::SubtaskSummary {
                RowCnt: atomic::NewInt64(20),
                Processed: atomic::NewInt64(200),
                Progresses: vec![
                    execute::Progress { RowCnt: 0, Processed: 0, UpdateTime: time::Unix(1001, 0) },
                    execute::Progress { RowCnt: 20, Processed: 200, UpdateTime: time::Unix(1002, 0) },
                ],
                ..Default::default()
            },
            proto::SubtaskStateRunning,
        ),
        (
            execute::SubtaskSummary {
                RowCnt: atomic::NewInt64(30),
                Processed: atomic::NewInt64(300),
                Progresses: vec![
                    execute::Progress { RowCnt: 0, Processed: 0, UpdateTime: time::Unix(1000, 0) },
                    execute::Progress { RowCnt: 15, Processed: 150, UpdateTime: time::Unix(1001, 0) },
                    execute::Progress { RowCnt: 30, Processed: 300, UpdateTime: time::Unix(1002, 0) },
                ],
                ..Default::default()
            },
            proto::SubtaskStateSucceed,
        ),
        (
            execute::SubtaskSummary {
                RowCnt: atomic::NewInt64(0),
                Processed: atomic::NewInt64(0),
                ..Default::default()
            },
            proto::SubtaskStateSucceed,
        ),
    ];

    let check_show_info = |step: &str, processed: &str, total: &str, percent: &str, speed: &str, eta: &str, imported: i64| {
        let rows = tk.MustQuery(format!("show import job {}", job_id)).Rows();
        assert_eq!(step, rows[0][fmap["CurStep"]]);
        assert_eq!(processed, rows[0][fmap["CurStepProcessedSize"]]);
        assert_eq!(total, rows[0][fmap["CurStepTotalSize"]]);
        assert_eq!(percent, rows[0][fmap["CurStepProgressPct"]]);
        assert_eq!(speed, rows[0][fmap["CurStepSpeed"]]);
        assert_eq!(eta, rows[0][fmap["CurStepETA"]]);

        let imported_rows = strconv::Atoi(rows[0][fmap["ImportedRows"]].as_string()).expect("Go require.NoError(t, err)");
        assert_eq!(imported, imported_rows);
    };

    // Init step。
    assert_ok!(importer::StartJob(ctx, conn, job_id, importer::JobStepGlobalSorting));
    check_show_info("init", "0B", "0B", "N/A", "0B/s", "N/A", 0);

    testfailpoint::Enable("github.com/pingcap/tidb/pkg/dxf/importinto/mockSpeedDuration", "return(5000)");

    // Encode step：两个有进度的 subtask 共同贡献 500B/1000B 和 50%。
    switch_task_step(ctx, &manager, task_id, proto::ImportStepEncodeAndSort);
    for (summary, state) in &subtasks {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepEncodeAndSort, "", Some(bytes.clone()), summary, *state, proto::ImportInto, 11);
    }
    let loc = tk.Session().GetSessionVars().Location();
    let mut run_info = importinto::GetRuntimeInfoForJob(ctx, loc, job_id).expect("Go require.NoError(t, err)");
    assert_eq!(1000, run_info.Total);
    assert_eq!(500, run_info.Processed);
    check_show_info("encode", "500B", "1000B", "50", "100B/s", "00:00:05", 0);

    // Merge step：summary 总量为 0，因此显示 0 和 N/A。
    switch_task_step(ctx, &manager, task_id, proto::ImportStepMergeSort);
    run_info = importinto::GetRuntimeInfoForJob(ctx, loc, job_id).expect("Go require.NoError(t, err)");
    assert_eq!(0, run_info.Total);
    assert_eq!(0, run_info.Processed);
    check_show_info("merge-sort", "0B", "0B", "0", "0B/s", "N/A", 0);

    // Ingest step。
    for (summary, state) in &subtasks {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepWriteAndIngest, "", Some(bytes.clone()), summary, *state, proto::ImportInto, 11);
    }
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/dxf/importinto/mockSpeedDuration", "return(10000)");
    switch_task_step(ctx, &manager, task_id, proto::ImportStepWriteAndIngest);
    check_show_info("ingest", "500B", "1000B", "50", "50B/s", "00:00:10", 50);

    // collect-conflicts step 使用 conflicts 单位而非字节单位。
    switch_task_step(ctx, &manager, task_id, proto::ImportStepCollectConflicts);
    for (summary, state) in &subtasks {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepCollectConflicts, "", Some(bytes.clone()), summary, *state, proto::ImportInto, 11);
    }
    check_show_info("collect-conflicts", "500 conflicts", "1000 conflicts", "50", "50 conflicts/s", "00:00:10", 0);

    // conflict-resolution step 的总量来自 ResolveConflictsSummary，所以 500/500 为 100%。
    switch_task_step(ctx, &manager, task_id, proto::ImportStepConflictResolution);
    for (summary, state) in &subtasks {
        testutil::CreateSubTaskWithSummary(&manager, task_id, proto::ImportStepConflictResolution, "", Some(bytes.clone()), summary, *state, proto::ImportInto, 11);
    }
    check_show_info("conflict-resolution", "500 conflicts", "500 conflicts", "100", "50 conflicts/s", "00:00:00", 0);

    // Post-process step 不再展示进度，但 imported rows 使用 task summary 的最终值。
    switch_task_step(ctx, &manager, task_id, proto::ImportStepPostProcess);
    check_show_info("post-process", "0B", "0B", "N/A", "0B/s", "N/A", 100);
}

// test_show_import_group 对应 Go 的 TestShowImportGroup。
// 它验证空分组不可见、按 group 聚合 job 数，以及指定 group 查询的 create time 字段。
#[test]
fn test_show_import_group() {
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/domain/MockDisableDistTask", "return(true)");

    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    let pool = pools::NewResourcePool(|| Ok(tk.Session()), 1, 1, Duration::from_secs(1));
    defer(|| pool.Close());
    let ctx = util::WithInternalSourceType(context::Background(), kv::InternalDistTask);

    let manager = storage::NewTaskManager(pool);
    storage::SetTaskManager(manager.clone());
    assert_ok!(manager.InitMeta(ctx, ":4000", ""));
    let conn = tk.Session().GetSQLExecutor();

    let mut rows = tk.MustQuery(r#"show import group "group2""#).Rows();
    assert_eq!(0, rows.len());
    rows = tk.MustQuery("show import groups").Rows();
    assert_eq!(0, rows.len());

    let import_jobs = vec![
        ImportJobCase { SchemaName: "test", TableName: "t1", TableID: 1, GroupKey: "group1" },
        ImportJobCase { SchemaName: "test", TableName: "t2", TableID: 2, GroupKey: "group1" },
        ImportJobCase { SchemaName: "test", TableName: "t3", TableID: 3, GroupKey: "group2" },
        ImportJobCase { SchemaName: "test", TableName: "t4", TableID: 4, GroupKey: "" },
    ];

    for job in &import_jobs {
        let job_id = importer::CreateJob(ctx, conn, job.SchemaName, job.TableName, job.TableID, "root", job.GroupKey, importer::ImportParameters::default(), 1000)
            .expect("Go require.NoError(t, err)");
        let task_id = manager.CreateTask(ctx, importinto::TaskKey(job_id), proto::ImportInto, "", 1, "", 0, proto::ExtraParams::default(), None)
            .expect("Go require.NoError(t, err)");
        switch_task_step(ctx, &manager, task_id, proto::ImportStepEncodeAndSort);
        testutil::CreateSubTask(&manager, task_id, proto::ImportStepEncodeAndSort, "", None, proto::ImportInto, 11);
    }

    rows = tk.MustQuery("show import groups").Sort().Rows();
    for row in &rows {
        // create time should never be null。
        assert_ne!("<nil>", row[7]);
    }
    assert_eq!(2, rows.len());
    assert_eq!("group1", rows[0][0]);
    assert_eq!("2", rows[0][1]);
    assert_eq!("group2", rows[1][0]);
    assert_eq!("1", rows[1][1]);

    rows = tk.MustQuery(r#"show import group "nonexist""#).Rows();
    assert_eq!(0, rows.len());
    rows = tk.MustQuery(r#"show import group "group2""#).Rows();
    assert_eq!(1, rows.len());
    assert_eq!("group2", rows[0][0]);
    assert_eq!("1", rows[0][1]);
    assert_ne!("<nil>", rows[0][7]);
}

// ImportJobCase 对应 Go TestShowImportGroup 中的匿名 importJobs 元素。
struct ImportJobCase {
    SchemaName: &'static str,
    TableName: &'static str,
    TableID: i64,
    GroupKey: &'static str,
}

// test_format_time 对应 Go 的 TestFormatTime。
// 保留天级、跨天带秒、一天内和 8 小时边界的格式化期望。
#[test]
fn test_format_time() {
    assert_eq!("1 d 00:00:00", importinto::FormatSecondAsTime(86400));
    assert_eq!("2 d 00:00:01", importinto::FormatSecondAsTime(172801));
    assert_eq!("23:59:59", importinto::FormatSecondAsTime(86399));
    assert_eq!("00:59:59", importinto::FormatSecondAsTime(3599));
    assert_eq!("08:00:00", importinto::FormatSecondAsTime(28800));
}
"###;

use crate::FormatSecondAsTime;
use crate::job::{
    GetRuntimeInfoForJob, RuntimeInfoProvider, SubtaskRuntimeSummary, TaskKey, TaskRuntimeSnapshot,
};
use crate::proto::TaskMeta;
use astersql_dxf_framework_proto as dxfproto;
use astersql_executor_importer as importer;
use std::time::SystemTime;

struct JobTestkitRuntime {
    job_id: i64,
    snapshot: TaskRuntimeSnapshot,
}

impl RuntimeInfoProvider for JobTestkitRuntime {
    fn GetTaskRuntime(
        &self,
        task_key: &str,
    ) -> Result<TaskRuntimeSnapshot, astersql_errors::SharedError> {
        assert_eq!(task_key, TaskKey(self.job_id));
        Ok(self.snapshot.clone())
    }

    fn GetJobLastUpdateTime(
        &self,
        task_key: &str,
    ) -> Result<Option<SystemTime>, astersql_errors::SharedError> {
        assert_eq!(task_key, TaskKey(self.job_id));
        Ok(None)
    }
}

fn runtime_case(
    job_id: i64,
    step: dxfproto::Step,
    meta: TaskMeta,
    rows: &[(i64, i64, i64)],
) -> JobTestkitRuntime {
    JobTestkitRuntime {
        job_id,
        snapshot: TaskRuntimeSnapshot {
            State: dxfproto::TaskStateRunning,
            Step: step,
            Meta: meta.Marshal().unwrap(),
            ErrorMessage: None,
            Subtasks: rows
                .iter()
                .map(|&(row_count, processed, speed)| SubtaskRuntimeSummary {
                    RowCount: row_count,
                    Processed: processed,
                    Speed: speed,
                    UpdateTime: SystemTime::UNIX_EPOCH,
                })
                .collect(),
        },
    }
}

#[test]
fn imported_rows_match_go_local_and_global_sort_steps() {
    let local = runtime_case(
        111,
        dxfproto::ImportStepImport,
        TaskMeta {
            Summary: importer::Summary {
                EncodeSummary: importer::StepSummary {
                    Bytes: 10_000,
                    RowCnt: 1_000,
                },
                IngestSummary: importer::StepSummary {
                    Bytes: 10_000,
                    RowCnt: 1_000,
                },
                ..Default::default()
            },
            ..Default::default()
        },
        &[(300, 4_000, 0), (400, 4_000, 0)],
    );
    let info = GetRuntimeInfoForJob(&local, chrono_tz::UTC, 111).unwrap();
    assert_eq!(info.ImportRows, 700);
    assert_eq!(info.Percent(), "80");

    let global = runtime_case(
        222,
        dxfproto::ImportStepWriteAndIngest,
        TaskMeta {
            Summary: importer::Summary {
                IngestSummary: importer::StepSummary {
                    Bytes: 10_000,
                    RowCnt: 1_000,
                },
                ..Default::default()
            },
            ..Default::default()
        },
        &[(100, 1_000, 0), (200, 2_000, 0)],
    );
    let info = GetRuntimeInfoForJob(&global, chrono_tz::UTC, 222).unwrap();
    assert_eq!(info.ImportRows, 300);
    assert_eq!(info.Percent(), "30");
}

#[test]
fn show_import_progress_fields_match_go_step_matrix() {
    let meta = TaskMeta {
        Summary: importer::Summary {
            EncodeSummary: importer::StepSummary {
                Bytes: 1_000,
                RowCnt: 100,
            },
            IngestSummary: importer::StepSummary {
                Bytes: 1_000,
                RowCnt: 100,
            },
            CollectConflictsSummary: importer::StepSummary {
                RowCnt: 1_000,
                ..Default::default()
            },
            ResolveConflictsSummary: importer::StepSummary {
                RowCnt: 500,
                ..Default::default()
            },
            ImportedRows: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let cases = [
        (dxfproto::StepInit, 0, "0B", "0B", "N/A", "0B/s", "N/A", 0),
        (
            dxfproto::ImportStepEncodeAndSort,
            100,
            "500B",
            "1000B",
            "50",
            "100B/s",
            "00:00:05",
            0,
        ),
        (
            dxfproto::ImportStepMergeSort,
            0,
            "0B",
            "0B",
            "0",
            "0B/s",
            "N/A",
            0,
        ),
        (
            dxfproto::ImportStepWriteAndIngest,
            50,
            "500B",
            "1000B",
            "50",
            "50B/s",
            "00:00:10",
            50,
        ),
        (
            dxfproto::ImportStepCollectConflicts,
            50,
            "500 conflicts",
            "1000 conflicts",
            "50",
            "50 conflicts/s",
            "00:00:10",
            0,
        ),
        (
            dxfproto::ImportStepConflictResolution,
            50,
            "500 conflicts",
            "500 conflicts",
            "100",
            "50 conflicts/s",
            "00:00:00",
            0,
        ),
        (
            dxfproto::ImportStepPostProcess,
            0,
            "0B",
            "0B",
            "N/A",
            "0B/s",
            "N/A",
            100,
        ),
    ];
    for (step, speed, processed, total, percent, speed_str, eta, imported_rows) in cases {
        let rows = if step == dxfproto::StepInit
            || step == dxfproto::ImportStepMergeSort
            || step == dxfproto::ImportStepPostProcess
        {
            vec![]
        } else {
            vec![(20, 200, speed / 2), (30, 300, speed - speed / 2)]
        };
        let provider = runtime_case(333, step, meta.clone(), &rows);
        let info = GetRuntimeInfoForJob(&provider, chrono_tz::UTC, 333).unwrap();
        assert_eq!(info.ProcessedSize(), processed, "step {step:?}");
        assert_eq!(info.TotalSize(), total, "step {step:?}");
        assert_eq!(info.Percent(), percent, "step {step:?}");
        assert_eq!(info.SpeedStr(), speed_str, "step {step:?}");
        assert_eq!(info.ETA(), eta, "step {step:?}");
        assert_eq!(info.ImportRows, imported_rows, "step {step:?}");
    }
}

#[cfg(feature = "nextgen")]
mod nextgen_submission_tests {
    use super::*;
    use crate::job::{StorageSessionTableModeChanger, StorageTaskSubmissionService, SubmitTask};
    use astersql_dxf_framework_storage as storage;
    use astersql_meta_model as model;
    use astersql_parser_ast as ast;
    use std::sync::{Arc, Mutex};

    struct KeyspaceBackend(Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>);

    impl storage::SQLBackend for KeyspaceBackend {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            self.0.lock().unwrap().push((sql.to_owned(), args));
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

    fn manager(calls: Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>) -> storage::TaskManager {
        let session = storage::sessionctx::Context::with_backend(Arc::new(KeyspaceBackend(calls)));
        storage::NewTaskManager(storage::util::SessionPool::new(session))
    }

    fn plan(global_sort: bool) -> importer::Plan {
        importer::Plan {
            TableInfo: Some(Arc::new(model::TableInfo {
                ID: 7,
                Name: ast::NewCIStr("t"),
                ..Default::default()
            })),
            Parameters: Some(importer::ImportParameters::default()),
            ThreadCnt: if global_sort { 16 } else { 1 },
            MaxNodeCnt: if global_sort { 8 } else { 1 },
            CloudStorageURI: if global_sort {
                "local:///tmp/prepare-mode-sort".into()
            } else {
                String::new()
            },
            ..Default::default()
        }
    }

    fn inserted_rows(calls: &Arc<Mutex<Vec<(String, Vec<storage::Value>)>>>, table: &str) -> usize {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(sql, _)| {
                sql.to_ascii_lowercase()
                    .starts_with(&format!("insert into mysql.{table}"))
            })
            .count()
    }

    #[test]
    fn nextgen_submission_routes_jobs_and_tasks_to_go_keyspaces_and_uses_async_prepare() {
        let sys = Arc::new(Mutex::new(Vec::new()));
        let user = Arc::new(Mutex::new(Vec::new()));
        let sys_manager = manager(sys.clone());
        let user_manager = manager(user.clone());
        let table_mode = Arc::new(StorageSessionTableModeChanger);

        let system_service = StorageTaskSubmissionService::WithManagers(
            false,
            table_mode.clone(),
            sys_manager.clone(),
            sys_manager.clone(),
            "dxf_service".into(),
            false,
        );
        let submitted = SubmitTask(
            &system_service,
            &mut plan(false),
            "import into t from '/path/to/file'",
        )
        .unwrap();
        assert_eq!((submitted.JobID, submitted.TaskID), (11, 42));
        assert_eq!(inserted_rows(&sys, "tidb_import_jobs"), 1);
        assert_eq!(inserted_rows(&sys, "tidb_global_task"), 1);
        assert_eq!(inserted_rows(&user, "tidb_import_jobs"), 0);
        assert_eq!(inserted_rows(&user, "tidb_global_task"), 0);

        sys.lock().unwrap().clear();
        let user_service = StorageTaskSubmissionService::WithManagers(
            true,
            table_mode.clone(),
            user_manager,
            sys_manager.clone(),
            "dxf_service".into(),
            false,
        );
        SubmitTask(
            &user_service,
            &mut plan(false),
            "import into t from '/path/to/file'",
        )
        .unwrap();
        assert_eq!(inserted_rows(&user, "tidb_import_jobs"), 1);
        assert_eq!(inserted_rows(&user, "tidb_global_task"), 0);
        assert_eq!(inserted_rows(&sys, "tidb_global_task"), 1);
        assert_eq!(inserted_rows(&sys, "tidb_import_jobs"), 0);

        sys.lock().unwrap().clear();
        let mut global_plan = plan(true);
        SubmitTask(
            &system_service,
            &mut global_plan,
            "import into t from 's3://bucket/test.csv'",
        )
        .unwrap();
        assert_eq!((global_plan.ThreadCnt, global_plan.MaxNodeCnt), (1, 1));
        assert_eq!(inserted_rows(&sys, "tidb_import_jobs"), 1);
        assert_eq!(inserted_rows(&sys, "tidb_global_task"), 1);
        let task_insert = sys
            .lock()
            .unwrap()
            .iter()
            .find(|(sql, _)| sql.starts_with("insert into mysql.tidb_global_task"))
            .cloned()
            .unwrap();
        assert_eq!(task_insert.1[4], storage::Value::Int(1));
        assert_eq!(task_insert.1[8], storage::Value::Int(1));
        assert!(
            task_insert
                .1
                .iter()
                .any(|value| *value == storage::Value::Json("{\"prepare_mode\":1}".into()))
        );
    }
}

#[test]
/// 秒数格式化边界：跨天、一天内、不足一小时，应与 Go 期望一致。
fn import_progress_time_format_matches_go_boundaries() {
    assert_eq!(FormatSecondAsTime(86_400), "1 d 00:00:00");
    assert_eq!(FormatSecondAsTime(172_801), "2 d 00:00:01");
    assert_eq!(FormatSecondAsTime(86_399), "23:59:59");
    assert_eq!(FormatSecondAsTime(3_599), "00:59:59");
    assert_eq!(FormatSecondAsTime(28_800), "08:00:00");
}

#[test]
fn large_task_ids_isolate_active_subtasks_and_last_update_time() {
    use crate::job::{RuntimeInfoProvider, StorageRuntimeInfoProvider};
    use astersql_dxf_framework_storage as storage;
    let (_store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let session = astersql_session::runtime::ConcreteSession::new(domain);
    let manager = session.ImportTaskManager().unwrap();
    const ADJACENT: i64 = 9_007_199_254_740_992;
    const TARGET: i64 = ADJACENT + 1;
    // Explicit IDs avoid depending on allocator implementation, while using the real tables.
    for (id, job) in [(ADJACENT, 9528), (TARGET, 9527)] {
        manager.ExecuteSQLWithNewSession((), "insert into mysql.tidb_global_task(id, task_key, type, state, step, concurrency, meta, extra_params) values (%?, %?, 'ImportInto', 'running', 1, 8, '{}', '{}')", vec![id.into(), crate::job::TaskKey(job).into()]).unwrap();
        manager.ExecuteSQLWithNewSession((), "insert into mysql.tidb_background_subtask(task_key, step, exec_id, state, type, concurrency, state_update_time, meta, summary, checkpoint) values (%?, 1, 'large-id-exec', 'succeed', 1, 8, %?, '{}', '{}', '{}')", vec![id.to_string().into(), if id == TARGET {1000_i64} else {2000_i64}.into()]).unwrap();
    }
    let tasks = [TARGET, ADJACENT]
        .into_iter()
        .map(|id| manager.GetTaskByID((), id).unwrap())
        .collect();
    manager.TransferTasks2History((), tasks).unwrap();
    for id in [ADJACENT, TARGET] {
        manager.ExecuteSQLWithNewSession((), "insert into mysql.tidb_background_subtask(task_key, step, exec_id, state, type, concurrency, state_update_time, meta, summary, checkpoint) values (%?, 1, 'large-id-exec', 'pending', 1, 8, %?, '{}', '{}', '{}')", vec![id.to_string().into(), if id == TARGET {1500_i64} else {2500_i64}.into()]).unwrap();
    }
    let active = manager.GetActiveSubtasks((), TARGET).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].TaskID, TARGET);
    let filtered = manager
        .GetSubtasksByExecIDAndStepAndStates((), "large-id-exec".into(), TARGET, 1, vec!["pending"])
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].TaskID, TARGET);
    let provider = StorageRuntimeInfoProvider::WithManager(manager.clone());
    let runtime = provider.GetTaskRuntime(&crate::job::TaskKey(9527)).unwrap();
    assert_eq!(
        runtime.Subtasks.len(),
        1,
        "runtime summary must exclude adjacent task"
    );

    // ConcreteSession cannot yet evaluate FROM_UNIXTIME around an aggregate.
    // Keep the real query's tables, both bound keys, UNION/MAX and transaction;
    // adapt only its timestamp projection at the time conversion boundary.
    struct UnixTimestampProjection(storage::SQLExecutor);
    impl storage::SQLBackend for UnixTimestampProjection {
        fn execute(
            &self,
            sql: &str,
            args: Vec<storage::Value>,
        ) -> Result<storage::SQLResult, storage::Error> {
            let timestamp = sql.contains("FROM_UNIXTIME(max(state_update_time))");
            let query = sql.replace(
                "FROM_UNIXTIME(max(state_update_time))",
                "max(state_update_time)",
            );
            let mut rows = self.0.execute(query, args)?;
            if timestamp {
                rows = rows
                    .into_iter()
                    .map(|row| {
                        storage::chunk::Row::new(vec![if row.IsNull(0) {
                            storage::Value::Null
                        } else {
                            storage::Value::Time(storage::time::Unix(row.GetInt64(0), 0))
                        }])
                    })
                    .collect();
            }
            Ok(storage::SQLResult {
                rows,
                affected_rows: 0,
            })
        }
    }
    manager
        .WithNewSession(|session| {
            let projection = std::sync::Arc::new(UnixTimestampProjection(session.GetSQLExecutor()));
            let projected_manager = storage::NewTaskManager(storage::util::SessionPool::new(
                storage::sessionctx::Context::with_backend(projection),
            ));
            let provider = StorageRuntimeInfoProvider::WithManager(projected_manager);
            assert_eq!(
                provider
                    .GetJobLastUpdateTime(&crate::job::TaskKey(9527))
                    .unwrap(),
                Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1500))
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn large_task_ids_isolate_history_aggregation() {
    use astersql_dxf_importinto_jobhistory::{GetFromHistory, taskkey};
    let (_store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let session = astersql_session::runtime::ConcreteSession::new(domain);
    let manager = session.ImportTaskManager().unwrap();
    const ADJACENT: i64 = 9_007_199_254_740_992;
    const TARGET: i64 = ADJACENT + 1;
    let meta = r#"{"Plan":{"DistSQLScanConcurrency":16,"DesiredTableInfo":{"index_info":[{},{}],"cols":[{},{},{}]},"TotalFileSize":2147483648},"Summary":{"row-count":1024}}"#;
    for (id, job) in [(ADJACENT, 9528), (TARGET, 9527)] {
        manager.ExecuteSQLWithNewSession((), "insert into mysql.tidb_global_task(id, task_key, type, state, step, concurrency, max_node_count, meta, extra_params) values (%?, %?, 'ImportInto', 'pending', 0, 8, 4, %?, '{}')", vec![id.into(), taskkey::ForJobInKeyspace("ks1".into(), job).into(), meta.as_bytes().to_vec().into()]).unwrap();
    }
    use astersql_dxf_framework_proto as proto;
    for (id, step, group, bytes, start, end) in [
        (
            TARGET,
            proto::ImportStepEncodeAndSort,
            "data",
            1_073_741_824_i64,
            100_i64,
            700_i64,
        ),
        (
            TARGET,
            proto::ImportStepWriteAndIngest,
            "data",
            1_073_741_824,
            700,
            2500,
        ),
        (
            TARGET,
            proto::ImportStepWriteAndIngest,
            "index-1",
            536_870_912,
            900,
            2100,
        ),
        (TARGET, proto::ImportStepPostProcess, "data", 0, 0, 0),
        (
            ADJACENT,
            proto::ImportStepWriteAndIngest,
            "data",
            4_294_967_296,
            50,
            4000,
        ),
    ] {
        manager.ExecuteSQLWithNewSession((), "insert into mysql.tidb_background_subtask(task_key, step, exec_id, state, type, concurrency, start_time, state_update_time, meta, summary, checkpoint) values (%?, %?, 'tidb-1', 'succeed', 1, 8, %?, %?, %?, %?, '{}')", vec![id.to_string().into(), step.into(), start.into(), end.into(), format!("{{\"kv-group\":\"{group}\"}}").into(), format!("{{\"bytes\":{bytes}}}").into()]).unwrap();
    }
    let tasks = [TARGET, ADJACENT]
        .into_iter()
        .map(|id| manager.GetTaskByID((), id).unwrap())
        .collect();
    manager.TransferTasks2History((), tasks).unwrap();
    // JSON_LENGTH/JSON_UNQUOTE dispatch in ConcreteSession is a pre-existing gap.
    // Capture the production history query and exercise its changed task_key binding
    // against the real persisted rows. The jobhistory unit suite verifies the unchanged
    // aggregation/formatting separately; this boundary test never fabricates stored rows.
    use astersql_dxf_framework_storage::{TaskManager, Value, chunk::Row};
    let query_manager = TaskManager::new();
    query_manager.push_result(vec![Row::new(vec![
        TARGET.into(),
        "pending".into(),
        8_i64.into(),
        4_i64.into(),
        16_i64.into(),
        2_i64.into(),
        3_i64.into(),
        2_147_483_648_i64.into(),
        1024_i64.into(),
    ])]);
    GetFromHistory((), &query_manager, "ks1", 9527).unwrap();
    let calls = query_manager.calls();
    let history_call = &calls[1];
    assert!(history_call.sql.contains("where task_key = %?"));
    let rows = manager.ExecuteSQLWithNewSession((), "select task_key, step, meta, summary, start_time, state_update_time from mysql.tidb_background_subtask_history where task_key = %?", history_call.args.clone()).unwrap();
    assert_eq!(
        rows.len(),
        4,
        "the neighboring task must not enter history aggregation"
    );
    for row in &rows {
        assert_eq!(row.GetString(0), TARGET.to_string());
        assert!(row.GetInt64(5) <= 2500);
    }
    let bytes: i64 = rows
        .iter()
        .map(|row| {
            let summary: serde_json::Value = serde_json::from_slice(&row.GetBytes(3)).unwrap();
            summary["bytes"].as_i64().unwrap()
        })
        .sum();
    assert_eq!(bytes, 2_684_354_560);
    assert_eq!(history_call.args, vec![Value::String(TARGET.to_string())]);
}
