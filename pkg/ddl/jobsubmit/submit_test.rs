// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Job 提交路径的单元测试：校验全局 ID 分配顺序与数量。
//
// CreateTable 带两个分区时，需要 1（表）+ 2（分区）+ 1（job_id）共 4 个全局 ID；
// 分配后 table/partition/job 各字段应与输入 ID 切片一一对应。

use std::sync::{Arc, Mutex};

use crate::{
    BdrPolicy, BeforeInsert, Error, ErrorKind, Job, JobArgs, JobSpec, JobState, JobType,
    MinJobIdProvider, PartitionDefinition, PartitionInfo, ServerState, Session, SessionPool,
    SubmitOptions, SystemTableManager, TableInfo, assign_global_ids_for_jobs,
    generate_ids_and_insert_jobs_with_retry, make_string_for_ids, required_global_id_count,
    submit_batch,
};

/// 验证 CreateTable（含分区）所需 ID 数量，以及表/分区/job ID 的分配顺序。
#[test]
fn canonical_submit_assigns_job_table_and_partition_global_ids() {
    let mut specs = vec![JobSpec {
        job: Job {
            version: 2,
            job_type: JobType::CreateTable,
            ..Job::default()
        },
        args: JobArgs::CreateTable {
            table: TableInfo {
                id: 0,
                partitions: Some(PartitionInfo {
                    definitions: vec![PartitionDefinition { id: 0 }, PartitionDefinition { id: 0 }],
                    new_table_id: 0,
                }),
            },
        },
        id_allocated: false,
    }];
    // 表 100、分区 101/102、job 103。
    assert_eq!(required_global_id_count(&specs), 4);
    assign_global_ids_for_jobs(&mut specs, &[100, 101, 102, 103]).unwrap();
    assert_eq!(specs[0].job.id, 103);
    assert_eq!(specs[0].job.table_id, 100);
    let JobArgs::CreateTable { table } = &specs[0].args else {
        panic!("create-table arguments must remain available");
    };
    assert_eq!(table.id, 100);
    assert_eq!(
        table
            .partitions
            .as_ref()
            .unwrap()
            .definitions
            .iter()
            .map(|partition| partition.id)
            .collect::<Vec<_>>(),
        vec![101, 102]
    );
}

#[test]
fn action_type_codes_match_persisted_tidb_values() {
    assert_eq!(JobType::CreateSchema.code(), 1);
    assert_eq!(JobType::CreateTable.code(), 3);
    assert_eq!(JobType::CreateView.code(), 21);
    assert_eq!(JobType::CreateSequence.code(), 34);
    assert_eq!(JobType::ExchangeTablePartition.code(), 42);
    assert_eq!(JobType::RenameTables.code(), 47);
    assert_eq!(JobType::CreateTables.code(), 60);
    assert_eq!(JobType::MultiSchemaChange.code(), 61);
    assert_eq!(JobType::CreateResourceGroup.code(), 68);
    assert_eq!(JobType::ReorganizePartition.code(), 64);
    assert_eq!(JobType::AlterTablePartitioning.code(), 71);
    assert_eq!(JobType::RemovePartitioning.code(), 72);
    assert_eq!(JobType::AlterTableMode.code(), 75);
}

#[test]
fn persisted_id_lists_use_go_lexicographic_order() {
    assert_eq!(make_string_for_ids([2, 10, 2, -1]), "-1,10,2");
}

#[test]
fn job_normalization_preserves_order_and_matches_scheduler_names() {
    let mut job = Job {
        schema_name: "TestDB".into(),
        table_name: "T1".into(),
        involving_schemas: vec![
            ("TestDB".into(), "T1".into()),
            ("AnotherDB".into(), "*".into()),
            ("*".into(), "*".into()),
            ("TestDB".into(), "T1".into()),
        ],
        ..Job::default()
    };

    job.normalize_involving_schema_info();
    assert_eq!(job.schema_name, "testdb");
    assert_eq!(job.table_name, "t1");
    assert_eq!(
        job.involving_schemas,
        vec![
            ("testdb".into(), "t1".into()),
            ("anotherdb".into(), "*".into()),
            ("*".into(), "*".into()),
            ("testdb".into(), "t1".into()),
        ]
    );
}

#[test]
fn involving_schema_check_uses_job_name_fallback_and_wildcards() {
    assert!(Job::default().check_involving_schema_info().is_err());
    let mut table_job = Job {
        schema_name: "test".into(),
        table_name: "t1".into(),
        ..Job::default()
    };
    assert!(table_job.check_involving_schema_info().is_ok());

    table_job.table_name.clear();
    assert!(table_job.check_involving_schema_info().is_ok());

    table_job.schema_name = "*".into();
    assert!(table_job.check_involving_schema_info().is_ok());

    table_job.table_name = "t1".into();
    assert!(table_job.check_involving_schema_info().is_err());
}

