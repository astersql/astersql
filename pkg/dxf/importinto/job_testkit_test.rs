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

// IMPORT INTO job 相关 testkit 集成测试与可执行边界用例。

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