#[test]
fn may_need_reorg_matches_job_type_and_sub_job_rules() {
    for job_type in [
        JobType::AddIndex,
        JobType::AddPrimaryKey,
        JobType::ReorganizePartition,
        JobType::RemovePartitioning,
        JobType::AlterTablePartitioning,
    ] {
        assert!(
            Job {
                job_type,
                ..Job::default()
            }
            .may_need_reorg()
        );
    }
    assert!(
        !Job {
            job_type: JobType::TruncateTable,
            ..Job::default()
        }
        .may_need_reorg()
    );
    assert!(
        Job {
            job_type: JobType::MultiSchemaChange,
            sub_jobs: vec![crate::SubJob {
                job_type: Some(JobType::ModifyColumn),
                need_reorg: true,
                ..crate::SubJob::default()
            }],
            ..Job::default()
        }
        .may_need_reorg()
    );
}

#[derive(Default)]
struct MockSessionState {
    begin_calls: usize,
    rollback_calls: usize,
    commit_calls: usize,
    execute_calls: usize,
    put_calls: usize,
    execute_failures: usize,
    lock_conflicts: usize,
    flashback_job: bool,
    upgrading: bool,
    bdr_denied: bool,
    bdr_role: String,
    start_ts: u64,
    next_id: i64,
    executed_sql: Vec<String>,
}

struct MockSession {
    state: Arc<Mutex<MockSessionState>>,
}

impl Session for MockSession {
    fn begin(&mut self) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        state.begin_calls += 1;
        Ok(())
    }

    fn rollback(&mut self) {
        self.state.lock().unwrap().rollback_calls += 1;
    }

    fn commit(&mut self) -> Result<(), Error> {
        self.state.lock().unwrap().commit_calls += 1;
        Ok(())
    }

    fn read_bdr_role_and_start_ts(&mut self) -> Result<(String, u64), Error> {
        let state = self.state.lock().unwrap();
        Ok((state.bdr_role.clone(), state.start_ts))
    }

    fn transaction_start_ts(&self) -> Result<u64, Error> {
        Ok(10)
    }

    fn set_pessimistic(&mut self) {}

    fn lock_global_id_key(&mut self, _for_update_ts: u64) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        if state.lock_conflicts > 0 {
            state.lock_conflicts -= 1;
            return Err(Error {
                kind: ErrorKind::WriteConflict,
                message: "write conflict".into(),
            });
        }
        Ok(())
    }

    fn current_version(&self) -> Result<u64, Error> {
        Ok(20)
    }

    fn set_snapshot_ts(&mut self, _timestamp: u64) {}

    fn generate_global_ids(&mut self, count: usize) -> Result<Vec<i64>, Error> {
        let mut state = self.state.lock().unwrap();
        let first = state.next_id;
        state.next_id += count as i64;
        Ok((0..count).map(|offset| first + offset as i64).collect())
    }

    fn execute(&mut self, sql: &str, _label: &str) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        state.execute_calls += 1;
        state.executed_sql.push(sql.to_owned());
        if state.execute_failures > 0 {
            state.execute_failures -= 1;
            return Err(Error {
                kind: ErrorKind::Retryable,
                message: "retryable insert failure".into(),
            });
        }
        Ok(())
    }
}

struct MockSessionPool {
    state: Arc<Mutex<MockSessionState>>,
}

impl SessionPool for MockSessionPool {
    fn get(&self) -> Result<Box<dyn Session>, Error> {
        Ok(Box::new(MockSession {
            state: Arc::clone(&self.state),
        }))
    }

    fn put(&self, _session: Box<dyn Session>) {
        self.state.lock().unwrap().put_calls += 1;
    }
}

struct MockSystemTableManager {
    state: Arc<Mutex<MockSessionState>>,
}

impl SystemTableManager for MockSystemTableManager {
    fn has_flashback_cluster_job(&self, _min_job_id: i64) -> Result<bool, Error> {
        Ok(self.state.lock().unwrap().flashback_job)
    }
}

struct MockMinJobID;

impl MinJobIdProvider for MockMinJobID {
    fn current_min_job_id(&self) -> i64 {
        0
    }
}

struct MockServerState {
    state: Arc<Mutex<MockSessionState>>,
}

impl ServerState for MockServerState {
    fn is_upgrading(&self) -> bool {
        self.state.lock().unwrap().upgrading
    }
}

struct MockBdrPolicy {
    state: Arc<Mutex<MockSessionState>>,
}

impl BdrPolicy for MockBdrPolicy {
    fn is_denied(&self, _role: &str, _job_type: JobType, _args: &JobArgs) -> bool {
        self.state.lock().unwrap().bdr_denied
    }
}

fn test_options(state: &Arc<Mutex<MockSessionState>>) -> SubmitOptions {
    SubmitOptions {
        session_pool: Arc::new(MockSessionPool {
            state: Arc::clone(state),
        }),
        system_table_manager: Arc::new(MockSystemTableManager {
            state: Arc::clone(state),
        }),
        min_job_id_provider: Arc::new(MockMinJobID),
        server_state: Some(Arc::new(MockServerState {
            state: Arc::clone(state),
        })),
        bdr_policy: Arc::new(MockBdrPolicy {
            state: Arc::clone(state),
        }),
        before_insert_with_assigned_ids: None,
        max_retry_count: 2,
        backoff: Arc::new(|_| {}),
    }
}

fn submit_spec(job_type: JobType) -> JobSpec {
    JobSpec {
        job: Job {
            version: 2,
            schema_name: "TestDB".into(),
            table_name: "T1".into(),
            involving_schemas: vec![("TestDB".into(), "T1".into())],
            job_type,
            ..Job::default()
        },
        args: JobArgs::None,
        id_allocated: true,
    }
}

#[test]
fn submit_batch_enqueues_job_and_returns_session_to_pool() {
    let state = Arc::new(Mutex::new(MockSessionState {
        bdr_role: "none".into(),
        start_ts: 42,
        next_id: 100,
        ..Default::default()
    }));
    let options = test_options(&state);
    let mut spec = submit_spec(JobType::AlterTableMode);

    submit_batch(&options, std::slice::from_mut(&mut spec)).unwrap();

    assert_eq!(spec.job.id, 100);
    assert_eq!(spec.job.state, JobState::Queueing);
    assert_eq!(spec.job.start_ts, 42);
    assert_eq!(spec.job.bdr_role, "none");
    assert!(spec.job.trace_info_present);
    assert_eq!(spec.job.schema_name, "testdb");
    assert_eq!(spec.job.table_name, "t1");
    let state = state.lock().unwrap();
    assert_eq!(state.begin_calls, 1);
    assert_eq!(state.commit_calls, 1);
    assert_eq!(state.put_calls, 1);
    assert_eq!(state.execute_calls, 1);
}

#[test]
fn submit_batch_checks_flashback_bdr_and_upgrade_state() {
    let state = Arc::new(Mutex::new(MockSessionState {
        flashback_job: true,
        ..Default::default()
    }));
    let options = test_options(&state);
    assert!(submit_batch(&options, &mut [submit_spec(JobType::AlterTableMode)]).is_err());

    let state = Arc::new(Mutex::new(MockSessionState {
        bdr_role: "primary".into(),
        bdr_denied: true,
        ..Default::default()
    }));
    let options = test_options(&state);
    assert!(submit_batch(&options, &mut [submit_spec(JobType::AlterTableMode)]).is_err());

    let state = Arc::new(Mutex::new(MockSessionState {
        start_ts: 9,
        next_id: 10,
        upgrading: true,
        ..Default::default()
    }));
    let options = test_options(&state);
    let mut spec = submit_spec(JobType::AlterTableMode);
    submit_batch(&options, std::slice::from_mut(&mut spec)).unwrap();
    assert_eq!(spec.job.state, JobState::Pausing);
    assert!(spec.job.admin_operator_system);
}

#[test]
fn submit_batch_retry_runs_cleanup_only_for_failed_attempt() {
    let state = Arc::new(Mutex::new(MockSessionState {
        execute_failures: 1,
        next_id: 100,
        ..Default::default()
    }));
    let assigned_ids = Arc::new(Mutex::new(Vec::new()));
    let cleanup_ids = Arc::new(Mutex::new(Vec::new()));
    let assigned_ids_for_hook = Arc::clone(&assigned_ids);
    let cleanup_ids_for_hook = Arc::clone(&cleanup_ids);
    let hook: BeforeInsert = Arc::new(move |specs| {
        let id = specs[0].job.id;
        assigned_ids_for_hook.lock().unwrap().push(id);
        let cleanup_ids = Arc::clone(&cleanup_ids_for_hook);
        Some(Box::new(move || cleanup_ids.lock().unwrap().push(id)))
    });
    let mut options = test_options(&state);
    options.before_insert_with_assigned_ids = Some(hook);
    let mut spec = submit_spec(JobType::AlterTableMode);

    submit_batch(&options, std::slice::from_mut(&mut spec)).unwrap();

    assert_eq!(*assigned_ids.lock().unwrap(), vec![100, 101]);
    assert_eq!(*cleanup_ids.lock().unwrap(), vec![100]);
    let state = state.lock().unwrap();
    assert_eq!(state.rollback_calls, 1);
    assert_eq!(state.execute_calls, 2);
    assert_eq!(spec.job.id, 101);
}

#[test]
fn lock_global_id_retries_write_conflicts_with_backoff() {
    let state = Arc::new(Mutex::new(MockSessionState {
        lock_conflicts: 2,
        next_id: 100,
        ..Default::default()
    }));
    let backoff_attempts = Arc::new(Mutex::new(Vec::new()));
    let mut options = test_options(&state);
    let backoff_attempts_for_hook = Arc::clone(&backoff_attempts);
    options.backoff =
        Arc::new(move |attempt| backoff_attempts_for_hook.lock().unwrap().push(attempt));
    let mut session = MockSession {
        state: Arc::clone(&state),
    };
    let mut spec = submit_spec(JobType::AlterTableMode);

    generate_ids_and_insert_jobs_with_retry(
        &mut session,
        std::slice::from_mut(&mut spec),
        &options,
    )
    .unwrap();

    assert_eq!(*backoff_attempts.lock().unwrap(), vec![0, 1]);
}

#[test]
fn begin_failure_does_not_rollback_an_unstarted_transaction() {
    struct BeginFailSession {
        rollback_calls: Arc<Mutex<usize>>,
    }
    impl Session for BeginFailSession {
        fn begin(&mut self) -> Result<(), Error> {
            Err(Error::invalid("begin failed"))
        }
        fn rollback(&mut self) {
            *self.rollback_calls.lock().unwrap() += 1;
        }
        fn commit(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn read_bdr_role_and_start_ts(&mut self) -> Result<(String, u64), Error> {
            Ok(("none".into(), 1))
        }
        fn transaction_start_ts(&self) -> Result<u64, Error> {
            Ok(1)
        }
        fn set_pessimistic(&mut self) {}
        fn lock_global_id_key(&mut self, _: u64) -> Result<(), Error> {
            Ok(())
        }
        fn current_version(&self) -> Result<u64, Error> {
            Ok(1)
        }
        fn set_snapshot_ts(&mut self, _: u64) {}
        fn generate_global_ids(&mut self, _: usize) -> Result<Vec<i64>, Error> {
            Ok(vec![1])
        }
        fn execute(&mut self, _: &str, _: &str) -> Result<(), Error> {
            Ok(())
        }
    }

    let mut spec = submit_spec(JobType::AlterTableMode);
    let state = Arc::new(Mutex::new(MockSessionState::default()));
    let options = test_options(&state);
    let rollback_calls = Arc::new(Mutex::new(0));
    let mut session = BeginFailSession {
        rollback_calls: Arc::clone(&rollback_calls),
    };
    assert!(
        generate_ids_and_insert_jobs_with_retry(
            &mut session,
            std::slice::from_mut(&mut spec),
            &options
        )
        .is_err()
    );
    assert_eq!(*rollback_calls.lock().unwrap(), 0);
}

#[test]
fn canonical_submit_preserves_complete_reorg_metadata() {
    use astersql_meta_model::group_3 as model;
    let meta: model::DDLReorgMeta = serde_json::from_value(serde_json::json!({
        "sql_mode": 2097152,
        "location": {"name":"", "offset":28800},
        "reorg_tp": 3,
        "is_fast_reorg": true,
        "is_dist_reorg": true,
        "use_cloud_storage": true,
        "resource_group_name": "ddl_group",
        "version": 1,
        "target_scope": "background",
        "max_node_count": 3,
        "analyze_state": 2,
        "stage": 2,
        "use_new_collate": true,
        "concurrency": 8,
        "batch_size": 128,
        "max_write_speed": 1048576
    }))
    .unwrap();
    let expected = serde_json::to_value(&meta).unwrap();
    let job = Job {
        version: 2,
        job_type: JobType::ModifyColumn,
        need_reorg: true,
        reorg_meta: Some(Arc::new(meta)),
        ..Default::default()
    };
    let decoded = model::Job::decode(&job.encode(&JobArgs::Opaque(b"{}".to_vec()))).unwrap();
    assert_eq!(
        serde_json::to_value(decoded.reorg_meta.unwrap()).unwrap(),
        expected
    );
}
