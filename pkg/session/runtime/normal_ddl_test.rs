// Copyright 2026 AsterSQL.

use super::{CreateAnalyzeSession, system_session::SystemSessionPool};
use astersql_ddl::job_scheduler::JobScheduler;
use astersql_ddl::job_worker::{
    DurableJobExecutor, DurableJobSession, JobLease, JobWorker, WorkerType,
};
use astersql_ddl::table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor};
use astersql_meta_model::group_3::{Job, JobState};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, Ordering},
};
struct Lease(AtomicBool);
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct Barrier {
    fail: bool,
    seen: Vec<i64>,
}
impl DdlSchemaBarrier for Barrier {
    fn recover(&mut self, job: &Job, lease: &dyn JobLease) -> Result<(), String> {
        self.wait(job, job.last_schema_version, lease)
    }
    fn wait(&mut self, _: &Job, version: i64, lease: &dyn JobLease) -> Result<(), String> {
        if !lease.is_owner() {
            return Err("lost owner".into());
        }
        if version > 0 {
            self.seen.push(version);
            if self.fail {
                return Err("schema unavailable".into());
            }
        }
        Ok(())
    }
}
struct Policy(Option<String>);
impl DdlJobPolicy for Policy {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, job: &Job) -> Result<bool, String> {
        Ok(job.state != JobState::Paused)
    }
    fn error_limit(&self) -> i64 {
        3
    }
    fn mdl_owner(&self) -> Option<String> {
        self.0.clone()
    }
}
fn scheduler() -> JobScheduler {
    JobScheduler::new(
        JobWorker::new(WorkerType::General),
        JobWorker::new(WorkerType::AddIndex),
    )
}
use super::normal_ddl_fixture::{Fixture, hash, hex};
fn executor() -> NormalDdlExecutor<Barrier, Policy> {
    NormalDdlExecutor {
        barrier: Barrier {
            fail: false,
            seen: vec![],
        },
        policy: Policy(None),
        sequence: Arc::new(AtomicI64::new(0)),
    }
}
#[test]
fn crossks_align_normal_ddl_persists_table_then_synced_history_after_barrier() {
    let f = Fixture::new();
    f.insert(81001, JobState::Queueing);
    let lease = Lease(AtomicBool::new(true));
    let mut e = executor();
    let mut session = f.pool.acquire().unwrap();
    let mut s = scheduler();
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    let job = f.queue(81001).unwrap();
    assert_eq!(job.state, JobState::Done);
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(table.Mode, astersql_meta_model::TableMode::TableModeImport);
    assert_eq!(table.Columns.len(), 2);
    assert_eq!(
        job.binlog_info
            .as_ref()
            .unwrap()
            .table_info
            .as_ref()
            .unwrap()
            .ID,
        f.table
    );
    assert!(f.reader().get_history_ddl_job(81001).unwrap().is_none());
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert!(f.queue(81001).is_none());
    let history = f.reader().get_history_ddl_job(81001).unwrap().unwrap();
    assert_eq!(history.state, JobState::Synced);
    assert_eq!(history.cdc_write_source, 41);
    assert!(history.binlog_info.unwrap().finished_ts > 0);
}

fn version(f: &Fixture) -> i64 {
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let version = astersql_kv::GetInt64(
        &astersql_kv::Context::default(),
        txn.as_ref(),
        &astersql_meta::transaction_meta_string_key(b"SchemaVersionKey"),
    )
    .unwrap();
    txn.Rollback().unwrap();
    version
}
fn mode(f: &Fixture) -> astersql_meta_model::TableMode {
    f.reader().get_table(f.db, f.table).unwrap().unwrap().Mode
}
fn set_mode(f: &Fixture, mode: astersql_meta_model::TableMode) {
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Mode = mode;
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
}
#[test]
fn crossks_align_normal_ddl_pause_cancel_and_invalid_transition_preserve_metadata() {
    for scenario in 0..3 {
        let f = Fixture::new();
        let state = if scenario == 0 {
            JobState::Paused
        } else if scenario == 1 {
            JobState::Cancelling
        } else {
            JobState::Queueing
        };
        if scenario == 2 {
            set_mode(&f, astersql_meta_model::TableMode::TableModeRestore)
        }
        let before = version(&f);
        let original = mode(&f);
        f.insert(81002, state);
        let lease = Lease(AtomicBool::new(true));
        let mut e = executor();
        let mut session = f.pool.acquire().unwrap();
        let count = scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap();
        assert_eq!(version(&f), before);
        assert_eq!(mode(&f), original);
        if scenario == 0 {
            assert_eq!(count, 0);
            assert_eq!(f.queue(81002).unwrap().state, JobState::Paused);
            assert!(f.reader().get_history_ddl_job(81002).unwrap().is_none())
        } else {
            assert_eq!(count, 1);
            assert!(f.queue(81002).is_none());
            let job = f.reader().get_history_ddl_job(81002).unwrap().unwrap();
            assert_eq!(job.state, JobState::Cancelled);
            assert_eq!(job.error_count, 1);
            assert!(
                job.error
                    .unwrap()
                    .contains(if scenario == 1 { "8214" } else { "8259" })
            );
        }
    }
}
#[test]
fn crossks_align_normal_ddl_sync_failure_restart_and_same_mode_are_idempotent() {
    let f = Fixture::new();
    f.insert(81003, JobState::Queueing);
    let before = version(&f);
    let lease = Lease(AtomicBool::new(true));
    let mut e = executor();
    e.barrier.fail = true;
    let mut session = f.pool.acquire().unwrap();
    assert!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap_err()
            .contains("schema unavailable")
    );
    assert_eq!(version(&f), before + 1);
    assert_eq!(f.queue(81003).unwrap().state, JobState::Done);
    assert!(f.reader().get_history_ddl_job(81003).unwrap().is_none());
    assert!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .is_err()
    );
    assert_eq!(version(&f), before + 1);
    let mut restarted = executor();
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut restarted, 0)
            .unwrap(),
        1
    );
    assert_eq!(version(&f), before + 1);
    assert_eq!(
        f.reader()
            .get_history_ddl_job(81003)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    // A second request for the already current mode completes with no new diff.
    let revision = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Revision;
    f.insert(81004, JobState::Queueing);
    let mut s = scheduler();
    s.schedule_persisted(&mut session, &lease, &mut restarted, 0)
        .unwrap();
    s.schedule_persisted(&mut session, &lease, &mut restarted, 0)
        .unwrap();
    assert_eq!(version(&f), before + 1);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Revision,
        revision
    );
}
struct RetiringSession<'a> {
    inner: &'a mut dyn DurableJobSession,
    lease: &'a Lease,
}
impl DurableJobSession for RetiringSession<'_> {
    fn with_execution_context(
        &mut self,
        operation: astersql_ddl::job_worker::ExecutionOperation,
    ) -> Result<Vec<u8>, String> {
        let output = self.inner.with_execution_context(operation)?;
        self.lease.0.store(false, Ordering::Release);
        Ok(output)
    }
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        self.inner.query(sql, label)
    }
    fn begin(&mut self) -> Result<(), String> {
        self.inner.begin()
    }
    fn commit(&mut self) -> Result<(), String> {
        self.inner.commit()
    }
    fn rollback(&mut self) {
        self.inner.rollback()
    }
    fn with_transaction(
        &mut self,
        operation: astersql_ddl::job_worker::TransactionOperation,
    ) -> Result<Vec<u8>, String> {
        let output = self.inner.with_transaction(operation)?;
        self.lease.0.store(false, Ordering::Release);
        Ok(output)
    }
}
#[test]
fn crossks_align_normal_ddl_owner_loss_rolls_back_table_diff_job_and_history() {
    let f = Fixture::new();
    f.insert(81005, JobState::Queueing);
    let before = version(&f);
    let lease = Lease(AtomicBool::new(true));
    let mut e = executor();
    let mut session = f.pool.acquire().unwrap();
    let mut retiring = RetiringSession {
        inner: &mut session,
        lease: &lease,
    };
    assert!(
        scheduler()
            .schedule_persisted(&mut retiring, &lease, &mut e, 0)
            .unwrap_err()
            .contains("not DDL owner")
    );
    assert_eq!(version(&f), before);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
    assert_eq!(f.queue(81005).unwrap().state, JobState::Queueing);
    assert!(f.reader().get_history_ddl_job(81005).unwrap().is_none());
}
#[test]
fn crossks_align_normal_ddl_malformed_args_persist_errors_and_cancel_at_limit() {
    let f = Fixture::new();
    f.insert(81006, JobState::Queueing);
    let before = version(&f);
    let mut job = f.queue(81006).unwrap();
    job.raw_args = b"[]".to_vec();
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id=81006",
            hex(&job.encode(false).unwrap())
        ))
        .unwrap();
    let lease = Lease(AtomicBool::new(true));
    let mut e = executor();
    let mut session = f.pool.acquire().unwrap();
    for n in 1..=4 {
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap();
        let job = f.queue(81006).unwrap();
        assert_eq!(job.error_count, n);
        assert!(job.error.unwrap().contains("invalid TableMode arguments"));
        assert_eq!(
            job.state,
            if n == 4 {
                JobState::Cancelling
            } else {
                JobState::Running
            }
        );
        assert_eq!(version(&f), before);
    }
    scheduler()
        .schedule_persisted(&mut session, &lease, &mut e, 0)
        .unwrap();
    assert!(f.queue(81006).is_none());
    assert_eq!(
        f.reader()
            .get_history_ddl_job(81006)
            .unwrap()
            .unwrap()
            .state,
        JobState::Cancelled
    );
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
}

#[test]
fn crossks_align_normal_ddl_public_schema_barrier_waits_for_follower() {
    use astersql_ddl_schemaver::{
        Context, DDLAllSchemaVersions, DDLGlobalSchemaVersion, EtcdClient, MemoryEtcdClient,
        NewEtcdSyncer,
    };
    let f = Fixture::new();
    f.insert(81007, JobState::Queueing);
    let before = version(&f);
    let client = Arc::new(MemoryEtcdClient::default());
    let context = Context::Background().WithTimeout(std::time::Duration::from_secs(20));
    let follower = format!("{DDLAllSchemaVersions}/task8-follower");
    client.Put(&context, &follower, "0", None).unwrap();
    let snapshot_domain = f.domain.clone();
    let session_pool = f.pool.clone();
    let barrier = astersql_ddl::schema_version::NormalDdlSchemaBarrier {
        syncer: NewEtcdSyncer(client.clone(), "task8-owner"),
        context: context.clone(),
        etcd: Some(client.clone()),
        snapshot: Box::new(move || {
            snapshot_domain
                .storage_handle()
                .with_storage(|s| {
                    let v = s.CurrentVersion("global")?;
                    Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
                })
                .map_err(|e| e.to_string())
        }),
        session: Box::new(move || Ok(Box::new(session_pool.acquire()?))),
        mdl_enabled: false,
        owner_id: "task8-owner".into(),
        nextgen: true,
    };
    let mut e = NormalDdlExecutor {
        barrier,
        policy: Policy(None),
        sequence: Arc::new(AtomicI64::new(0)),
    };
    let watcher = client.clone();
    let pool = f.pool.clone();
    let follower_key = follower.clone();
    let watcher_context = context.clone();
    let updater = std::thread::spawn(move || {
        loop {
            assert!(!watcher_context.Done(), "owner never published version");
            let rows = watcher
                .Get(&watcher_context, DDLGlobalSchemaVersion, false)
                .unwrap();
            if rows
                .Kvs
                .first()
                .is_some_and(|kv| kv.Value == format!("{}", before + 1).as_bytes())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let rows = pool
            .acquire()
            .unwrap()
            .query("SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id=81007")
            .unwrap();
        assert_eq!(
            Job::decode(rows[0][0].as_bytes()).unwrap().state,
            JobState::Done
        );
        assert!(
            pool.acquire()
                .unwrap()
                .query("SELECT job_id FROM mysql.tidb_ddl_history WHERE job_id=81007")
                .unwrap()
                .is_empty()
        );
        watcher
            .Put(
                &watcher_context,
                &follower_key,
                &format!("{}", before + 1),
                None,
            )
            .unwrap();
    });
    let lease = Lease(AtomicBool::new(true));
    let mut session = f.pool.acquire().unwrap();
    let mut s = scheduler();
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    updater.join().unwrap();
    assert_eq!(f.queue(81007).unwrap().state, JobState::Done);
    s.schedule_persisted(&mut session, &lease, &mut e, 0)
        .unwrap();
    assert_eq!(
        f.reader()
            .get_history_ddl_job(81007)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT job_id FROM mysql.tidb_ddl_history WHERE job_id=81007")
            .unwrap()
            .len(),
        1
    );
    context.Cancel();
}
#[test]
fn crossks_align_normal_ddl_registers_mdl_in_metadata_commit() {
    let f = Fixture::new();
    f.insert(81008, JobState::Queueing);
    let lease = Lease(AtomicBool::new(true));
    let mut e = executor();
    e.policy.0 = Some("normal-owner".into());
    let mut session = f.pool.acquire().unwrap();
    scheduler()
        .schedule_persisted(&mut session, &lease, &mut e, 0)
        .unwrap();
    let job = f.queue(81008).unwrap();
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT version, table_ids, owner_id FROM mysql.tidb_mdl_info WHERE job_id=81008")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], job.last_schema_version.to_string());
    assert_eq!(rows[0][1], f.table.to_string());
    assert_eq!(rows[0][2], "normal-owner");
    assert!(f.reader().get_history_ddl_job(81008).unwrap().is_none());
}

#[test]
fn crossks_align_normal_ddl_missing_nonpublic_and_unknown_mode_cancel_without_mutation() {
    for scenario in 0..3 {
        let f = Fixture::new();
        f.insert(81009, JobState::Queueing);
        let before = version(&f);
        let key = hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        );
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        let mut raw: serde_json::Value = serde_json::from_slice(
            &txn.Get(&astersql_kv::Context::default(), key.clone(), &[])
                .unwrap()
                .Value,
        )
        .unwrap();
        if scenario == 0 {
            txn.Delete(key.clone()).unwrap();
        } else {
            if scenario == 1 {
                raw["state"] = serde_json::json!(0);
            } else {
                raw["mode"] = serde_json::json!(3);
            }
            txn.Set(key.clone(), serde_json::to_vec(&raw).unwrap())
                .unwrap();
        }
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        let lease = Lease(AtomicBool::new(true));
        let mut e = executor();
        let mut session = f.pool.acquire().unwrap();
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap();
        assert_eq!(version(&f), before);
        assert!(f.queue(81009).is_none());
        let job = f.reader().get_history_ddl_job(81009).unwrap().unwrap();
        assert_eq!(job.state, JobState::Cancelled);
        assert!(job.error.unwrap().contains(match scenario {
            0 => "1146",
            1 => "8210",
            _ => "8259",
        }));
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        let stored = txn.Get(&astersql_kv::Context::default(), key, &[]);
        if scenario == 0 {
            assert!(stored.is_err())
        } else {
            let now: serde_json::Value = serde_json::from_slice(&stored.unwrap().Value).unwrap();
            assert_eq!(now, raw);
        }
        txn.Rollback().unwrap();
    }
}

#[test]
fn crossks_align_normal_ddl_consumes_task6_submit_only_backend_job() {
    use astersql_domain_crossks::{
        AlterTableModeTarget, Cancellation, DdlBackend, DdlClient, Error, HistoryJobState,
        SubmitOnlyBackend, TableMode,
    };
    let f = Fixture::new();
    f.domain
        .set_global_system_variable("tidb_cdc_write_source", "9");
    let manager = astersql_ddl_systable::new_manager(f.pool.clone());
    let min_id = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
        manager.clone(),
    ));
    min_id.refresh(&astersql_ddl_systable::Context::default());
    let state = Arc::new(astersql_ddl_serverstate::EtcdSyncer::with_client(
        Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default()),
        "/tidb/server/global_state",
    ));
    let options = f.pool.table_mode_submit_options(
        manager,
        min_id,
        Some(Arc::new(super::system_session::JobSubmitServerState(
            state.clone(),
        ))),
    );
    let storage = f.domain.storage_handle();
    let pool = f.pool.clone();
    let backend = Arc::new(SubmitOnlyBackend::new(
        options,
        Arc::new(move || {
            storage
                .with_storage(|s| {
                    let v = s.CurrentVersion("global")?;
                    Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
                })
                .map_err(|e| Error(e.to_string()))
        }),
        Arc::new(move || {
            pool.acquire()
                .map_err(Error)?
                .ddl_session_variables()
                .map_err(Error)
        }),
        Arc::new(move || {
            use astersql_ddl_serverstate::Syncer;
            state
                .get_global_state(&astersql_ddl_serverstate::SyncContext::new())
                .map(|_| ())
                .map_err(|e| Error(e.to_string()))
        }),
        None,
    ));
    let client = DdlClient::new(backend.clone());
    let target = client
        .resolve_alter_table_mode_target(AlterTableModeTarget {
            schema_id: f.db,
            table_id: f.table,
            schema_name: "test".into(),
            table_name: "normal_ddl_target".into(),
            current_mode: TableMode::Restore,
            target_mode: TableMode::Import,
        })
        .unwrap();
    let mut job = client.build_alter_table_mode_job(&target).unwrap().unwrap();
    backend.refresh_server_state().unwrap();
    backend.submit(&mut job).unwrap();
    assert!(job.id > 0);
    assert_eq!(f.queue(job.id).unwrap().cdc_write_source, 9);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
    assert!(backend.history_job(job.id).unwrap().is_none());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let manager = astersql_owner::NewMockManager(
        astersql_owner::Context::new(),
        "task8-normal-owner",
        None,
        "/test/task8-normal-owner",
    );
    let lease = super::system_session::DdlOwnerLease {
        owner: manager.clone(),
        cancellation: Arc::new(astersql_session_syssession::CancellationToken::default()),
    };
    let mut e = executor();
    let mut session = f.pool.acquire().unwrap();
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        0
    );
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
    runtime.block_on(manager.CampaignOwner(&[])).unwrap();
    let mut s = scheduler();
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert_eq!(f.queue(job.id).unwrap().state, JobState::Done);
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert!(matches!(
        backend.history_job(job.id).unwrap(),
        Some(HistoryJobState::Synced)
    ));
    client
        .wait_ddl_finished(&Cancellation::default(), job.id)
        .unwrap();
    assert!(f.queue(job.id).is_none());
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
    runtime.block_on(manager.Close());
}

#[test]
fn crossks_align_normal_ddl_dispatches_create_schema_with_table_mode_in_one_queue() {
    let f = Fixture::new();
    let db = astersql_meta_model::DBInfo {
        ID: 91001,
        Name: astersql_meta_model::ast::NewCIStr("mixed_queue_schema"),
        Charset: "utf8mb4".into(),
        Collate: "utf8mb4_bin".into(),
        State: astersql_meta_model::SchemaState::None,
        ..Default::default()
    };
    let mut job = Job::default();
    job.id = 91001;
    job.tp = 1;
    job.schema_id = db.ID;
    job.schema_name = db.Name.O.clone();
    job.state = JobState::Queueing;
    job.version = astersql_meta_model::group_3::JobVersion::V2;
    job.raw_args = serde_json::to_vec(&serde_json::json!({"db_info": db})).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES (91001,0,'91001','',X'{}',1,0)",hex(&job.encode(false).unwrap()))).unwrap();
    f.insert(91002, JobState::Queueing);
    let lease = Lease(AtomicBool::new(true));
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    let mut scheduler = scheduler();
    for _ in 0..4 {
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap();
    }
    assert!(f.queue(91001).is_none());
    assert!(f.queue(91002).is_none());
    let database = f.reader().get_database(91001).unwrap().unwrap();
    assert_eq!(database.State, astersql_meta_model::SchemaState::Public);
    assert_eq!(database.Name.O, "mixed_queue_schema");
    assert_eq!(database.Charset, "utf8mb4");
    assert_eq!(database.Collate, "utf8mb4_bin");
    let history = f.reader().get_history_ddl_job(91001).unwrap().unwrap();
    assert_eq!(history.state, JobState::Synced);
    assert_eq!(history.binlog_info.unwrap().db_info.unwrap().ID, 91001);
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
}

#[test]
fn crossks_align_normal_ddl_create_schema_v1_cancels_name_conflict_without_metadata_change() {
    let f = Fixture::new();
    let before = f.reader().get_schema_version_with_non_empty_diff().unwrap();
    let database = astersql_meta_model::DBInfo {
        ID: 92001,
        Name: astersql_meta_model::ast::NewCIStr("TEST"),
        Charset: "utf8mb4".into(),
        Collate: "utf8mb4_bin".into(),
        ..Default::default()
    };
    let mut job = Job::default();
    job.id = 92001;
    job.tp = 1;
    job.schema_id = 92001;
    job.schema_name = "TEST".into();
    job.state = JobState::Queueing;
    job.version = astersql_meta_model::group_3::JobVersion::V1;
    job.raw_args = serde_json::to_vec(&vec![database]).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES (92001,0,'92001','',X'{}',1,0)",hex(&job.encode(false).unwrap()))).unwrap();
    let mut session = f.pool.acquire().unwrap();
    let mut scheduler = scheduler();
    scheduler
        .schedule_persisted(
            &mut session,
            &Lease(AtomicBool::new(true)),
            &mut executor(),
            0,
        )
        .unwrap();
    assert!(f.queue(92001).is_none());
    assert!(f.reader().get_database(92001).unwrap().is_none());
    assert_eq!(
        f.reader().get_schema_version_with_non_empty_diff().unwrap(),
        before
    );
    let history = f.reader().get_history_ddl_job(92001).unwrap().unwrap();
    assert_eq!(history.state, JobState::Cancelled);
    assert!(history.error.unwrap().contains("1007"));
}

impl Fixture {
    fn insert_job(&self, job: &mut Job) {
        let wire = hex(&job.encode(false).unwrap());
        self.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES ({},0,'{}','{}',X'{}',{},0)", job.id,job.schema_id,job.table_id,wire,job.tp)).unwrap();
    }
}

#[test]
fn crossks_align_normal_ddl_modify_schema_charset_changes_metadata_and_noop_has_no_diff() {
    let f = Fixture::new();
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    for id in [93001, 93002] {
        let mut job = Job::default();
        job.id = id;
        job.tp = 26;
        job.schema_id = f.db;
        job.schema_name = "test".into();
        job.state = JobState::Queueing;
        job.version = astersql_meta_model::group_3::JobVersion::V1;
        job.raw_args = serde_json::to_vec(&vec!["latin1", "latin1_bin"]).unwrap();
        f.insert_job(&mut job);
        let before = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        for _ in 0..2 {
            scheduler
                .schedule_persisted(&mut session, &lease, &mut executor, 0)
                .unwrap();
        }
        assert!(f.queue(id).is_none());
        let database = f.reader().get_database(f.db).unwrap().unwrap();
        assert_eq!(database.Charset, "latin1");
        assert_eq!(database.Collate, "latin1_bin");
        let history = f.reader().get_history_ddl_job(id).unwrap().unwrap();
        assert_eq!(history.state, JobState::Synced);
        if id == 93002 {
            assert_eq!(history.binlog_info.unwrap().schema_version, 0);
            assert_eq!(
                f.reader().get_schema_version_with_non_empty_diff().unwrap(),
                before
            );
        } else {
            assert!(history.binlog_info.unwrap().schema_version > before);
        }
    }
}

#[test]
fn crossks_align_normal_ddl_unavailable_handler_preserves_legal_job_without_cancellation() {
    let f = Fixture::new();
    let mut job = Job::default();
    job.id = 94001;
    job.tp = 3;
    job.schema_id = f.db;
    job.table_id = 94001;
    job.schema_name = "test".into();
    job.table_name = "pending_create".into();
    job.state = JobState::Queueing;
    f.insert_job(&mut job);
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    for _ in 0..6 {
        let error = scheduler
            .schedule_persisted(
                &mut session,
                &Lease(AtomicBool::new(true)),
                &mut executor,
                0,
            )
            .unwrap_err();
        assert!(error.contains("handler unavailable"));
        let retained = f.queue(94001).unwrap();
        assert_eq!(retained.state, JobState::Queueing);
        assert_eq!(retained.error_count, 0);
    }
    assert!(f.reader().get_history_ddl_job(94001).unwrap().is_none());
}

#[test]
fn crossks_align_normal_ddl_table_mode_cancels_stale_table_name_like_go() {
    let f = Fixture::new();
    f.insert(95001, JobState::Queueing);
    let mut job = f.queue(95001).unwrap();
    job.table_name = "old_name_before_rename".into();
    let wire = hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap());
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{wire}' WHERE job_id=95001"
        ))
        .unwrap();
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    for _ in 0..2 {
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap();
    }
    let history = f.reader().get_history_ddl_job(95001).unwrap().unwrap();
    assert_eq!(history.state, JobState::Cancelled);
    assert!(history.error.unwrap().contains("1146"));
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
}

fn metadata_table_action(action: u8, multi: bool) {
    let f = Fixture::new();
    let before = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut job = Job::default();
    job.id = 97000 + i64::from(action);
    job.tp = action;
    job.schema_id = f.db;
    job.table_id = f.table;
    job.schema_name = "test".into();
    job.table_name = "normal_ddl_target".into();
    job.state = JobState::Queueing;
    job.version = astersql_meta_model::group_3::JobVersion::V1;
    job.raw_args = match action {
        17 => serde_json::to_vec(&vec!["updated persistent comment"]).unwrap(),
        39 => serde_json::to_vec(&vec![16]).unwrap(),
        _ => unreachable!(),
    };
    if multi {
        job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
            revertible: true,
            ..Default::default()
        });
    }
    f.insert_job(&mut job);
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    scheduler
        .schedule_persisted(&mut session, &lease, &mut executor, 0)
        .unwrap();
    if multi {
        assert!(
            !f.queue(job.id)
                .unwrap()
                .multi_schema_info
                .unwrap()
                .revertible
        );
        assert_eq!(
            f.reader()
                .get_table(f.db, f.table)
                .unwrap()
                .unwrap()
                .Comment,
            before.Comment
        );
    }
    for _ in 0..2 {
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap();
    }
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(table.ID, before.ID);
    assert_eq!(table.Columns.len(), before.Columns.len());
    assert_eq!(table.Mode, before.Mode);
    assert_eq!(table.Revision, before.Revision + 1);
    if action == 17 {
        assert_eq!(table.Comment, "updated persistent comment");
    }
    if action == 39 {
        assert_eq!(table.AutoIDCache, 16);
    }
    let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
    assert_eq!(history.state, JobState::Synced);
    assert_eq!(
        history.binlog_info.unwrap().table_info.unwrap().Revision,
        table.Revision
    );
}

#[test]
fn crossks_align_normal_ddl_metadata_comment_preserves_full_table_and_multi_boundary() {
    metadata_table_action(17, false);
    metadata_table_action(17, true);
}

#[test]
fn crossks_align_normal_ddl_metadata_auto_id_cache_preserves_full_table() {
    metadata_table_action(39, false);
}

#[test]
fn crossks_align_normal_ddl_schema_placement_checks_policy_and_preserves_go_noop_rules() {
    let f = Fixture::new();
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let mut encoded = vec![0];
    encoded.extend(
        serde_json::to_vec(
            &serde_json::json!({"id": 98001, "name": {"O":"placement","L":"placement"},"state":5}),
        )
        .unwrap(),
    );
    txn.Set(hash(b"Policies", b"Policy:98001"), encoded)
        .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let reference = astersql_meta_model::PolicyRefInfo {
        ID: 98001,
        Name: astersql_meta_model::ast::NewCIStr("placement"),
    };
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    for (index, policy) in [
        Some(reference.clone()),
        Some(reference),
        None,
        None,
        Some(astersql_meta_model::PolicyRefInfo {
            ID: 98099,
            Name: astersql_meta_model::ast::NewCIStr("missing"),
        }),
    ]
    .into_iter()
    .enumerate()
    {
        let mut job = Job::default();
        job.id = 98500 + index as i64;
        job.tp = 55;
        job.schema_id = f.db;
        job.schema_name = "test".into();
        job.state = JobState::Queueing;
        job.version = astersql_meta_model::group_3::JobVersion::V2;
        job.raw_args = serde_json::to_vec(&serde_json::json!({"policy_ref":policy})).unwrap();
        f.insert_job(&mut job);
        let before = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        for _ in 0..2 {
            scheduler
                .schedule_persisted(
                    &mut session,
                    &Lease(AtomicBool::new(true)),
                    &mut executor,
                    0,
                )
                .unwrap();
        }
        let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
        let database = f.reader().get_database(f.db).unwrap().unwrap();
        if index == 4 {
            assert_eq!(history.state, JobState::Cancelled);
            assert!(history.error.unwrap().contains("8239"));
            assert!(database.PlacementPolicyRef.is_none());
            assert_eq!(
                f.reader().get_schema_version_with_non_empty_diff().unwrap(),
                before
            );
        } else {
            assert_eq!(history.state, JobState::Synced);
            if index < 2 {
                assert_eq!(database.PlacementPolicyRef.unwrap().ID, 98001);
            } else {
                assert!(database.PlacementPolicyRef.is_none());
            }
            if index == 1 {
                assert_eq!(history.binlog_info.unwrap().schema_version, 0);
            } else {
                assert!(history.binlog_info.unwrap().schema_version > before);
            }
        }
    }
}

#[test]
fn crossks_align_normal_ddl_modify_schema_missing_database_uses_go_drop_exists_error() {
    let f = Fixture::new();
    let mut job = Job::default();
    job.id = 98601;
    job.tp = 26;
    job.schema_id = 98601;
    job.schema_name = "missing_database".into();
    job.state = JobState::Queueing;
    job.raw_args = serde_json::to_vec(&vec!["latin1", "latin1_bin"]).unwrap();
    f.insert_job(&mut job);
    let mut session = f.pool.acquire().unwrap();
    scheduler()
        .schedule_persisted(
            &mut session,
            &Lease(AtomicBool::new(true)),
            &mut executor(),
            0,
        )
        .unwrap();
    let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
    assert_eq!(history.state, JobState::Cancelled);
    assert!(history.error.unwrap().contains("1008"));
}

#[test]
fn crossks_align_normal_ddl_drop_foreign_key_preserves_other_keys_and_missing_error() {
    let f = Fixture::new();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.ForeignKeys = ["remove_me", "keep_me"]
        .into_iter()
        .enumerate()
        .map(|(id, name)| astersql_meta_model::FKInfo {
            ID: id as i64 + 1,
            Name: astersql_meta_model::ast::NewCIStr(name),
            State: astersql_meta_model::SchemaState::Public,
            ..Default::default()
        })
        .collect();
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    for (index, name) in ["REMOVE_ME", "missing"].into_iter().enumerate() {
        let before = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        let mut job = Job::default();
        job.id = 99001 + index as i64;
        job.tp = 10;
        job.schema_id = f.db;
        job.table_id = f.table;
        job.schema_name = "test".into();
        job.table_name = "normal_ddl_target".into();
        job.state = JobState::Queueing;
        job.version = astersql_meta_model::group_3::JobVersion::V1;
        job.raw_args = serde_json::to_vec(&vec![astersql_meta_model::ast::NewCIStr(name)]).unwrap();
        f.insert_job(&mut job);
        for _ in 0..2 {
            scheduler
                .schedule_persisted(
                    &mut session,
                    &Lease(AtomicBool::new(true)),
                    &mut executor,
                    0,
                )
                .unwrap();
        }
        let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
        let actual = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        assert_eq!(actual.ForeignKeys.len(), 1);
        assert_eq!(actual.ForeignKeys[0].Name.L, "keep_me");
        assert_eq!(actual.Columns.len(), table.Columns.len());
        if index == 0 {
            assert_eq!(history.state, JobState::Synced);
            assert_eq!(history.schema_state, astersql_meta_model::SchemaState::None);
            assert!(f.reader().get_schema_version_with_non_empty_diff().unwrap() > before);
        } else {
            assert_eq!(history.state, JobState::Cancelled);
            assert!(history.error.unwrap().contains("1091"));
            assert_eq!(
                f.reader().get_schema_version_with_non_empty_diff().unwrap(),
                before
            );
        }
    }
}

#[test]
fn crossks_align_normal_ddl_drop_foreign_key_rollback_finishes_rollback_done() {
    let f = Fixture::new();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.ForeignKeys.push(astersql_meta_model::FKInfo {
        ID: 1,
        Name: astersql_meta_model::ast::NewCIStr("rollback_fk"),
        State: astersql_meta_model::SchemaState::Public,
        ..Default::default()
    });
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let mut job = Job::default();
    job.id = 99009;
    job.tp = 10;
    job.schema_id = f.db;
    job.table_id = f.table;
    job.schema_name = "test".into();
    job.table_name = "normal_ddl_target".into();
    job.state = JobState::Rollingback;
    job.version = astersql_meta_model::group_3::JobVersion::V1;
    job.raw_args =
        serde_json::to_vec(&vec![astersql_meta_model::ast::NewCIStr("rollback_fk")]).unwrap();
    f.insert_job(&mut job);
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    for _ in 0..2 {
        scheduler
            .schedule_persisted(
                &mut session,
                &Lease(AtomicBool::new(true)),
                &mut executor,
                0,
            )
            .unwrap();
    }
    let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
    assert_eq!(history.state, JobState::RollbackDone);
    assert_eq!(history.schema_state, astersql_meta_model::SchemaState::None);
    assert!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .ForeignKeys
            .is_empty()
    );
}

#[test]
fn crossks_align_normal_ddl_keeps_multi_schema_skip_version_in_worker_transaction() {
    use astersql_ddl::job_worker::DurableJobExecutor;
    let f = Fixture::new();
    let before = version(&f);
    let before_table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut job = Job::default();
    job.id = 99101;
    job.tp = 17;
    job.schema_id = f.db;
    job.table_id = f.table;
    job.schema_name = "test".into();
    job.table_name = "normal_ddl_target".into();
    job.state = JobState::Running;
    job.version = astersql_meta_model::group_3::JobVersion::V1;
    job.raw_args = serde_json::to_vec(&vec!["batched comment"]).unwrap();
    job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
        skip_version: true,
        revertible: false,
        ..Default::default()
    });
    let mut session = f.pool.acquire().unwrap();
    astersql_ddl::job_worker::DurableJobSession::begin(&mut session).unwrap();
    let step = executor().step(&mut session, &mut job).unwrap();
    astersql_ddl::job_worker::DurableJobSession::commit(&mut session).unwrap();
    assert_eq!(step.schema_version, 0);
    assert_eq!(version(&f), before);
    assert!(job.multi_schema_info.as_ref().unwrap().skip_version);
    let actual = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(actual.Comment, "batched comment");
    assert_eq!(actual.Revision, before_table.Revision + 1);
    assert_eq!(actual.Columns.len(), before_table.Columns.len());
}

#[test]
fn crossks_align_normal_ddl_service_consumes_mixed_queue_and_stops_owner() {
    use astersql_domain::domain::{DdlService, StartMode};
    use astersql_owner::manager::Context;
    let f = Fixture::new();
    f.insert(99201, JobState::Queueing);
    let mut job = Job::default();
    job.id = 99202;
    job.tp = 1;
    job.schema_id = 99203;
    job.schema_name = "normal_service_schema".into();
    job.state = JobState::Queueing;
    job.version = astersql_meta_model::group_3::JobVersion::V1;
    job.raw_args = serde_json::to_vec(&vec![astersql_meta_model::DBInfo {
        ID: job.schema_id,
        Name: astersql_meta_model::ast::NewCIStr("normal_service_schema"),
        ..Default::default()
    }])
    .unwrap();
    f.insert_job(&mut job);
    let cancellation = Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancellation.clone(),
        "normal-service",
        None,
        format!("/normal-service/{}", f.db),
    );
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let service = Arc::new(super::normal_ddl_service::NormalDdlService::new(
        owner.clone(),
        runtime,
        cancellation,
        f.pool.clone(),
        Arc::new(super::normal_ddl_service::DomainSchemaLoader(
            Arc::downgrade(&f.domain),
        )),
        Arc::new(|| Ok(Box::new(executor()))),
        Arc::new(|_| Err("test does not submit table mode through string interface".into())),
        Arc::new(|| {}),
        true,
    ));
    f.domain.set_ddl(service.clone());
    service.start(StartMode::Normal).unwrap();
    service.start(StartMode::Normal).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if f.reader().get_history_ddl_job(99201).unwrap().is_some()
            && f.reader().get_history_ddl_job(99202).unwrap().is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "normal DDL service did not consume mixed queue: {:?}",
            service.last_error()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
    assert!(f.reader().get_database(99203).unwrap().is_some());
    assert_eq!(service.owner_id(), Some("normal-service".into()));
    service.stop().unwrap();
    service.stop().unwrap();
    assert!(!owner.IsOwner());
    assert!(
        service
            .start(StartMode::Normal)
            .unwrap_err()
            .contains("closed")
    );
    assert!(f.pool.acquire().is_err());
    f.domain.close();
}

#[test]
fn crossks_align_normal_ddl_service_domain_close_releases_owned_service_and_pool() {
    let f = Fixture::new();
    let weak = Arc::downgrade(&f.domain);
    let cancellation = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancellation.clone(),
        "normal-close",
        None,
        format!("/normal-close/{}", f.db),
    );
    let service = Arc::new(super::normal_ddl_service::NormalDdlService::new(
        owner,
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        cancellation,
        f.pool.clone(),
        Arc::new(super::normal_ddl_service::DomainSchemaLoader(
            Arc::downgrade(&f.domain),
        )),
        Arc::new(|| Ok(Box::new(executor()))),
        Arc::new(|_| Ok(())),
        Arc::new(|| {}),
        false,
    ));
    f.domain.set_ddl(service.clone());
    f.domain.close();
    drop(service);
    drop(f);
    assert!(
        weak.upgrade().is_none(),
        "closed Domain retains its DDL service/pool cycle"
    );
}

#[test]
fn crossks_align_normal_ddl_table_mode_uses_shared_multi_schema_version() {
    use astersql_ddl::job_worker::{DurableJobExecutor, DurableJobSession};
    let f = Fixture::new();
    f.insert(99301, JobState::Running);
    let mut job = f.queue(99301).unwrap();
    job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
        skip_version: true,
        ..Default::default()
    });
    let before = version(&f);
    let mut session = f.pool.acquire().unwrap();
    session.begin().unwrap();
    let result = executor().step(&mut session, &mut job).unwrap();
    session.commit().unwrap();
    assert_eq!(result.schema_version, 0);
    assert_eq!(version(&f), before);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
    assert_eq!(job.binlog_info.unwrap().schema_version, 0);
}

#[test]
fn crossks_align_normal_ddl_service_public_owner_handoff_consumes_same_queue() {
    use astersql_domain::domain::{DdlService, StartMode};
    use astersql_owner::manager::Context;
    let f = Fixture::new();
    let key = format!("/normal-handoff/{}", f.db);
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let pool2 = SystemSessionPool::new(f.domain.clone());
    let mut owners = Vec::new();
    let mut services = Vec::new();
    for (id, pool) in [("first", f.pool.clone()), ("second", pool2.clone())] {
        let cancellation = Context::new();
        let owner =
            astersql_owner::mock::NewMockManager(cancellation.clone(), id, None, key.clone());
        let service = Arc::new(super::normal_ddl_service::NormalDdlService::new(
            owner.clone(),
            runtime.clone(),
            cancellation,
            pool,
            Arc::new(super::normal_ddl_service::DomainSchemaLoader(
                Arc::downgrade(&f.domain),
            )),
            Arc::new(|| Ok(Box::new(executor()))),
            Arc::new(|_| Err("unused submission adapter".into())),
            Arc::new(|| {}),
            true,
        ));
        service.start(StartMode::Normal).unwrap();
        owners.push(owner);
        services.push(service);
    }
    assert!(owners[0].IsOwner());
    assert!(!owners[1].IsOwner());
    f.insert(99401, JobState::Queueing);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while f.reader().get_history_ddl_job(99401).unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "first owner did not consume persisted job"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    services[0].stop().unwrap();
    let second = Fixture {
        domain: f.domain.clone(),
        pool: pool2,
        db: f.db,
        table: f.table,
    };
    second.insert(99402, JobState::Queueing);
    while second
        .reader()
        .get_history_ddl_job(99402)
        .unwrap()
        .is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "second owner did not take over queue: {:?}",
            services[1].last_error()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(owners[1].IsOwner());
    assert!(!owners[0].IsOwner());
    assert_eq!(
        second
            .reader()
            .get_history_ddl_job(99402)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    services[1].stop().unwrap();
    f.domain.close();
}

#[test]
fn crossks_align_normal_ddl_refresh_meta_only_updates_schema_and_diff() {
    let f = Fixture::new();
    let original = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut scheduler = scheduler();
    let mut executor = executor();
    let mut session = f.pool.acquire().unwrap();
    for (index, job_version) in [
        astersql_meta_model::group_3::JobVersion::V1,
        astersql_meta_model::group_3::JobVersion::V2,
    ]
    .into_iter()
    .enumerate()
    {
        let before = version(&f);
        let mut job = Job::default();
        job.id = 99501 + index as i64;
        job.tp = 76;
        job.schema_id = f.db;
        job.table_id = f.table;
        job.schema_name = "test".into();
        job.table_name = "normal_ddl_target".into();
        job.state = JobState::Queueing;
        job.version = job_version;
        let args = serde_json::json!({"schema_id":f.db,"table_id":f.table,"involved_db":"test","involved_table":"normal_ddl_target"});
        job.raw_args = serde_json::to_vec(&if index == 0 {
            serde_json::json!([args])
        } else {
            args
        })
        .unwrap();
        f.insert_job(&mut job);
        for _ in 0..2 {
            scheduler
                .schedule_persisted(
                    &mut session,
                    &Lease(AtomicBool::new(true)),
                    &mut executor,
                    0,
                )
                .unwrap();
        }
        let history = f.reader().get_history_ddl_job(job.id).unwrap().unwrap();
        assert_eq!(history.state, JobState::Synced);
        assert_eq!(
            history.schema_state,
            astersql_meta_model::SchemaState::Public
        );
        assert!(version(&f) > before);
        assert_eq!(
            astersql_meta_model::EncodeTableInfo(
                &f.reader().get_table(f.db, f.table).unwrap().unwrap()
            )
            .unwrap(),
            astersql_meta_model::EncodeTableInfo(&original).unwrap()
        );
        let snapshot = f
            .domain
            .storage_handle()
            .with_storage(|s| {
                let v = s.CurrentVersion("global")?;
                Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
            })
            .unwrap();
        let diff = snapshot
            .Get(
                &astersql_kv::Context::default(),
                astersql_meta::transaction_meta_string_key(
                    format!("Diff:{}", version(&f)).as_bytes(),
                ),
                &[],
            )
            .unwrap();
        let diff: serde_json::Value = serde_json::from_slice(&diff.Value).unwrap();
        assert_eq!(diff["type"], 76);
        assert_eq!(diff["table_id"], f.table);
    }
}

fn normal_upgrade_policy() -> (
    Arc<astersql_ddl_serverstate::EtcdSyncer>,
    astersql_ddl::normal_policy::NormalDdlJobPolicy,
) {
    let state = Arc::new(astersql_ddl_serverstate::EtcdSyncer::new(
        Arc::new(astersql_ddl_serverstate::StateStore::default()),
        "/normal-policy-state",
    ));
    let context = astersql_ddl_serverstate::SyncContext::new();
    astersql_ddl_serverstate::Syncer::init(state.as_ref(), &context).unwrap();
    let policy = astersql_ddl::normal_policy::NormalDdlJobPolicy {
        state: state.clone(),
        context,
        owner_id: "normal-policy-owner".into(),
    };
    (state, policy)
}
#[test]
fn crossks_align_normal_ddl_policy_upgrade_pauses_then_resumes_durable_job() {
    use astersql_ddl_serverstate::Syncer;
    let f = Fixture::new();
    f.insert(99601, JobState::Queueing);
    let before = version(&f);
    let (state, policy) = normal_upgrade_policy();
    state
        .update_global_state(
            &policy.context,
            astersql_ddl_serverstate::StateInfo {
                state: astersql_ddl_serverstate::STATE_UPGRADING.into(),
            },
        )
        .unwrap();
    let mut executor = NormalDdlExecutor {
        barrier: Barrier {
            fail: false,
            seen: vec![],
        },
        policy,
        sequence: Arc::new(AtomicI64::new(0)),
    };
    let mut scheduler = scheduler();
    let mut session = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    assert_eq!(
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap(),
        0
    );
    assert_eq!(f.queue(99601).unwrap().state, JobState::Pausing);
    assert_eq!(
        f.queue(99601).unwrap().admin_operator,
        astersql_meta_model::group_3::AdminCommandOperator::System
    );
    assert_eq!(
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap(),
        1
    );
    assert_eq!(f.queue(99601).unwrap().state, JobState::Paused);
    assert_eq!(
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap(),
        0
    );
    assert_eq!(version(&f), before);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
    state
        .update_global_state(
            &executor.policy.context,
            astersql_ddl_serverstate::StateInfo {
                state: astersql_ddl_serverstate::STATE_NORMAL_RUNNING.into(),
            },
        )
        .unwrap();
    assert!(
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap_err()
            .contains("need to be resumed")
    );
    let resumed = f.queue(99601).unwrap();
    assert_eq!(resumed.state, JobState::Queueing);
    assert!(resumed.error.is_none());
    assert!(resumed.pause_reason.is_none());
    for _ in 0..2 {
        scheduler
            .schedule_persisted(&mut session, &lease, &mut executor, 0)
            .unwrap();
    }
    assert_eq!(
        f.reader()
            .get_history_ddl_job(99601)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
    drop(state);
}
#[test]
fn crossks_align_normal_ddl_policy_retains_user_and_disk_full_pauses() {
    let f = Fixture::new();
    let (state, mut policy) = normal_upgrade_policy();
    let mut session = f.pool.acquire().unwrap();
    for (index, system) in [false, true].into_iter().enumerate() {
        let id = 99611 + index as i64;
        f.insert(id, JobState::Paused);
        let mut job = f.queue(id).unwrap();
        job.admin_operator = if system {
            astersql_meta_model::group_3::AdminCommandOperator::System
        } else {
            astersql_meta_model::group_3::AdminCommandOperator::EndUser
        };
        if system {
            job.set_pause_reason(
                astersql_meta_model::group_3::JOB_PAUSE_REASON_KV_DISK_FULL.into(),
                "insufficient TiKV space".into(),
            );
        }
        let bytes = astersql_meta::encode_go_ddl_job(&mut job, false).unwrap();
        session
            .query(format!(
                "update mysql.tidb_ddl_job set job_meta=X'{}' where job_id={id}",
                hex(&bytes)
            ))
            .unwrap();
        assert!(!policy.runnable(&mut session, &job).unwrap());
        let actual = f.queue(id).unwrap();
        assert_eq!(actual.state, JobState::Paused);
        assert_eq!(actual.admin_operator, job.admin_operator);
        assert_eq!(
            actual
                .pause_reason
                .as_ref()
                .map(|reason| &reason.reason_type),
            job.pause_reason.as_ref().map(|reason| &reason.reason_type)
        );
    }
    drop(state);
}

struct ConflictOnPolicyCommit<'a> {
    session: &'a mut super::system_session::SystemSessionLease,
    peer: Arc<SystemSessionPool>,
    replacement: Option<(i64, Vec<u8>)>,
}
impl astersql_ddl::job_worker::DurableJobSession for ConflictOnPolicyCommit<'_> {
    fn begin(&mut self) -> Result<(), String> {
        astersql_ddl::job_worker::DurableJobSession::begin(self.session)
    }
    fn commit(&mut self) -> Result<(), String> {
        if let Some((id, bytes)) = self.replacement.take() {
            self.peer.acquire()?.query(format!(
                "update mysql.tidb_ddl_job set job_meta=X'{}' where job_id={id}",
                hex(&bytes)
            ))?;
        }
        astersql_ddl::job_worker::DurableJobSession::commit(self.session)
    }
    fn rollback(&mut self) {
        astersql_ddl::job_worker::DurableJobSession::rollback(self.session)
    }
    fn query(&mut self, sql: &str, purpose: &str) -> Result<Vec<Vec<String>>, String> {
        astersql_ddl::job_worker::DurableJobSession::query(self.session, sql, purpose)
    }
    fn with_transaction(
        &mut self,
        operation: astersql_ddl::job_worker::TransactionOperation,
    ) -> Result<Vec<u8>, String> {
        astersql_ddl::job_worker::DurableJobSession::with_transaction(self.session, operation)
    }
}
#[test]
fn crossks_align_normal_ddl_policy_commit_conflict_preserves_concurrent_user_pause() {
    use astersql_ddl_serverstate::Syncer;
    let f = Fixture::new();
    f.insert(99621, JobState::Queueing);
    let before = version(&f);
    let job = f.queue(99621).unwrap();
    let mut user_pause = f.queue(99621).unwrap();
    user_pause.state = JobState::Paused;
    user_pause.admin_operator = astersql_meta_model::group_3::AdminCommandOperator::EndUser;
    let bytes = astersql_meta::encode_go_ddl_job(&mut user_pause, false).unwrap();
    let (state, mut policy) = normal_upgrade_policy();
    state
        .update_global_state(
            &policy.context,
            astersql_ddl_serverstate::StateInfo {
                state: astersql_ddl_serverstate::STATE_UPGRADING.into(),
            },
        )
        .unwrap();
    let mut session = f.pool.acquire().unwrap();
    let mut conflict = ConflictOnPolicyCommit {
        session: &mut session,
        peer: f.pool.clone(),
        replacement: Some((job.id, bytes)),
    };
    assert!(!policy.runnable(&mut conflict, &job).unwrap());
    let actual = f.queue(job.id).unwrap();
    assert_eq!(actual.state, JobState::Paused);
    assert_eq!(
        actual.admin_operator,
        astersql_meta_model::group_3::AdminCommandOperator::EndUser
    );
    assert_eq!(version(&f), before);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
}

// The connection-manager boundary uses the same TransactionMDL as real SQL
// sessions; the adapter must retain unrelated jobs and never clear held locks.
struct NormalConnectionCoordinator {
    mdl: Arc<astersql_session_sessmgr::TransactionMDL>,
    kills: std::sync::atomic::AtomicUsize,
}
impl astersql_session_sessmgr::InfoSchemaCoordinator for NormalConnectionCoordinator {
    fn StoreInternalSession(&self, _: astersql_session_sessmgr::InternalSession) {
        unreachable!("internal SQL uses the shared pool registry")
    }
    fn DeleteInternalSession(&self, _: &astersql_session_sessmgr::InternalSession) {
        unreachable!("internal SQL uses the shared pool registry")
    }
    fn ContainsInternalSession(&self, _: &astersql_session_sessmgr::InternalSession) -> bool {
        false
    }
    fn InternalSessionCount(&self) -> isize {
        0
    }
    fn CheckOldRunningTxn(
        &self,
        jobs: &mut std::collections::HashMap<i64, Arc<astersql_session_sessmgr::mdldef::JobMDL>>,
    ) {
        self.mdl.check_jobs(jobs);
    }
    fn KillNonFlashbackClusterConn(&self) {
        self.kills.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn crossks_align_normal_ddl_coordinator_fences_user_and_internal_transactions() {
    use astersql_infoschema_issyncer::InfoSchemaCoordinator;
    let f = Fixture::new();
    let manager = Arc::new(NormalConnectionCoordinator {
        mdl: Arc::new(astersql_session_sessmgr::TransactionMDL::default()),
        kills: std::sync::atomic::AtomicUsize::new(0),
    });
    manager.mdl.finish_table(101, 9);
    let erased: Arc<dyn astersql_session_sessmgr::InfoSchemaCoordinator> = manager.clone();
    f.domain.set_schema_coordinator(Arc::downgrade(&erased));
    let internal = Arc::new(astersql_domain_crossks::new_schema_coordinator());
    let internal_mdl = Arc::new(astersql_session_sessmgr::TransactionMDL::default());
    internal_mdl.finish_table(102, 9);
    internal.store_internal_session(Arc::new(astersql_domain_crossks::RegisteredMDLSession {
        id: 7,
        mdl: internal_mdl.clone(),
    }));
    let coordinator = super::normal_ddl_service::NormalSchemaCoordinator {
        domain: Arc::downgrade(&f.domain),
        internal: internal.clone(),
    };
    let make_jobs = || {
        [101, 102, 103]
            .into_iter()
            .map(|table| {
                (
                    table,
                    astersql_infoschema_issyncer::JobMDL {
                        Ver: 10,
                        TableIDs: [table].into_iter().collect(),
                    },
                )
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let mut jobs = make_jobs();
    coordinator.CheckOldRunningTxn(&mut jobs);
    assert_eq!(jobs.keys().copied().collect::<Vec<_>>(), vec![103]);
    coordinator.KillNonFlashbackClusterConn();
    assert_eq!(manager.kills.load(Ordering::SeqCst), 1);
    // Rechecking still blocks both transactions: checking did not release MDL.
    let mut jobs = make_jobs();
    coordinator.CheckOldRunningTxn(&mut jobs);
    assert_eq!(jobs.len(), 1);
    manager.mdl.clear();
    internal.delete_internal_session(7);
    let mut jobs = make_jobs();
    coordinator.CheckOldRunningTxn(&mut jobs);
    assert_eq!(jobs.len(), 3);
    drop(erased);
    drop(manager);
    assert!(
        f.domain.schema_coordinator().is_none(),
        "Domain must not retain Server"
    );
}

#[test]
fn normal_ddl_plan_user_mdl_real_internal_pool_preserves_go_restricted_bypass() {
    use super::system_session::{SystemSessionCallbacks, transaction_mdl};
    use astersql_infoschema_issyncer::InfoSchemaCoordinator;
    struct RestoreMdl(bool);
    impl Drop for RestoreMdl {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _restore = RestoreMdl(astersql_sessionctx_vardef::IsMDLEnabled());
    astersql_sessionctx_vardef::SetEnableMDL(true);
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1, 'internal-held')")
        .unwrap();
    let internal = Arc::new(astersql_domain_crossks::new_schema_coordinator());
    let borrowed = internal.clone();
    let returned = internal.clone();
    let destroyed = internal.clone();
    let pool = SystemSessionPool::new_with_callbacks(
        f.domain.clone(),
        SystemSessionCallbacks {
            borrowed: Arc::new(move |session| {
                borrowed.store_internal_session(Arc::new(
                    astersql_domain_crossks::RegisteredMDLSession {
                        id: session.session_id(),
                        mdl: transaction_mdl(session.as_ref()).unwrap(),
                    },
                ));
            }),
            returned: Arc::new(move |id| returned.delete_internal_session(id)),
            destroyed: Arc::new(move |id| destroyed.delete_internal_session(id)),
        },
    );
    let lease = pool.acquire().unwrap();
    let coordinator = super::normal_ddl_service::NormalSchemaCoordinator {
        domain: Arc::downgrade(&f.domain),
        internal: internal.clone(),
    };
    lease.query("BEGIN").unwrap();
    assert_eq!(
        lease.query("SELECT * FROM test.normal_ddl_target").unwrap(),
        vec![vec!["1".to_owned(), "internal-held".to_owned()]]
    );
    assert!(internal.contains_internal_session(lease.session_id()));
    let mut jobs = std::collections::HashMap::from([(
        77,
        astersql_infoschema_issyncer::JobMDL {
            Ver: f.domain.info_schema().SchemaMetaVersion() + 1,
            TableIDs: [f.table].into_iter().collect(),
        },
    )]);
    coordinator.CheckOldRunningTxn(&mut jobs);
    assert!(
        jobs.contains_key(&77),
        "Go RemoveLockDDLJobs skips the real pool's InRestrictedSQL sessions"
    );
    let id = lease.session_id();
    lease.query("COMMIT").unwrap();
    drop(lease);
    assert_eq!(internal.internal_session_count(), 0);
    let lease = pool.acquire().unwrap();
    assert_eq!(lease.session_id(), id, "a clean borrowed session is reused");
    lease
        .query("BEGIN; INSERT INTO test.normal_ddl_target VALUES (2, 'rollback')")
        .unwrap();
    drop(lease);
    assert_eq!(internal.internal_session_count(), 0);
    let lease = pool.acquire().unwrap();
    assert_eq!(
        lease.query("SELECT * FROM test.normal_ddl_target").unwrap(),
        vec![vec!["1".to_owned(), "internal-held".to_owned()]]
    );
    let id = lease.session_id();
    assert!(internal.contains_internal_session(id));
    pool.close();
    pool.close();
    assert_eq!(
        internal.internal_session_count(),
        0,
        "closing a pool destroys and unregisters outstanding leases"
    );
    assert!(lease.query("SELECT * FROM test.normal_ddl_target").is_err());
    drop(lease);
    assert!(!internal.contains_internal_session(id));
    assert!(pool.acquire().is_err());
}

struct ContextExecutor {
    pool: Arc<SystemSessionPool>,
    conflict: bool,
    fail: bool,
    cancel: Option<Arc<AtomicBool>>,
}
impl DurableJobExecutor for ContextExecutor {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &Job) -> Result<bool, String> {
        Ok(true)
    }
    fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
    fn wait_synced(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
    fn step(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &mut Job,
    ) -> Result<astersql_ddl::job_worker::DurableJobStep, String> {
        let mut current = Job::decode(&job.encode(false).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let pool = self.pool.clone();
        let conflict = self.conflict;
        let fail = self.fail;
        let cancel = self.cancel.clone();
        let bytes = session.with_execution_context(Box::new(move |context| {
            current.state = JobState::Running;
            let version = astersql_ddl::persistent_actions::step(context, &mut current)?;
            context.query(&format!("INSERT INTO mysql.tidb_mdl_info (job_id, version, table_ids) VALUES ({}, {version}, '{}')", current.id, current.table_id), "context-dual-write")?;
            // A different real SQL session cannot see either uncommitted write.
            assert!(pool.acquire()?.query(format!("SELECT job_id FROM mysql.tidb_mdl_info WHERE job_id={}", current.id))?.is_empty());
            if conflict {
                let mut other = pool.acquire()?;
                other.begin()?;
                let db = current.schema_id;
                let table = current.table_id;
                other.with_transaction(Box::new(move |txn| {
                    let mut meta = astersql_meta::TransactionMutator::new(txn);
                    let mut info = meta.get_table(db, table)?.unwrap();
                    info.Comment = "concurrent metadata".into();
                    meta.update_table(db, &mut info)?;
                    Ok(Vec::new())
                }))?;
                other.commit()?;
            }
            if let Some(owner) = cancel { owner.store(false, Ordering::Release); }
            if fail { return Err("handler SQL failure".into()); }
            current.encode(false).map_err(|e| e.to_string())
        }))?;
        *job = Job::decode(&bytes).map_err(|e| e.to_string())?;
        Ok(astersql_ddl::job_worker::DurableJobStep {
            schema_version: job.last_schema_version,
            update_raw_args: false,
            removed: false,
        })
    }
}
#[test]
fn normal_ddl_plan_transaction_context_atomic_commit_rollback_and_retry() {
    for failure in 0..4 {
        let f = Fixture::new();
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES (1, 'existing row')")
            .unwrap();
        let id = 84001 + failure;
        f.insert(id, JobState::Queueing);
        let original = f.queue(id).unwrap().encode(false).unwrap();
        let before = version(&f);
        let owner = Arc::new(AtomicBool::new(true));
        struct Owner(Arc<AtomicBool>);
        impl JobLease for Owner {
            fn is_owner(&self) -> bool {
                self.0.load(Ordering::Acquire)
            }
            fn is_cancelled(&self) -> bool {
                !self.is_owner()
            }
        }
        let lease = Owner(owner.clone());
        let mut executor = ContextExecutor {
            pool: f.pool.clone(),
            conflict: failure == 1,
            fail: failure == 2,
            cancel: (failure == 3).then_some(owner.clone()),
        };
        let mut session = f.pool.acquire().unwrap();
        let result = scheduler().schedule_persisted(&mut session, &lease, &mut executor, 0);
        if failure == 0 {
            assert_eq!(result.unwrap(), 1);
        } else {
            let error = result.unwrap_err();
            assert!(
                if failure == 1 {
                    error.contains("Write conflict")
                        || error.contains("write conflict")
                        || error.contains(astersql_kv::TxnRetryableMark)
                } else if failure == 2 {
                    error.contains("handler SQL failure")
                } else {
                    error.contains("owner") || error.contains("cancelled")
                },
                "{error}"
            );
            assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
            assert_eq!(version(&f), before);
            assert_eq!(f.queue(id).unwrap().encode(false).unwrap(), original);
            assert!(
                f.pool
                    .acquire()
                    .unwrap()
                    .query(format!(
                        "SELECT job_id FROM mysql.tidb_mdl_info WHERE job_id={id}"
                    ))
                    .unwrap()
                    .is_empty()
            );
            assert!(f.reader().get_history_ddl_job(id).unwrap().is_none());
            // Retry re-reads the durable queue through the same worker session.
            owner.store(true, Ordering::Release);
            executor.conflict = false;
            executor.fail = false;
            executor.cancel = None;
            assert_eq!(
                scheduler()
                    .schedule_persisted(&mut session, &lease, &mut executor, 0)
                    .unwrap(),
                1
            );
        }
        assert_eq!(f.queue(id).unwrap().state, JobState::Done);
        assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
        assert_eq!(version(&f), before + 1);
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query(format!(
                    "SELECT table_ids FROM mysql.tidb_mdl_info WHERE job_id={id}"
                ))
                .unwrap(),
            vec![vec![f.table.to_string()]]
        );
        if failure == 1 {
            assert_eq!(
                f.reader()
                    .get_table(f.db, f.table)
                    .unwrap()
                    .unwrap()
                    .Comment,
                "concurrent metadata"
            );
        }
    }
}

#[test]
fn normal_ddl_plan_transaction_context_statement_cleanup_and_boundaries() {
    let f = Fixture::new();
    f.insert(84010, JobState::Queueing);
    let mut job = f.queue(84010).unwrap();
    let before = version(&f);
    let mut session = f.pool.acquire().unwrap();
    assert!(
        session
            .with_execution_context(Box::new(|_| Ok(Vec::new())))
            .unwrap_err()
            .contains("active transaction required")
    );
    session.begin().unwrap();
    session
        .with_execution_context(Box::new(move |context| {
            for sql in [
                "COMMIT",
                "ROLLBACK",
                "BEGIN",
                "SET autocommit=1",
                "CREATE TABLE test.context_escape (id int)",
                "SELECT 1; COMMIT",
            ] {
                assert!(
                    context
                        .query(sql, "invalid-boundary")
                        .unwrap_err()
                        .contains("transactional DML")
                );
            }
            let mut stage = None;
            context.with_transaction(&mut |txn| {
                stage = Some(txn.StageStatement().map_err(|e| e.to_string())?);
                Ok(Vec::new())
            })?;
            job.state = JobState::Running;
            astersql_ddl::persistent_actions::step(context, &mut job)?;
            let sql =
                "INSERT INTO mysql.tidb_mdl_info (job_id, version, table_ids) VALUES (84010,1,'1')";
            context.query(sql, "staged-write")?;
            assert!(
                context
                    .query(sql, "duplicate-write")
                    .unwrap_err()
                    .contains("Duplicate")
            );
            context.with_transaction(&mut |txn| {
                txn.CleanupStatement(stage.unwrap())
                    .map_err(|e| e.to_string())?;
                Ok(Vec::new())
            })?;
            Ok(Vec::new())
        }))
        .unwrap();
    // Commit after discarding the failed action, as normal DDL persists job errors.
    session.commit().unwrap();
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeNormal);
    assert_eq!(version(&f), before);
    assert_eq!(f.queue(84010).unwrap().state, JobState::Queueing);
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT job_id FROM mysql.tidb_mdl_info WHERE job_id=84010")
            .unwrap()
            .is_empty()
    );
}

fn notifier_fixture() -> (Fixture, Job, serde_json::Value) {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1, 'existing row')")
        .unwrap();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo {
        BaseTableIDs: vec![1001, 1002],
        SQLContent: "select id, payload from test.base".into(),
        AlertWarningSec: 8,
        AlertOverdueSec: 9,
        ..Default::default()
    });
    let old = serde_json::to_value(&table).unwrap();
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let mut job = Job::default();
    job.id = 85001;
    job.tp = astersql_meta_model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES;
    job.schema_id = f.db;
    job.table_id = f.table;
    job.schema_name = "test".into();
    job.table_name = "normal_ddl_target".into();
    job.state = JobState::Queueing;
    job.version = astersql_meta_model::group_3::JobVersion::V2;
    job.raw_args=serde_json::to_vec(&serde_json::json!({"alert_warning_sec":60,"alert_overdue_sec":120,"alert_refresh_failed":true})).unwrap();
    f.insert_job(&mut job);
    (f, job, old)
}

#[test]
fn normal_ddl_plan_notifier_persistent_worker_publishes_full_event_once() {
    let (f, _, old) = notifier_fixture();
    let mut session = f.pool.acquire().unwrap();
    let mut ex = executor();
    let lease = Lease(AtomicBool::new(true));
    let mut sched = scheduler();
    // The event and Done job commit before the external schema barrier fails.
    // Recreate the owner executor/scheduler and recover the persisted job.
    ex.barrier.fail = true;
    let barrier_error = sched
        .schedule_persisted(&mut session, &lease, &mut ex, 0)
        .unwrap_err();
    assert!(
        barrier_error.contains("schema unavailable"),
        "{barrier_error}"
    );
    assert_eq!(f.queue(85001).unwrap().state, JobState::Done);
    assert!(f.reader().get_history_ddl_job(85001).unwrap().is_none());
    drop(session);
    let mut session = f.pool.acquire().unwrap();
    let mut sched = scheduler();
    let mut ex = executor();
    for _ in 0..3 {
        sched
            .schedule_persisted(&mut session, &lease, &mut ex, 0)
            .unwrap();
    }
    let rows=f.pool.acquire().unwrap().query("SELECT ddl_job_id, sub_job_id, schema_change, processed_by_flag FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001").unwrap();
    assert_eq!(
        rows.len(),
        1,
        "normal persistent worker must publish exactly one event"
    );
    assert_eq!(
        (&rows[0][0], &rows[0][1], &rows[0][3]),
        (&"85001".into(), &"-1".into(), &"0".into())
    );
    let event: serde_json::Value = serde_json::from_str(&rows[0][2]).unwrap();
    assert_eq!(event["type"], 91);
    assert_eq!(event["old_table_info"], old);
    let after = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(event["table_info"], serde_json::to_value(&after).unwrap());
    assert_eq!(after.MaterializedView.as_ref().unwrap().AlertWarningSec, 60);
    assert_eq!(
        after.MaterializedView.as_ref().unwrap().BaseTableIDs,
        vec![1001, 1002]
    );
    assert_eq!(
        f.reader()
            .get_history_ddl_job(85001)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=1")
            .unwrap(),
        vec![vec!["existing row".to_string()]]
    );
}

#[test]
fn normal_ddl_plan_notifier_metadata_conflict_rolls_back_then_owner_retries() {
    let (f, mut job, old) = notifier_fixture();
    let before = version(&f);
    let original = f.queue(job.id).unwrap().encode(false).unwrap();
    let mut session = f.pool.acquire().unwrap();
    session.begin().unwrap();
    executor().step(&mut session, &mut job).unwrap();
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001")
            .unwrap()
            .is_empty()
    );
    let db = f.db;
    let table = f.table;
    let mut competing = f.pool.acquire().unwrap();
    competing.begin().unwrap();
    competing
        .with_transaction(Box::new(move |txn| {
            let mut m = astersql_meta::TransactionMutator::new(txn);
            let mut info = m.get_table(db, table)?.unwrap();
            info.Comment = "competing owner".into();
            m.update_table(db, &mut info)?;
            Ok(Vec::new())
        }))
        .unwrap();
    competing.commit().unwrap();
    let error = session.commit().unwrap_err();
    assert!(
        error.contains(astersql_kv::TxnRetryableMark)
            || error.to_lowercase().contains("write conflict"),
        "{error}"
    );
    session.rollback();
    assert_eq!(version(&f), before);
    assert_eq!(f.queue(85001).unwrap().encode(false).unwrap(), original);
    assert!(f.reader().get_history_ddl_job(85001).unwrap().is_none());
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.reader()
            .get_table(db, table)
            .unwrap()
            .unwrap()
            .MaterializedView
            .as_ref()
            .unwrap()
            .AlertWarningSec,
        8
    );
    let mut ex = executor();
    let lease = Lease(AtomicBool::new(true));
    let mut sched = scheduler();
    for _ in 0..3 {
        sched
            .schedule_persisted(&mut session, &lease, &mut ex, 0)
            .unwrap();
    }
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&rows[0][0]).unwrap();
    assert_eq!(event["old_table_info"]["comment"], "competing owner");
    assert_eq!(
        event["old_table_info"]["materialized_view"],
        old["materialized_view"]
    );
    assert_eq!(
        f.reader()
            .get_history_ddl_job(85001)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
}

#[test]
fn normal_ddl_plan_notifier_duplicate_commit_error_rolls_back_metadata() {
    let (f, mut job, _) = notifier_fixture();
    let mut session = f.pool.acquire().unwrap();
    session.query("INSERT INTO mysql.tidb_ddl_notifier (ddl_job_id,sub_job_id,schema_change,processed_by_flag) VALUES (85001,-1,'{\"type\":91}',7)").unwrap();
    let before = version(&f);
    session.begin().unwrap();
    executor().step(&mut session, &mut job).unwrap();
    // Optimistic SQL defers the duplicate constraint to the shared commit.
    let error = session.commit().unwrap_err();
    assert!(
        error.contains("1062") || error.contains("Duplicate entry"),
        "{error}"
    );
    session.rollback();
    assert_eq!(version(&f), before);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedView
            .as_ref()
            .unwrap()
            .AlertWarningSec,
        8
    );
    let rows=session.query("SELECT schema_change,processed_by_flag FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001").unwrap();
    assert_eq!(
        rows,
        vec![vec!["{\"type\":91}".to_string(), "7".to_string()]]
    );
}

#[test]
fn normal_ddl_plan_notifier_v1_multi_boundary_keys_and_validation() {
    for case in 0..9 {
        let (f, mut job, _) = notifier_fixture();
        job.version = astersql_meta_model::group_3::JobVersion::V1;
        job.raw_args = b"[60,120]".to_vec(); // Go legacy two-field decoding keeps AlertRefreshFailed=false.
        if case == 0 {
            job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
                revertible: true,
                seq: 4,
                ..Default::default()
            });
        }
        if case == 1 {
            job.raw_args = b"[60,120,true]".to_vec();
            job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
                revertible: false,
                seq: 5,
                skip_version: true,
                ..Default::default()
            });
        }
        if case == 7 {
            job.schema_name = "mysql".into();
        }
        if case == 8 {
            job.schema_id = 99999999;
        }
        if case == 2 {
            job.raw_args = br#"["invalid"]"#.to_vec();
        }
        if case == 3 {
            job.table_name = "stale_name".into();
        }
        if case == 4 {
            job.table_id = 99999999;
        }
        if case == 5 || case == 6 {
            let mut info = f.reader().get_table(f.db, f.table).unwrap().unwrap();
            if case == 5 {
                info.MaterializedView = None;
            } else {
                info.State = astersql_meta_model::SchemaState::WriteOnly;
            }
            let mut txn = f
                .domain
                .storage_handle()
                .with_storage(|s| s.Begin(&[]))
                .unwrap();
            txn.Set(
                hash(
                    format!("DB:{}", f.db).as_bytes(),
                    format!("Table:{}", f.table).as_bytes(),
                ),
                astersql_meta_model::EncodeTableInfo(&info).unwrap(),
            )
            .unwrap();
            txn.Commit(&astersql_kv::Context::default()).unwrap();
        }
        let before = version(&f);
        let mut session = f.pool.acquire().unwrap();
        session.begin().unwrap();
        let mut ex = executor();
        ex.step(&mut session, &mut job).unwrap();
        if case == 0 {
            assert!(!job.multi_schema_info.as_ref().unwrap().revertible);
            assert_eq!(job.last_schema_version, 0);
            assert!(
                session
                    .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001")
                    .unwrap()
                    .is_empty()
            );
            ex.step(&mut session, &mut job).unwrap();
            job.state = JobState::Running;
            job.multi_schema_info.as_mut().unwrap().seq = 6;
            job.raw_args = b"[61,121,true]".to_vec();
            ex.step(&mut session, &mut job).unwrap();
        }
        session.commit().unwrap();
        let rows=session.query("SELECT sub_job_id,schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=85001 ORDER BY sub_job_id").unwrap();
        if case < 2 {
            assert_eq!(rows.len(), if case == 0 { 2 } else { 1 });
            assert_eq!(rows[0][0], if case == 0 { "4" } else { "5" });
            if case == 0 {
                assert_eq!(rows[1][0], "6");
                let first: serde_json::Value = serde_json::from_str(&rows[0][1]).unwrap();
                let second: serde_json::Value = serde_json::from_str(&rows[1][1]).unwrap();
                assert_eq!(
                    first["table_info"]["materialized_view"]["alert_warning_sec"],
                    60
                );
                assert_eq!(second["old_table_info"], first["table_info"]);
                assert_eq!(
                    second["table_info"]["materialized_view"]["alert_warning_sec"],
                    61
                );
            } else {
                assert_eq!(version(&f), before);
            }
            assert_eq!(job.state, JobState::Done);
            let info = f.reader().get_table(f.db, f.table).unwrap().unwrap();
            assert_eq!(
                info.MaterializedView.as_ref().unwrap().AlertRefreshFailed,
                true
            );
        } else if case == 7 {
            assert!(rows.is_empty());
            assert_eq!(job.state, JobState::Done);
            assert!(version(&f) > before);
        } else {
            assert_eq!(job.state, JobState::Cancelled);
            let error = job.error.as_ref().unwrap();
            let expected = match case {
                3 | 4 => "1146",
                5 => "[ddl:1347]",
                6 => "8210",
                8 => "1049",
                _ => "",
            };
            assert!(error.contains(expected), "{error}");
            assert!(rows.is_empty());
            assert_eq!(version(&f), before);
        }
    }
}

fn delete_range_finished_job(f: &Fixture, id: i64, tp: u8, args: &str) {
    f.insert(id, JobState::Done);
    let mut job = f.queue(id).unwrap();
    job.tp = tp;
    job.version = astersql_meta_model::group_3::JobVersion::V2;
    job.raw_args = args.as_bytes().to_vec();
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET type={tp},job_meta=X'{}' WHERE job_id={id}",
            hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap())
        ))
        .unwrap();
}
fn delete_range_rows(f: &Fixture, id: i64) -> Vec<Vec<String>> {
    f.pool.acquire().unwrap().query(format!("SELECT element_id,start_key,end_key,ts FROM mysql.gc_delete_range WHERE job_id={id} ORDER BY element_id")).unwrap()
}
#[test]
fn normal_ddl_plan_delete_range_partition_table_and_index_finish() {
    use astersql_meta_model::group_3::{ACTION_ADD_INDEX, ACTION_DROP_TABLE};
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'retained-until-gc')")
        .unwrap();
    // Finished args retain the removed physical partitions, while the logical
    // table range covers any global-index regions.
    delete_range_finished_job(
        &f,
        86001,
        ACTION_DROP_TABLE,
        r#"{"old_partition_ids":[501,502]}"#,
    );
    let mut session = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    let rows = delete_range_rows(&f, 86001);
    assert_eq!(rows.len(), 3, "normal finish must persist GC ranges");
    for (i, tid) in [501, 502, f.table].into_iter().enumerate() {
        assert_eq!(rows[i][0], (i + 1).to_string());
        assert_eq!(
            rows[i][1],
            hex(astersql_tablecodec::EncodeTablePrefix(tid).as_ref())
        );
        assert_eq!(
            rows[i][2],
            hex(astersql_tablecodec::EncodeTablePrefix(tid + 1).as_ref())
        );
        assert!(rows[i][3].parse::<u64>().unwrap() > 0);
    }
    assert!(f.queue(86001).is_none());
    assert!(f.reader().get_history_ddl_job(86001).unwrap().is_some());
    delete_range_finished_job(
        &f,
        86002,
        ACTION_ADD_INDEX,
        r#"{"partition_ids":[501,502],"index_args":[{"index_id":11},{"index_id":12,"is_global":true}]}"#,
    );
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut session, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    let rows = delete_range_rows(&f, 86002);
    assert_eq!(rows.len(), 3);
    for (i, (tid, iid)) in [(501, 11), (502, 11), (f.table, 12)]
        .into_iter()
        .enumerate()
    {
        let temp = astersql_tablecodec::TempIndexPrefix | iid;
        assert_eq!(
            rows[i][1],
            hex(astersql_tablecodec::EncodeTableIndexPrefix(tid, temp).as_ref())
        );
        assert_eq!(
            rows[i][2],
            hex(astersql_tablecodec::EncodeTableIndexPrefix(tid, temp + 1).as_ref())
        );
    }
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=1")
            .unwrap()[0][0],
        "retained-until-gc"
    );
}

#[test]
fn normal_ddl_plan_delete_range_worker_conflict_preserves_gc_and_retry() {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'worker-conflict')")
        .unwrap();
    delete_range_finished_job(
        &f,
        86010,
        astersql_meta_model::group_3::ACTION_DROP_TABLE,
        r#"{"old_partition_ids":[601,602]}"#,
    );
    let mut worker = f.pool.acquire().unwrap();
    worker.begin().unwrap();
    let worker_ts = worker
        .with_transaction(Box::new(|t| Ok(t.StartTS().to_be_bytes().to_vec())))
        .unwrap();
    let mut job = f.queue(86010).unwrap();
    assert!(executor().step(&mut worker, &mut job).unwrap().removed);
    // GC is visible outside the still-open worker transaction.
    let gc = delete_range_rows(&f, 86010);
    assert_eq!(gc.len(), 3);
    assert!(gc.iter().all(|r| r[3].parse::<u64>().unwrap()
        > u64::from_be_bytes(worker_ts.as_slice().try_into().unwrap())));
    assert!(f.queue(86010).is_some());
    assert!(f.reader().get_history_ddl_job(86010).unwrap().is_none());
    f.pool
        .acquire()
        .unwrap()
        .query("UPDATE mysql.tidb_ddl_job SET processing=1 WHERE job_id=86010")
        .unwrap();
    let error = worker.commit().unwrap_err();
    assert!(error.contains(astersql_kv::TxnRetryableMark), "{error}");
    worker.rollback();
    assert!(f.queue(86010).is_some());
    assert!(f.reader().get_history_ddl_job(86010).unwrap().is_none());
    assert_eq!(delete_range_rows(&f, 86010), gc);
    // A fresh worker reloads the complete job. INSERT IGNORE preserves the
    // original range and timestamp instead of allocating duplicate elements.
    let lease = Lease(AtomicBool::new(true));
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    assert_eq!(delete_range_rows(&f, 86010), gc);
    assert!(f.queue(86010).is_none());
    assert!(f.reader().get_history_ddl_job(86010).unwrap().is_some());
}

#[test]
fn normal_ddl_plan_delete_range_gc_sql_failure_retries_before_history() {
    let f = Fixture::new();
    delete_range_finished_job(
        &f,
        86011,
        astersql_meta_model::group_3::ACTION_DROP_TABLE,
        r#"{"old_partition_ids":[611,612]}"#,
    );
    // A real missing system table produces the same SQL storage error a broken
    // bootstrap would return; no executor flags or fake successful rows.
    f.pool
        .acquire()
        .unwrap()
        .query("DROP TABLE mysql.gc_delete_range")
        .unwrap();
    let mut worker = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    let error = scheduler()
        .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
        .unwrap_err();
    assert!(error.contains("gc_delete_range"), "{error}");
    assert!(f.queue(86011).is_some());
    assert!(f.reader().get_history_ddl_job(86011).unwrap().is_none());
    f.pool
        .acquire()
        .unwrap()
        .query(astersql_meta_metadef::CreateGCDeleteRangeTable)
        .unwrap();
    assert!(delete_range_rows(&f, 86011).is_empty());
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    assert_eq!(delete_range_rows(&f, 86011).len(), 3);
    assert!(f.queue(86011).is_none());
}

#[test]
fn normal_ddl_plan_delete_range_finished_action_matrix_and_v1() {
    use astersql_meta_model::group_3::*;
    let f = Fixture::new();
    let mut worker = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    // Each expected (physical table, optional index) uses the Go tablecodec
    // identity. Cases cover every Go range-generating action, including MV GC.
    let cases: Vec<(u8, &str, Vec<(i64, Option<i64>)>)> = vec![
        (
            ACTION_DROP_SCHEMA,
            r#"{"all_dropped_table_ids":[701,702]}"#,
            vec![(701, None), (702, None)],
        ),
        (
            ACTION_TRUNCATE_TABLE,
            r#"{"old_partition_ids":[701]}"#,
            vec![(701, None), (f.table, None)],
        ),
        (
            ACTION_DROP_MATERIALIZED_VIEW,
            r#"{"old_partition_ids":[701]}"#,
            vec![(701, None), (f.table, None)],
        ),
        (
            ACTION_DROP_MATERIALIZED_VIEW_LOG,
            r#"{}"#,
            vec![(f.table, None)],
        ),
        (
            ACTION_DROP_MATERIALIZED_VIEW_SHADOW,
            r#"{}"#,
            vec![(f.table, None)],
        ),
        (
            ACTION_DROP_TABLE_PARTITION,
            r#"{"old_physical_tbl_ids":[701,702]}"#,
            vec![(701, None), (702, None)],
        ),
        (
            ACTION_TRUNCATE_TABLE_PARTITION,
            r#"{"old_partition_ids":[701,702]}"#,
            vec![(701, None), (702, None)],
        ),
        (
            ACTION_REORGANIZE_PARTITION,
            r#"{"old_physical_tbl_ids":[701],"old_global_indexes":[{"table_id":700,"index_id":17}]}"#,
            vec![(700, Some(17)), (701, None)],
        ),
        (
            ACTION_REMOVE_PARTITIONING,
            r#"{"old_physical_tbl_ids":[701],"old_global_indexes":[{"table_id":700,"index_id":17}]}"#,
            vec![(700, Some(17)), (701, None)],
        ),
        (
            ACTION_ALTER_TABLE_PARTITIONING,
            r#"{"old_physical_tbl_ids":[701]}"#,
            vec![(701, None)],
        ),
        (
            ACTION_DROP_INDEX,
            r#"{"partition_ids":[701,702],"index_args":[{"index_id":17}]}"#,
            vec![(701, Some(17)), (702, Some(17))],
        ),
        (
            ACTION_DROP_PRIMARY_KEY,
            r#"{"index_args":[{"index_id":17}]}"#,
            vec![(f.table, Some(17))],
        ),
        (
            ACTION_DROP_COLUMN,
            r#"{"index_ids":[17,18],"partition_ids":[701,702]}"#,
            vec![
                (701, Some(17)),
                (701, Some(18)),
                (702, Some(17)),
                (702, Some(18)),
            ],
        ),
        (
            ACTION_MODIFY_COLUMN,
            r#"{"index_ids":[17],"partition_ids":[701]}"#,
            vec![(701, Some(17))],
        ),
        (
            ACTION_ADD_PRIMARY_KEY,
            r#"{"index_args":[{"index_id":17}]}"#,
            vec![(f.table, Some(astersql_tablecodec::TempIndexPrefix | 17))],
        ),
        (
            ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER,
            r#"{"old_mview_id":701,"shadow_table_id":702}"#,
            vec![(701, None)],
        ),
        (
            ACTION_DROP_INDEX,
            r#"{"index_args":[{"index_id":17,"is_vector":true}]}"#,
            vec![],
        ),
        (ACTION_DROP_COLUMN, r#"{"index_ids":[]}"#, vec![]),
    ];
    for (i, (tp, args, expected)) in cases.into_iter().enumerate() {
        let id = 86100 + i as i64;
        delete_range_finished_job(&f, id, tp, args);
        assert_eq!(
            scheduler()
                .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
                .unwrap(),
            1,
            "action {tp}"
        );
        let rows = delete_range_rows(&f, id);
        assert_eq!(rows.len(), expected.len(), "action {tp}");
        for (j, (tid, index)) in expected.into_iter().enumerate() {
            let (start, end) = match index {
                Some(iid) => (
                    astersql_tablecodec::EncodeTableIndexPrefix(tid, iid),
                    astersql_tablecodec::EncodeTableIndexPrefix(tid, iid + 1),
                ),
                None => (
                    astersql_tablecodec::EncodeTablePrefix(tid),
                    astersql_tablecodec::EncodeTablePrefix(tid + 1),
                ),
            };
            assert_eq!(rows[j][0], (j + 1).to_string());
            assert_eq!(rows[j][1], hex(start.as_ref()));
            assert_eq!(rows[j][2], hex(end.as_ref()));
        }
        assert!(f.reader().get_history_ddl_job(id).unwrap().is_some());
    }
    // Legacy finished arguments use V1 arrays, not the V2 object layout.
    for (i, (tp, args, count)) in [
        (ACTION_DROP_TABLE, r#"["",[701,702],[]]"#, 3),
        (
            ACTION_DROP_INDEX,
            r#"[{"O":"idx","L":"idx"},false,17,[701,702],false]"#,
            2,
        ),
        (ACTION_ADD_INDEX, r#"[17,false,[701,702],false]"#, 2),
        (ACTION_ADD_INDEX, r#"[17,false,null,false]"#, 1),
        (
            ACTION_DROP_INDEX,
            r#"[{"O":"idx","L":"idx"},false,17,null,false]"#,
            1,
        ),
        (ACTION_TRUNCATE_TABLE, r#"["dGVzdA==",null]"#, 1),
        (ACTION_MODIFY_COLUMN, r#"[[17],[701],[]]"#, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let id = 86200 + i as i64;
        delete_range_finished_job(&f, id, tp, "{}");
        let mut job = f.queue(id).unwrap();
        job.version = JobVersion::V1;
        job.raw_args = args.as_bytes().to_vec();
        worker
            .query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={id}",
                hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap())
            ))
            .unwrap();
        assert_eq!(
            scheduler()
                .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
                .unwrap(),
            1
        );
        assert_eq!(delete_range_rows(&f, id).len(), count);
    }
}

#[test]
fn normal_ddl_plan_delete_range_subjobs_rollback_cancel_and_warning() {
    use astersql_meta_model::group_3::*;
    let f = Fixture::new();
    let mut worker = f.pool.acquire().unwrap();
    let lease = Lease(AtomicBool::new(true));
    let cases = [
        (
            86301,
            ACTION_ADD_INDEX,
            JobState::RollbackDone,
            None,
            r#"{"partition_ids":[701],"index_args":[{"index_id":17}]}"#,
            2,
        ),
        (
            86302,
            ACTION_CREATE_MATERIALIZED_VIEW,
            JobState::RollbackDone,
            None,
            "{}",
            1,
        ),
        (
            86303,
            ACTION_CREATE_MATERIALIZED_VIEW,
            JobState::Done,
            None,
            "{}",
            0,
        ),
        (
            86304,
            ACTION_DROP_TABLE,
            JobState::Cancelled,
            None,
            r#"{"old_partition_ids":[701]}"#,
            0,
        ),
        (
            86305,
            ACTION_DROP_INDEX,
            JobState::Done,
            Some("[ddl:1091]Can't DROP 'absent'; check that column/key exists"),
            r#"{"index_args":[{"index_id":17}]}"#,
            0,
        ),
    ];
    for (id, tp, state, warning, args, count) in cases {
        delete_range_finished_job(&f, id, tp, args);
        let mut job = f.queue(id).unwrap();
        job.state = state;
        job.warning = warning.map(str::to_owned);
        worker
            .query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={id}",
                hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap())
            ))
            .unwrap();
        assert_eq!(
            scheduler()
                .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
                .unwrap(),
            1
        );
        let rows = delete_range_rows(&f, id);
        assert_eq!(rows.len(), count);
        if id == 86301 {
            assert_eq!(
                rows[0][1],
                hex(astersql_tablecodec::EncodeTableIndexPrefix(701, 17).as_ref())
            );
            assert_eq!(
                rows[1][1],
                hex(astersql_tablecodec::EncodeTableIndexPrefix(
                    701,
                    astersql_tablecodec::TempIndexPrefix | 17
                )
                .as_ref())
            );
        }
    }
    delete_range_finished_job(&f, 86310, ACTION_MULTI_SCHEMA_CHANGE, "{}");
    let mut job = f.queue(86310).unwrap();
    job.multi_schema_info = Some(MultiSchemaInfo {
        sub_jobs: vec![
            SubJob {
                tp: ACTION_DROP_INDEX,
                state: JobState::Done,
                raw_args: br#"{"index_args":[{"index_id":17}]}"#.to_vec(),
                ..Default::default()
            },
            // The same index keeps its element ID across proxy jobs.
            SubJob {
                tp: ACTION_DROP_COLUMN,
                state: JobState::Done,
                raw_args: br#"{"index_ids":[17,18]}"#.to_vec(),
                ..Default::default()
            },
            SubJob {
                tp: ACTION_DROP_COLUMN,
                state: JobState::Cancelled,
                raw_args: br#"{"index_ids":[19]}"#.to_vec(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    worker
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id=86310",
            hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap())
        ))
        .unwrap();
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut worker, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    let rows = delete_range_rows(&f, 86310);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[1][0], "2");
    assert_eq!(
        rows[1][1],
        hex(astersql_tablecodec::EncodeTableIndexPrefix(f.table, 18).as_ref())
    );
}

#[test]
fn normal_ddl_plan_delete_range_gc_commit_conflict_rolls_back_batch() {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO mysql.gc_delete_range VALUES (86400,1,'00','01',1)")
        .unwrap();
    delete_range_finished_job(
        &f,
        86401,
        astersql_meta_model::group_3::ACTION_DROP_SCHEMA,
        r#"{"all_dropped_table_ids":[801,802]}"#,
    );
    let mut job = f.queue(86401).unwrap();
    let mut gc = f.pool.acquire().unwrap();
    // Hold the GC commit boundary open solely to force a deterministic real
    // MVCC conflict. The complete original range INSERT executes on the real
    // independent SQL session; no range, row or storage error is mocked.
    gc.begin().unwrap();
    gc.query("UPDATE mysql.gc_delete_range SET ts=2 WHERE job_id=86400")
        .unwrap();
    astersql_ddl::delete_range::add_persistent_delete_range_job(&mut gc, &mut job).unwrap();
    assert_eq!(
        gc.query("SELECT job_id FROM mysql.gc_delete_range WHERE job_id=86401")
            .unwrap()
            .len(),
        2
    );
    assert!(delete_range_rows(&f, 86401).is_empty());
    f.pool
        .acquire()
        .unwrap()
        .query("UPDATE mysql.gc_delete_range SET ts=3 WHERE job_id=86400")
        .unwrap();
    let error = gc.commit().unwrap_err();
    assert!(error.contains(astersql_kv::TxnRetryableMark), "{error}");
    gc.rollback();
    assert!(delete_range_rows(&f, 86401).is_empty());
    assert!(f.queue(86401).is_some());
    assert!(f.reader().get_history_ddl_job(86401).unwrap().is_none());
    let lease = Lease(AtomicBool::new(true));
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut gc, &lease, &mut executor(), 0)
            .unwrap(),
        1
    );
    assert_eq!(delete_range_rows(&f, 86401).len(), 2);
}

fn table_validation_fixture() -> Fixture {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (7, 'validation row')")
        .unwrap();
    f
}

fn table_validation_seed(f: &Fixture, table: &astersql_meta_model::TableInfo) {
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
}

fn table_validation_index(
    table: &astersql_meta_model::TableInfo,
) -> astersql_meta_model::IndexInfo {
    astersql_meta_model::IndexInfo {
        ID: 123,
        Name: astersql_meta_model::ast::NewCIStr("validation_key"),
        Table: table.Name.clone(),
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[0].Name.clone(),
            Offset: 0,
            Length: -1,
            ..Default::default()
        }],
        State: astersql_meta_model::SchemaState::Public,
        Unique: true,
        ..Default::default()
    }
}

fn table_validation_run(f: &Fixture, expected_error: Option<&str>) {
    let before_table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let before_wire = astersql_meta_model::EncodeTableInfo(&before_table).unwrap();
    let before_version = version(f);
    f.insert(88001, JobState::Queueing);
    let mut s = scheduler();
    let mut e = executor();
    let lease = Lease(AtomicBool::new(true));
    let mut session = f.pool.acquire().unwrap();
    if let Some(expected) = expected_error {
        // Exercise the production checked entry through the real worker SQL/KV
        // context. Existing Go TableMode is unchecked, so do not alter its
        // dispatcher semantics just to create a validation test consumer.
        let mut current = f.queue(88001).unwrap();
        let mut candidate = before_table.clone();
        candidate.Mode = astersql_meta_model::TableMode::TableModeImport;
        let expected = expected.to_string();
        session.begin().unwrap();
        let encoded = session
            .with_execution_context(Box::new(move |context| {
                context.with_transaction(&mut |txn| {
                    let mut meta = astersql_meta::TransactionMutator::new(txn);
                    let error =
                        astersql_ddl::persistent_actions::update_version_and_table_with_check(
                            &mut meta,
                            &mut current,
                            &mut candidate,
                        )
                        .unwrap_err();
                    assert!(error.contains(&expected), "{error}");
                    assert_eq!(current.state, JobState::Cancelled);
                    current.error = Some(error);
                    current.error_count += 1;
                    current.encode(false).map_err(|e| e.to_string())
                })
            }))
            .unwrap();
        // The ordinary executor persists the returned error state separately
        // from action metadata, then the real scheduler performs cancellation
        // history/queue cleanup. Keep all SQL on this same worker transaction.
        session
            .query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id=88001",
                hex(&encoded)
            ))
            .unwrap();
        session.commit().unwrap();
    } else {
        let mut current = f.queue(88001).unwrap();
        let mut candidate = before_table.clone();
        candidate.Mode = astersql_meta_model::TableMode::TableModeImport;
        session.begin().unwrap();
        session
            .with_execution_context(Box::new(move |context| {
                context.with_transaction(&mut |txn| {
                    let mut meta = astersql_meta::TransactionMutator::new(txn);
                    let v = astersql_ddl::persistent_actions::update_version_and_table_with_check(
                        &mut meta,
                        &mut current,
                        &mut candidate,
                    )?;
                    assert_eq!(v, before_version + 1);
                    assert_eq!(current.state, JobState::Queueing);
                    Ok(Vec::new())
                })
            }))
            .unwrap();
        session.rollback();
        assert_eq!(version(f), before_version);
        assert_eq!(
            astersql_meta_model::EncodeTableInfo(
                &f.reader().get_table(f.db, f.table).unwrap().unwrap()
            )
            .unwrap(),
            before_wire
        );
    }
    assert_eq!(
        s.schedule_persisted(&mut session, &lease, &mut e, 0)
            .unwrap(),
        1
    );
    let after_table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    if let Some(error) = expected_error {
        assert!(
            f.queue(88001).is_none(),
            "invalid metadata must cancel immediately, not finish Done or retry"
        );
        let history = f.reader().get_history_ddl_job(88001).unwrap().unwrap();
        assert_eq!(history.state, JobState::Cancelled);
        assert_eq!(history.error_count, 1);
        assert!(
            history.error.as_deref().unwrap().contains(error),
            "{:?}",
            history.error
        );
        assert_eq!(history.last_schema_version, 0);
        assert_eq!(version(f), before_version);
        assert_eq!(
            astersql_meta_model::EncodeTableInfo(&after_table).unwrap(),
            before_wire
        );
        assert!(e.barrier.seen.is_empty());
    } else {
        assert_eq!(f.queue(88001).unwrap().state, JobState::Done);
        assert_eq!(
            after_table.Mode,
            astersql_meta_model::TableMode::TableModeImport
        );
        assert_eq!(version(f), before_version + 1);
        assert_eq!(e.barrier.seen, vec![before_version + 1]);
        // Normal updates may change revision/update-ts, but no unrelated Go metadata.
        let mut actual = after_table.clone();
        actual.Mode = before_table.Mode;
        actual.Revision = before_table.Revision;
        actual.UpdateTS = before_table.UpdateTS;
        assert_eq!(
            astersql_meta_model::EncodeTableInfo(&actual).unwrap(),
            before_wire
        );
        assert_eq!(
            s.schedule_persisted(&mut session, &lease, &mut e, 0)
                .unwrap(),
            1
        );
        assert!(f.queue(88001).is_none());
        let history = f.reader().get_history_ddl_job(88001).unwrap().unwrap();
        assert_eq!(history.state, JobState::Synced);
        assert_eq!(history.error_count, 0);
        assert_eq!(
            astersql_meta_model::EncodeTableInfo(
                history
                    .binlog_info
                    .as_ref()
                    .unwrap()
                    .table_info
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
            astersql_meta_model::EncodeTableInfo(&after_table).unwrap()
        );
    }
    let sql_history = session
        .query("SELECT job_meta FROM mysql.tidb_ddl_history WHERE job_id=88001")
        .unwrap();
    assert_eq!(sql_history.len(), 1);
    let recorded = astersql_meta::decode_go_history_job(sql_history[0][0].as_bytes()).unwrap();
    assert_eq!(
        recorded.state,
        if expected_error.is_some() {
            JobState::Cancelled
        } else {
            JobState::Synced
        }
    );
    assert_eq!(
        recorded.error_count,
        if expected_error.is_some() { 1 } else { 0 }
    );
    let rows = session
        .query("SELECT id,payload FROM test.normal_ddl_target")
        .unwrap();
    assert_eq!(
        rows,
        vec![vec!["7".to_string(), "validation row".to_string()]]
    );
}

#[test]
fn normal_ddl_plan_table_validation_rejects_none_states_atomically() {
    for index in [false, true] {
        let f = table_validation_fixture();
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let error = if index {
            let mut key = table_validation_index(&table);
            key.State = astersql_meta_model::SchemaState::None;
            table.Indices.push(key);
            "[table:8044]"
        } else {
            table.Columns[1].State = astersql_meta_model::SchemaState::None;
            "[table:8046]"
        };
        table_validation_seed(&f, &table);
        table_validation_run(&f, Some(error));
    }
}

#[test]
fn normal_ddl_plan_table_validation_invisible_explicit_and_implicit_primary() {
    for explicit in [false, true] {
        let f = table_validation_fixture();
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        table.PKIsHandle = false;
        let mut key = table_validation_index(&table);
        key.Primary = explicit;
        key.Invisible = true;
        table.Indices = vec![key];
        table_validation_seed(&f, &table);
        table_validation_run(&f, Some("[ddl:3522]"));
    }
}

#[test]
fn normal_ddl_plan_table_validation_go_offsets_cache_and_primary_exceptions() {
    for case in 0..6 {
        let f = table_validation_fixture();
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        table.Comment = "preserve complete metadata".into();
        table.AutoIDCache = 17;
        let mut key = table_validation_index(&table);
        match case {
            0 => {
                table.Columns[1].Offset = -4;
                key.Columns[0].Offset = 99;
            }
            1 => table.TableCacheStatusType = astersql_meta_model::TableCacheStatusEnable,
            2 => table.TableCacheStatusType = astersql_meta_model::TableCacheStatusSwitching,
            3 => {
                key.Primary = true;
                key.Invisible = true;
                assert!(table.PKIsHandle);
            }
            4 => {
                table.PKIsHandle = false;
                table.Columns[0].Hidden = true;
                key.Invisible = true;
            }
            _ => {
                table.PKIsHandle = false;
                table.Columns[0].State = astersql_meta_model::SchemaState::DeleteOnly;
                key.Invisible = true;
            }
        }
        table.Indices = vec![key];
        table_validation_seed(&f, &table);
        table_validation_run(&f, None);
    }
}

#[test]
fn normal_ddl_plan_table_validation_partition_structure_and_deferred_loading() {
    for case in 0..3 {
        let f = table_validation_fixture();
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        table.Partition = Some(astersql_meta_model::PartitionInfo {
            Enable: case != 2,
            Type: astersql_meta_model::ast::model::PartitionTypeHash,
            Expr: "id".into(),
            Definitions: if case == 1 {
                vec![astersql_meta_model::PartitionDefinition {
                    ID: 88100,
                    Name: astersql_meta_model::ast::NewCIStr("p0"),
                    ..Default::default()
                }]
            } else {
                vec![]
            },
            ..Default::default()
        });
        table_validation_seed(&f, &table);
        table_validation_run(
            &f,
            match case {
                0 => Some("[table:1735]"),
                1 => Some("partition expression loading is not supported"),
                _ => None,
            },
        );
    }
}

#[test]
fn normal_ddl_plan_table_validation_partial_index_is_explicitly_deferred() {
    let f = table_validation_fixture();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut key = table_validation_index(&table);
    key.ConditionExprString = "id > 0".into();
    table.Indices.push(key);
    table_validation_seed(&f, &table);
    table_validation_run(
        &f,
        Some("partial index expression loading is not supported"),
    );
}

#[test]
fn normal_ddl_plan_table_validation_checked_update_commit_rollback_and_skip_version() {
    for case in 0..3 {
        let f = table_validation_fixture();
        f.insert(88001, JobState::Queueing);
        let mut job = f.queue(88001).unwrap();
        if case == 2 {
            job.multi_schema_info = Some(astersql_meta_model::group_3::MultiSchemaInfo {
                skip_version: true,
                ..Default::default()
            });
        }
        let before_table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let before_wire = astersql_meta_model::EncodeTableInfo(&before_table).unwrap();
        let before_version = version(&f);
        let mut candidate = before_table.clone();
        candidate.Comment = "checked worker context".into();
        candidate.Columns[1].GeneratedExprString = "id + 1".into();
        candidate.Columns[0].DefaultIsExpr = true;
        candidate.Columns[0].DefaultValue = Some(astersql_meta_model::DefaultValue::String(
            b"abs(-7)".to_vec(),
        ));
        for column in ["id", "missing"] {
            candidate
                .Constraints
                .push(astersql_meta_model::ConstraintInfo {
                    Name: astersql_meta_model::ast::NewCIStr(format!("check_{column}")),
                    ConstraintCols: vec![astersql_meta_model::ast::NewCIStr(column)],
                    ExprString: format!("{column} > 0"),
                    State: astersql_meta_model::SchemaState::Public,
                    Enforced: true,
                    ..Default::default()
                });
        }
        let mut session = f.pool.acquire().unwrap();
        session.begin().unwrap();
        let result = session
            .with_execution_context(Box::new(move |context| {
                context.with_transaction(&mut |txn| {
                    let mut meta = astersql_meta::TransactionMutator::new(txn);
                    let v = astersql_ddl::persistent_actions::update_version_and_table_with_check(
                        &mut meta,
                        &mut job,
                        &mut candidate,
                    )?;
                    assert_eq!(v, if case == 2 { 0 } else { before_version + 1 });
                    assert_eq!(job.state, JobState::Queueing);
                    astersql_meta_model::EncodeTableInfo(&candidate).map_err(|e| e.to_string())
                })
            }))
            .unwrap();
        if case == 1 {
            session.rollback();
        } else {
            session.commit().unwrap();
        }
        let after = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let after_wire = astersql_meta_model::EncodeTableInfo(&after).unwrap();
        assert_eq!(after_wire, if case == 1 { before_wire } else { result });
        assert_eq!(version(&f), before_version + if case == 0 { 1 } else { 0 });
        assert_eq!(f.queue(88001).unwrap().state, JobState::Queueing);
        assert!(f.reader().get_history_ddl_job(88001).unwrap().is_none());
        assert_eq!(
            session
                .query("SELECT payload FROM test.normal_ddl_target WHERE id=7")
                .unwrap(),
            vec![vec!["validation row".to_string()]]
        );
    }
}

// Full Go-wire catalog candidates run through the checked worker transaction.
fn expression_validation_run(
    mut candidate: astersql_meta_model::TableInfo,
    expected: Option<&str>,
    expected_constraints: usize,
) {
    let f = table_validation_fixture();
    let old = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    candidate.ID = old.ID;
    candidate.Name = old.Name.clone();
    let before = astersql_meta_model::EncodeTableInfo(&old).unwrap();
    let before_version = version(&f);
    f.insert(88101, JobState::Queueing);
    let mut job = f.queue(88101).unwrap();
    let expected = expected.map(str::to_string);
    let reject = expected.is_some();
    let mut session = f.pool.acquire().unwrap();
    session.begin().unwrap();
    let output = session
        .with_execution_context(Box::new(move |context| {
            context.with_transaction(&mut |txn| {
                let mut meta = astersql_meta::TransactionMutator::new(txn);
                let result = astersql_ddl::persistent_actions::update_version_and_table_with_check(
                    &mut meta,
                    &mut job,
                    &mut candidate,
                );
                if let Some(expected) = &expected {
                    let error = result.unwrap_err();
                    assert!(error.contains(expected), "{error}");
                    assert_eq!(job.state, JobState::Cancelled);
                } else {
                    assert_eq!(result?, before_version + 1);
                    assert_eq!(candidate.Constraints.len(), expected_constraints);
                }
                astersql_meta_model::EncodeTableInfo(&candidate).map_err(|e| e.to_string())
            })
        }))
        .unwrap();
    // Committing the surrounding transaction must not expose any action writes on error.
    session.commit().unwrap();
    let after = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(
        astersql_meta_model::EncodeTableInfo(&after).unwrap(),
        if reject { before } else { output }
    );
    assert_eq!(version(&f), before_version + if reject { 0 } else { 1 });
    assert_eq!(
        session
            .query("SELECT id,payload FROM test.normal_ddl_target")
            .unwrap(),
        vec![vec!["7".to_string(), "validation row".to_string()]]
    );
}

fn expression_validation_candidate() -> astersql_meta_model::TableInfo {
    let f = table_validation_fixture();
    f.reader().get_table(f.db, f.table).unwrap().unwrap()
}

#[test]
fn normal_ddl_plan_expression_validation_generated_and_defaults() {
    for expression in [
        "ID + 1",
        "case when id > 0 then abs(id) else 0 end",
        "test.normal_ddl_target.id + 2",
    ] {
        let mut table = expression_validation_candidate();
        table.Columns[1].GeneratedExprString = expression.into();
        table.Columns[0].DefaultIsExpr = true;
        // Go default parsing intentionally does not resolve column names.
        table.Columns[0].DefaultValue = Some(astersql_meta_model::DefaultValue::String(
            b"unknown_default_name + 1".to_vec(),
        ));
        expression_validation_run(table, None, 0);
    }
}

#[test]
fn normal_ddl_plan_expression_validation_rejects_generated_and_default_errors() {
    for (generated, expression, error) in [
        (true, "missing_column + id", "can't find column"),
        (true, "id +", "1064"),
        (false, "abs(", "1064"),
    ] {
        let mut table = expression_validation_candidate();
        if generated {
            table.Columns[1].GeneratedExprString = expression.into();
        } else {
            table.Columns[1].DefaultIsExpr = true;
            table.Columns[1].DefaultValue = Some(astersql_meta_model::DefaultValue::String(
                expression.as_bytes().to_vec(),
            ));
        }
        expression_validation_run(table, Some(error), 0);
    }
}

#[test]
fn normal_ddl_plan_expression_validation_check_loading_go_boundaries() {
    let mut table = expression_validation_candidate();
    for (name, column, expr) in [
        ("keep", "id", "id > 0"),
        ("keep_unparsed", "id", "id >"),
        ("remove_missing", "missing", "missing > 0"),
        ("remove_nonpublic", "payload", "payload <> ''"),
    ] {
        table.Constraints.push(astersql_meta_model::ConstraintInfo {
            ID: table.Constraints.len() as i64 + 1,
            Name: astersql_meta_model::ast::NewCIStr(name),
            ConstraintCols: vec![astersql_meta_model::ast::NewCIStr(column)],
            ExprString: expr.into(),
            Enforced: true,
            State: astersql_meta_model::SchemaState::Public,
            ..Default::default()
        });
    }
    table.Columns[1].State = astersql_meta_model::SchemaState::WriteOnly;
    expression_validation_run(table, None, 2);
}

#[test]
fn normal_ddl_plan_reorg_checkpoint_restart_and_conflict() {
    use astersql_ddl::reorg::{PersistentReorgHandler, ReorgElement, ReorgInfo};
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'existing'),(9,'last')")
        .unwrap();
    f.insert(87001, JobState::Running);
    let mut job = f.queue(87001).unwrap();
    job.snapshot_ver = f
        .domain
        .storage_handle()
        .with_storage(|s| s.CurrentVersion("global"))
        .unwrap()
        .Ver;
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
            hex(&job.encode(false).unwrap()),
            job.id
        ))
        .unwrap();
    let mut start = b"t".to_vec();
    start.extend_from_slice(&((f.table as u64) ^ (1u64 << 63)).to_be_bytes());
    start.extend_from_slice(b"_r");
    start.extend_from_slice(&((1u64) ^ (1u64 << 63)).to_be_bytes());
    let mut end = start[..11].to_vec();
    end.extend_from_slice(&((9u64) ^ (1u64 << 63)).to_be_bytes());
    end.push(0);
    let mut info = ReorgInfo {
        job_id: job.id,
        physical_table_id: f.table,
        start_key: start.clone(),
        end_key: end.clone(),
        element: ReorgElement {
            id: 11,
            element_type: b"_idx_".to_vec(),
        },
        ..Default::default()
    };
    let mut a = f.pool.acquire().unwrap();
    PersistentReorgHandler::initialize(&mut a, &info).unwrap();
    drop(a);
    let replacement_pool = SystemSessionPool::new(f.domain.clone());
    let mut b = replacement_pool.acquire().unwrap();
    let restored = PersistentReorgHandler::restore(&mut b, &mut f.queue(job.id).unwrap()).unwrap();
    assert_eq!(restored.snapshot_ver, job.snapshot_ver);
    assert_eq!(restored.info, info);
    assert_eq!(restored.runtime.row_count(), 0);
    let mut recovered_job = f.queue(job.id).unwrap();
    let expected_info = info.clone();
    let expected_snapshot = job.snapshot_ver;
    b.begin().unwrap();
    b.with_execution_context(Box::new(move |context| {
        let recovered = context.restore_reorg(&mut recovered_job)?;
        assert_eq!(recovered.info, expected_info);
        assert_eq!(recovered.snapshot_ver, expected_snapshot);
        Ok(Vec::new())
    }))
    .unwrap();
    b.rollback();
    let meta = b
        .query(format!(
            "SELECT reorg_meta FROM mysql.tidb_ddl_reorg WHERE job_id={}",
            job.id
        ))
        .unwrap();
    let meta: serde_json::Value = serde_json::from_str(&meta[0][0]).unwrap();
    assert_eq!(meta["reorg_checkpoint"]["physical_id"], f.table);
    assert_eq!(meta["reorg_checkpoint"]["version"], 1);

    let mut next = start.clone();
    next.push(0);
    b.begin().unwrap();
    PersistentReorgHandler::stage_update(&mut b, &info, &next).unwrap();
    let mut c = f.pool.acquire().unwrap();
    PersistentReorgHandler::update(&mut c, &mut info, next.clone()).unwrap();
    assert!(b.commit().is_err());
    b.rollback();
    assert_eq!(
        PersistentReorgHandler::restore(&mut c, &mut job)
            .unwrap()
            .info
            .start_key,
        next
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target ORDER BY id")
            .unwrap(),
        vec![vec!["existing".to_string()], vec!["last".to_string()]]
    );
}

// Inject a competing real MVCC commit at the committing session boundary.
struct ReorgCommitConflict<'a> {
    session: &'a mut dyn DurableJobSession,
    competing: &'a mut dyn DurableJobSession,
    info: astersql_ddl::reorg::ReorgInfo,
}
impl DurableJobSession for ReorgCommitConflict<'_> {
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        self.session.query(sql, label)
    }
    fn begin(&mut self) -> Result<(), String> {
        self.session.begin()
    }
    fn rollback(&mut self) {
        self.session.rollback();
    }
    fn commit(&mut self) -> Result<(), String> {
        astersql_ddl::reorg::PersistentReorgHandler::update(
            self.competing,
            &mut self.info,
            vec![2, 128, 255],
        )?;
        self.session.commit()
    }
    fn with_transaction(
        &mut self,
        op: astersql_ddl::job_worker::TransactionOperation,
    ) -> Result<Vec<u8>, String> {
        self.session.with_transaction(op)
    }
}
#[test]
fn normal_ddl_plan_reorg_checkpoint_failed_commit_and_lifecycle() {
    use astersql_ddl::reorg::{PersistentReorgHandler, ReorgElement, ReorgInfo};
    let f = Fixture::new();
    f.insert(87002, JobState::Running);
    let mut job = f.queue(87002).unwrap();
    job.snapshot_ver = 42;
    let mut info = ReorgInfo {
        job_id: job.id,
        physical_table_id: f.table,
        start_key: vec![0, 128, 255],
        end_key: vec![255, 0],
        element: ReorgElement {
            id: 12,
            element_type: b"_col_".to_vec(),
        },
        ..Default::default()
    };
    let mut a = f.pool.acquire().unwrap();
    PersistentReorgHandler::initialize(&mut a, &info).unwrap();
    for state in [
        JobState::Paused,
        JobState::Cancelling,
        JobState::Rollingback,
    ] {
        job.state = state;
        PersistentReorgHandler::cleanup(&mut a, &job).unwrap();
        assert_eq!(
            PersistentReorgHandler::restore(&mut a, &mut job)
                .unwrap()
                .info,
            info
        );
    }
    let original = info.clone();
    let mut b = f.pool.acquire().unwrap();
    let mut conflict = ReorgCommitConflict {
        session: &mut a,
        competing: &mut b,
        info: info.clone(),
    };
    let error =
        PersistentReorgHandler::update(&mut conflict, &mut info, vec![1, 128, 255]).unwrap_err();
    assert!(error.contains(astersql_kv::TxnRetryableMark), "{error}");
    assert_eq!(
        info, original,
        "failed commit must not advance local checkpoint"
    );
    info.start_key = vec![2, 128, 255];
    assert_eq!(
        PersistentReorgHandler::restore(&mut b, &mut job)
            .unwrap()
            .info,
        info
    );
    drop(a);
    drop(b);
    let mut restarted = f.pool.acquire().unwrap();
    assert_eq!(
        PersistentReorgHandler::restore(&mut restarted, &mut job)
            .unwrap()
            .info,
        info
    );
    job.state = JobState::RollbackDone;
    PersistentReorgHandler::cleanup(&mut restarted, &job).unwrap();
    assert!(PersistentReorgHandler::restore(&mut restarted, &mut job).is_err());
    assert_eq!(job.snapshot_ver, 0);
    job.reorg_meta = Some(Default::default());
    job.reorg_meta.as_mut().unwrap().Version = 0;
    PersistentReorgHandler::initialize(&mut restarted, &info).unwrap();
    let legacy = PersistentReorgHandler::restore(&mut restarted, &mut job).unwrap();
    let mut legacy_end = info.end_key.clone();
    legacy_end.push(0);
    assert_eq!(legacy.info.end_key, legacy_end);
}

fn backfill_owner_fixture(
    unique: bool,
    values: &str,
) -> (Fixture, Job, astersql_ddl::reorg::PersistentReorgContext) {
    use astersql_ddl::reorg::{PersistentReorgHandler, ReorgElement, ReorgInfo};
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "INSERT INTO test.normal_ddl_target VALUES {values}"
        ))
        .unwrap();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: 81,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: astersql_meta_model::ast::NewCIStr("payload"),
            Offset: 1,
            Length: astersql_meta_model::types::UnspecifiedLength,
            ..Default::default()
        }],
        Unique: unique,
        State: astersql_meta_model::SchemaState::WriteReorganization,
        ..Default::default()
    });
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    f.insert(89001, JobState::Running);
    let mut job = f.queue(89001).unwrap();
    job.tp = astersql_meta_model::group_3::ACTION_ADD_INDEX;
    job.schema_state = astersql_meta_model::SchemaState::WriteReorganization;
    job.raw_args = serde_json::json!({"index_args":[{"unique":unique,"index_name":{"O":"payload_idx","L":"payload_idx"},"index_id":81,"index_part_specifications":[]}]}).to_string().into_bytes();
    job.reorg_meta = Some(astersql_meta_model::group_3::DDLReorgMeta {
        ReorgTp: astersql_meta_model::group_3::ReorgType::ReorgTypeTxn,
        Version: astersql_meta_model::group_3::CurrentReorgMetaVersion,
        ..Default::default()
    });
    job.reorg_meta.as_mut().unwrap().SetBatchSize(1);
    job.snapshot_ver = f
        .domain
        .storage_handle()
        .with_storage(|s| s.CurrentVersion("global"))
        .unwrap()
        .Ver;
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET type={},reorg=1,job_meta=X'{}' WHERE job_id=89001",
            astersql_meta_model::group_3::ACTION_ADD_INDEX,
            hex(&astersql_meta::encode_go_ddl_job(&mut job, false).unwrap())
        ))
        .unwrap();
    let start = astersql_tablecodec::GenTableRecordPrefix(f.table).0;
    let end = astersql_kv::Key(start.clone()).PrefixNext().0;
    let info = ReorgInfo {
        job_id: job.id,
        physical_table_id: f.table,
        start_key: start,
        end_key: end,
        element: ReorgElement {
            id: 81,
            element_type: b"_idx_".to_vec(),
        },
        ..Default::default()
    };
    let mut session = f.pool.acquire().unwrap();
    PersistentReorgHandler::initialize(&mut session, &info).unwrap();
    let reorg = PersistentReorgHandler::restore(&mut session, &mut job).unwrap();
    (f, job, reorg)
}
fn backfill_owner_entries(f: &Fixture) -> Vec<(Vec<u8>, Vec<u8>)> {
    let prefix = astersql_tablecodec::EncodeTableIndexPrefix(f.table, 81).0;
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let mut iter = txn
        .Iter(
            astersql_kv::Key(prefix.clone()),
            Some(astersql_kv::Key(prefix).PrefixNext()),
        )
        .unwrap();
    let mut rows = Vec::new();
    while iter.Valid() {
        rows.push((iter.Key().0, iter.Value()));
        iter.Next().unwrap();
    }
    iter.Close();
    txn.Rollback().unwrap();
    rows
}
#[test]
fn normal_ddl_plan_backfill_owner_real_rows_checkpoint_and_replay() {
    let (f, job, mut reorg) = backfill_owner_fixture(true, "(1,'one'),(2,'two'),(3,NULL)");
    let mut session = f.pool.acquire().unwrap();
    let mut worker = JobWorker::new(WorkerType::AddIndex);
    let result = worker
        .run_transactional_index_backfill(
            &mut session,
            &backfill_election(),
            &job,
            &mut reorg,
            &[81],
        )
        .unwrap();
    assert_eq!(result.total_scan_count, 3);
    assert_eq!(result.total_added_count, 3);
    assert_eq!(reorg.info.start_key, reorg.info.end_key);
    assert_eq!(backfill_owner_entries(&f).len(), 3);
    assert_eq!(reorg.runtime.row_count(), 3);
    // Checkpoint lag is possible in Go: replay committed rows with actual
    // canonical unique keys, including a non-distinct NULL entry.
    let prior = backfill_owner_entries(&f);
    reorg.info.start_key = astersql_tablecodec::GenTableRecordPrefix(f.table).0;
    let replay = worker
        .run_transactional_index_backfill(
            &mut session,
            &backfill_election(),
            &job,
            &mut reorg,
            &[81],
        )
        .unwrap();
    assert_eq!(replay.total_added_count, 0);
    assert_eq!(backfill_owner_entries(&f), prior);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT id,payload FROM test.normal_ddl_target ORDER BY id")
            .unwrap()
            .len(),
        3
    );
}

struct BackfillElection {
    owner: AtomicBool,
    epoch: std::sync::atomic::AtomicU64,
    cancelled: AtomicBool,
}
impl JobLease for BackfillElection {
    fn is_owner(&self) -> bool {
        self.owner.load(Ordering::Acquire)
    }
    fn owner_epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
struct BackfillBoundaryFault {
    session: super::system_session::SystemSessionLease,
    after_scan: Option<Box<dyn FnOnce()>>,
    attempts: usize,
    stop_after: usize,
}
impl DurableJobSession for BackfillBoundaryFault {
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        DurableJobSession::query(&mut self.session, sql, label)
    }
    fn begin(&mut self) -> Result<(), String> {
        DurableJobSession::begin(&mut self.session)
    }
    fn commit(&mut self) -> Result<(), String> {
        DurableJobSession::commit(&mut self.session)
    }
    fn rollback(&mut self) {
        DurableJobSession::rollback(&mut self.session)
    }
    fn with_transaction(
        &mut self,
        op: astersql_ddl::job_worker::TransactionOperation,
    ) -> Result<Vec<u8>, String> {
        DurableJobSession::with_transaction(&mut self.session, op)
    }
    fn backfill_index_batch(
        &mut self,
        request: astersql_ddl::backfilling::IndexBackfillBatch,
    ) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
        self.attempts += 1;
        let result = self.session.backfill_index_batch(request)?;
        if self.attempts == self.stop_after {
            if let Some(fault) = self.after_scan.take() {
                fault();
            }
        }
        Ok(result)
    }
}
#[test]
fn normal_ddl_plan_backfill_owner_fences_loss_cancel_pause_and_same_owner_new_term() {
    use astersql_ddl::reorg::PersistentReorgHandler;
    for case in 0..5 {
        let (f, mut job, mut reorg) = backfill_owner_fixture(false, "(1,'one'),(2,'two')");
        let lease = Arc::new(BackfillElection {
            owner: AtomicBool::new(true),
            epoch: std::sync::atomic::AtomicU64::new(1),
            cancelled: AtomicBool::new(false),
        });
        let fault_lease = lease.clone();
        let pool = f.pool.clone();
        let start = reorg.info.start_key.clone();
        let mut fault = BackfillBoundaryFault {
            attempts: 0,
            stop_after: 1,
            session: f.pool.acquire().unwrap(),
            after_scan: Some(Box::new(move || match case {
                0 => fault_lease.owner.store(false, Ordering::Release),
                1 => fault_lease.cancelled.store(true, Ordering::Release),
                2 => {
                    fault_lease.epoch.fetch_add(1, Ordering::AcqRel);
                }
                _ => {
                    let mut admin = astersql_meta::decode_go_history_job(
                        pool.acquire()
                            .unwrap()
                            .query("SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id=89001")
                            .unwrap()[0][0]
                            .as_bytes(),
                    )
                    .unwrap();
                    admin.state = if case == 3 {
                        JobState::Pausing
                    } else {
                        JobState::Cancelling
                    };
                    pool.acquire()
                        .unwrap()
                        .query(format!(
                            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id=89001",
                            hex(&astersql_meta::encode_go_ddl_job(&mut admin, false).unwrap())
                        ))
                        .unwrap();
                }
            })),
        };
        let error = JobWorker::new(WorkerType::AddIndex)
            .run_transactional_index_backfill(&mut fault, lease.as_ref(), &job, &mut reorg, &[81])
            .unwrap_err();
        assert!(
            error.contains("owner")
                || error.contains("cancel")
                || error.contains("changed")
                || error.contains(astersql_kv::TxnRetryableMark),
            "{error}"
        );
        assert!(backfill_owner_entries(&f).is_empty(), "case {case}");
        assert_eq!(reorg.info.start_key, start);
        assert_eq!(reorg.runtime.row_count(), 0);
        assert_eq!(
            PersistentReorgHandler::restore(&mut fault.session, &mut job)
                .unwrap()
                .info
                .start_key,
            start
        );
        // A fresh owner/session resumes from the same durable cursor.
        lease.owner.store(true, Ordering::Release);
        lease.cancelled.store(false, Ordering::Release);
        let mut active = f.queue(job.id).unwrap();
        active.state = JobState::Running;
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id=89001",
                hex(&astersql_meta::encode_go_ddl_job(&mut active, false).unwrap())
            ))
            .unwrap();
        JobWorker::new(WorkerType::AddIndex)
            .run_transactional_index_backfill(
                &mut f.pool.acquire().unwrap(),
                lease.as_ref(),
                &active,
                &mut reorg,
                &[81],
            )
            .unwrap();
        assert_eq!(backfill_owner_entries(&f).len(), 2);
    }
}
#[test]
fn normal_ddl_plan_backfill_owner_unique_conflict_retains_committed_prefix() {
    let (f, job, mut reorg) = backfill_owner_fixture(true, "(1,'same'),(2,'same')");
    let lease = backfill_election();
    let mut session = f.pool.acquire().unwrap();
    let error = JobWorker::new(WorkerType::AddIndex)
        .run_transactional_index_backfill(&mut session, &lease, &job, &mut reorg, &[81])
        .unwrap_err();
    assert!(error.contains("1062"), "{error}");
    assert_eq!(backfill_owner_entries(&f).len(), 1);
    assert_eq!(reorg.runtime.row_count(), 1);
    f.pool
        .acquire()
        .unwrap()
        .query("UPDATE test.normal_ddl_target SET payload='other' WHERE id=2")
        .unwrap();
    let resumed = astersql_ddl::reorg::PersistentReorgHandler::restore(
        &mut session,
        &mut f.queue(job.id).unwrap(),
    )
    .unwrap();
    assert_eq!(resumed.info.start_key, reorg.info.start_key);
    JobWorker::new(WorkerType::AddIndex)
        .run_transactional_index_backfill(&mut session, &lease, &job, &mut reorg, &[81])
        .unwrap();
    assert_eq!(backfill_owner_entries(&f).len(), 2);
}

fn backfill_election() -> BackfillElection {
    BackfillElection {
        owner: AtomicBool::new(true),
        epoch: std::sync::atomic::AtomicU64::new(1),
        cancelled: AtomicBool::new(false),
    }
}
#[test]
fn normal_ddl_plan_backfill_owner_mvcc_retry_regenerates_changed_row() {
    let (f, job, mut reorg) = backfill_owner_fixture(false, "(1,'old'),(2,'two')");
    let pool = f.pool.clone();
    let mut fault = BackfillBoundaryFault {
        attempts: 0,
        stop_after: 1,
        session: f.pool.acquire().unwrap(),
        after_scan: Some(Box::new(move || {
            pool.acquire()
                .unwrap()
                .query("UPDATE test.normal_ddl_target SET payload='new' WHERE id=1")
                .unwrap();
        })),
    };
    let result = JobWorker::new(WorkerType::AddIndex)
        .run_transactional_index_backfill(&mut fault, &backfill_election(), &job, &mut reorg, &[81])
        .unwrap();
    assert_eq!(result.total_added_count, 2);
    assert_eq!(
        result.total_scan_count, 2,
        "failed batch is excluded from statistics"
    );
    let entries = backfill_owner_entries(&f);
    assert_eq!(
        entries.len(),
        2,
        "abandoned snapshot must not leave old index value"
    );
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let index = table.Indices.iter().find(|idx| idx.ID == 81).unwrap();
    let mut row = std::collections::HashMap::new();
    row.insert("id".into(), Some("1".into()));
    row.insert("payload".into(), Some("new".into()));
    let expected = super::row_codec::encode_relational_index_value_row(
        &table,
        index,
        &row,
        astersql_types::scalar::StrictFlags,
        vec![astersql_types::datum::NewStringDatum("new".into())],
    )
    .unwrap();
    // The production backfiller retains restored collation data in the value;
    // the older DML convenience encoder does not. Compare its canonical key,
    // and independently decode the persisted handle from the full value.
    let actual = entries
        .iter()
        .find(|entry| entry.0 == expected.0.0)
        .expect("index must use the retry snapshot's new value");
    assert_eq!(
        astersql_tablecodec::DecodeIndexHandle(actual.0.clone(), actual.1.clone(), 1)
            .unwrap()
            .unwrap()
            .IntValue(),
        1
    );
    assert_eq!(
        fault.attempts, 3,
        "one failed batch and two committed batches"
    );
}
#[test]
fn normal_ddl_plan_backfill_owner_preserves_ingest_dxf_selection() {
    for case in 0..3 {
        let (f, mut job, mut reorg) = backfill_owner_fixture(false, "(1,'one')");
        let meta = job.reorg_meta.as_mut().unwrap();
        if case == 0 {
            meta.ReorgTp = astersql_meta_model::group_3::ReorgType::ReorgTypeIngest;
        }
        if case == 1 {
            meta.IsDistReorg = true;
        }
        if case == 2 {
            meta.ReorgTp = astersql_meta_model::group_3::ReorgType::ReorgTypeTxnMerge;
        }
        let before = job.encode(false).unwrap();
        let error = JobWorker::new(WorkerType::AddIndex)
            .run_transactional_index_backfill(
                &mut f.pool.acquire().unwrap(),
                &backfill_election(),
                &job,
                &mut reorg,
                &[81],
            )
            .unwrap_err();
        assert!(error.contains("ingest/DXF"), "{error}");
        assert_eq!(job.encode(false).unwrap(), before);
        assert!(backfill_owner_entries(&f).is_empty());
    }
}
#[test]
fn normal_ddl_plan_backfill_owner_public_manager_reacquisition_changes_tenure() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let cancel = astersql_owner::manager::Context::new();
        let manager = astersql_owner::mock::NewMockManager(
            cancel.clone(),
            "backfill-epoch",
            None,
            format!("/ddl/backfill-epoch/{}", std::process::id()),
        );
        manager.CampaignOwner(&[]).await.unwrap();
        assert!(manager.IsOwner());
        let old = manager.OwnerEpoch();
        assert!(old > 0);
        manager.RetireOwner().await;
        manager.CampaignCancel().await;
        manager.CampaignOwner(&[]).await.unwrap();
        assert!(manager.IsOwner());
        assert!(manager.OwnerEpoch() > old);
        let lease = super::system_session::DdlOwnerLease {
            owner: manager.clone(),
            cancellation: Arc::new(astersql_session_syssession::CancellationToken::default()),
        };
        assert_eq!(lease.owner_epoch(), manager.OwnerEpoch());
        manager.Close().await;
    });
}

#[test]
fn normal_ddl_plan_backfill_owner_handoff_recovers_committed_prefix_on_new_pool() {
    let (f, mut job, mut reorg) = backfill_owner_fixture(true, "(1,'one'),(2,'two'),(3,'three')");
    let lease = Arc::new(backfill_election());
    let retiring = lease.clone();
    let mut fault = BackfillBoundaryFault {
        session: f.pool.acquire().unwrap(),
        attempts: 0,
        stop_after: 2,
        after_scan: Some(Box::new(move || {
            retiring.owner.store(false, Ordering::Release);
        })),
    };
    let error = JobWorker::new(WorkerType::AddIndex)
        .run_transactional_index_backfill(&mut fault, lease.as_ref(), &job, &mut reorg, &[81])
        .unwrap_err();
    assert!(error.contains("owner"), "{error}");
    assert_eq!(backfill_owner_entries(&f).len(), 1);
    assert_eq!(reorg.runtime.row_count(), 1);
    let expected = reorg.info.start_key.clone();
    drop(fault);
    let next_pool = SystemSessionPool::new(f.domain.clone());
    let mut session = next_pool.acquire().unwrap();
    let mut resumed =
        astersql_ddl::reorg::PersistentReorgHandler::restore(&mut session, &mut job).unwrap();
    assert_eq!(resumed.info.start_key, expected);
    let result = JobWorker::new(WorkerType::AddIndex)
        .run_transactional_index_backfill(
            &mut session,
            &backfill_election(),
            &job,
            &mut resumed,
            &[81],
        )
        .unwrap();
    assert_eq!(result.total_scan_count, 2);
    assert_eq!(result.total_added_count, 2);
    assert_eq!(backfill_owner_entries(&f).len(), 3);
    assert_eq!(resumed.info.start_key, resumed.info.end_key);
    drop(session);
    next_pool.close();
}

static NORMAL_SCHEMA_RUNTIME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn normal_ddl_plan_schema_runtime_reads_user_kv_and_closes_loops() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{MemoryEtcdClient, NewEtcdSyncer};
    let f = Fixture::new();
    struct RestoreMdl(bool);
    impl Drop for RestoreMdl {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _restore = RestoreMdl(astersql_sessionctx_vardef::IsMDLEnabled());
    astersql_sessionctx_vardef::SetEnableMDL(true);

    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'existing row')")
        .unwrap();
    let client = Arc::new(MemoryEtcdClient::default());
    let runtime = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        NewEtcdSyncer(client.clone(), "normal-schema"),
        std::time::Duration::from_millis(100),
    )
    .unwrap();
    f.insert(99344, JobState::Queueing);
    runtime.start().unwrap();
    runtime.start().unwrap();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Comment = "direct KV user metadata".into();
    let next = version(&f) + 20;
    runtime.pool.acquire().unwrap().query(format!(
        "INSERT INTO mysql.tidb_mdl_info (job_id, version, table_ids) VALUES (99345,{next},'{}')", f.table
    )).unwrap();
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", f.table).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Set(
        astersql_meta::transaction_meta_string_key(b"SchemaVersionKey"),
        next.to_string().into_bytes(),
    )
    .unwrap();
    txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{next}").as_bytes()),
        serde_json::to_vec(&serde_json::json!({"version": next, "type": 17, "schema_id": f.db, "table_id": f.table})).unwrap()).unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while f
        .domain
        .table_by_name("test", "normal_ddl_target")
        .unwrap()
        .Comment
        != table.Comment
        || runtime.validator.snapshot().latest_schema_ver != next
        || !runtime.refresher.is_running()
        || runtime.refresher.current_min_job_id() != 99344
    {
        assert!(
            std::time::Instant::now() < deadline,
            "user metadata/validator/min-job runtime did not refresh"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(f.domain.info_schema().SchemaMetaVersion(), next);
    assert_eq!(
        runtime
            .pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=1")
            .unwrap()[0][0],
        "existing row"
    );
    assert!(
        !runtime
            .syncer
            .skipMDLCheck(&[f.table].into_iter().collect())
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !runtime.syncer.mdlCheckContains(f.table) {
        assert!(
            std::time::Instant::now() < deadline,
            "SyncLoop did not read user-table MDL row"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(runtime.syncer.mdlCheckSnapshot().1[&99345].Ver, next);
    assert!(
        astersql_sessionctx_vardef::IsMDLEnabled(),
        "MDL publication regression requires enabled MDL"
    );
    {
        use astersql_ddl_schemaver::{Context, DDLAllSchemaVersionsByJob, EtcdClient};
        loop {
            let publication = client
                .Get(
                    &Context::Background(),
                    &format!("{DDLAllSchemaVersionsByJob}/99345/normal-schema"),
                    false,
                )
                .unwrap();
            if publication
                .Kvs
                .first()
                .is_some_and(|kv| kv.Value == next.to_string().as_bytes())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "MDLCheckLoop did not publish user-table version"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    runtime.close().unwrap();
    runtime.close().unwrap();
    assert!(!runtime.refresher.is_running());
    assert!(runtime.pool.acquire().is_err());
    assert!(runtime.start().is_err());
}

#[test]
fn normal_ddl_plan_schema_runtime_service_owns_domain_loops() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_domain::domain::{DdlService, StartMode};
    let f = Fixture::new();
    let runtime = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        astersql_ddl_schemaver::NewEtcdSyncer(
            Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default()),
            "normal-lifecycle",
        ),
        std::time::Duration::from_millis(100),
    )
    .unwrap();
    let cancel = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancel.clone(),
        "schema-only",
        None,
        format!("/schema-only/{}", f.db),
    );
    let service = Arc::new(
        super::normal_ddl_service::NormalDdlService::new(
            owner,
            Arc::new(tokio::runtime::Runtime::new().unwrap()),
            cancel,
            runtime.pool.clone(),
            runtime.clone(),
            Arc::new(|| Ok(Box::new(executor()))),
            Arc::new(|_| Ok(())),
            Arc::new(|| {}),
            false,
        )
        .with_schema_runtime(runtime.clone()),
    );
    f.domain.set_ddl(service.clone());
    assert!(service.manages_schema_sync());
    f.domain.start(StartMode::Normal).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while runtime.active_loop_count() != 4 || !runtime.refresher.is_running() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !f.domain
            .background_worker_names()
            .iter()
            .any(|name| name == "schema-reload"),
        "legacy Domain reload loop still runs"
    );
    f.domain.close();
    assert_eq!(runtime.active_loop_count(), 0);
    assert!(!runtime.refresher.is_running());
    assert!(!runtime.validator.snapshot().is_started);
    assert!(runtime.pool.acquire().is_err());
    service.stop().unwrap();
    assert!(runtime.start().is_err());
}

#[test]
fn normal_ddl_plan_schema_runtime_constructor_failures_clean_protocol() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{
        Context, DDLAllSchemaVersions, EtcdClient, MemoryEtcdClient, NewEtcdSyncer,
    };
    let f = Fixture::new();
    let client = Arc::new(MemoryEtcdClient::default());
    let protocol = NewEtcdSyncer(client.clone(), "constructor-failure");
    assert!(
        super::session_factory::prepare_normal_schema_runtime(
            &f.domain,
            protocol.clone(),
            std::time::Duration::ZERO
        )
        .is_err()
    );
    client.FailPuts(1);
    assert!(
        super::session_factory::prepare_normal_schema_runtime(
            &f.domain,
            protocol.clone(),
            std::time::Duration::from_millis(100)
        )
        .is_err()
    );
    assert!(
        client
            .Get(
                &Context::Background(),
                &format!("{DDLAllSchemaVersions}/constructor-failure"),
                false
            )
            .unwrap()
            .Kvs
            .is_empty()
    );
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        hash(b"DBs", format!("DB:{}", f.db).as_bytes()),
        b"invalid Go database JSON".to_vec(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    assert!(
        super::session_factory::prepare_normal_schema_runtime(
            &f.domain,
            protocol,
            std::time::Duration::from_millis(100)
        )
        .is_err()
    );
    assert!(
        client
            .Get(
                &Context::Background(),
                &format!("{DDLAllSchemaVersions}/constructor-failure"),
                false
            )
            .unwrap()
            .Kvs
            .is_empty()
    );
}

#[test]
fn normal_ddl_plan_schema_runtime_cancel_during_version_publication() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    struct RestoreMdl(bool);
    impl Drop for RestoreMdl {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _restore = RestoreMdl(astersql_sessionctx_vardef::IsMDLEnabled());
    astersql_sessionctx_vardef::SetEnableMDL(false);
    let f = Fixture::new();
    let client = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    let runtime = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        astersql_ddl_schemaver::NewEtcdSyncer(client.clone(), "normal-cancel"),
        std::time::Duration::from_millis(100),
    )
    .unwrap();
    runtime.start().unwrap();
    client.FailPuts(usize::MAX);
    let next = version(&f) + 20;
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        astersql_meta::transaction_meta_string_key(b"SchemaVersionKey"),
        next.to_string().into_bytes(),
    )
    .unwrap();
    txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{next}").as_bytes()),
        serde_json::to_vec(&serde_json::json!({"version": next, "type": 17, "schema_id": f.db, "table_id": f.table})).unwrap()).unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while f.domain.info_schema().SchemaMetaVersion() != next {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let closing = runtime.clone();
    let join = std::thread::spawn(move || {
        tx.send(closing.close()).unwrap();
    });
    let closed = rx.recv_timeout(std::time::Duration::from_secs(2));
    // Always release the fault and join before asserting, including red runs.
    client.FailPuts(0);
    join.join().unwrap();
    assert!(
        closed.is_ok(),
        "closing schema runtime could not cancel version publication"
    );
    closed.unwrap().unwrap();
    assert_eq!(runtime.active_loop_count(), 0);
    assert!(!runtime.refresher.is_running());
}

fn schema_barrier_owner() -> (
    Arc<tokio::runtime::Runtime>,
    Arc<dyn astersql_owner::manager::Manager>,
    astersql_owner::manager::Context,
) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let cancel = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancel.clone(),
        "schema-barrier-owner",
        None,
        format!("/ddl/schema-barrier/{}", std::process::id()),
    );
    runtime.block_on(owner.CampaignOwner(&[])).unwrap();
    (runtime, owner, cancel)
}
#[test]
fn normal_ddl_plan_schema_barrier_unsynced_nodes_prevent_history_and_restart_recovers() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{Context, MemoryEtcdClient, NewEtcdSyncer};
    let f = Fixture::new();
    astersql_sessionctx_vardef::SetEnableMDL(false);
    astersql_ddl_schemaver::SetMDLEnabled(false);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'barrier existing row')")
        .unwrap();
    let client = Arc::new(MemoryEtcdClient::default());
    let first = NewEtcdSyncer(client.clone(), "barrier-first");
    let second = NewEtcdSyncer(client.clone(), "barrier-second");
    second.Init(Context::Background()).unwrap();
    let schema = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        first.clone(),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    let lease = Lease(AtomicBool::new(true));
    let mut e = NormalDdlExecutor {
        barrier: schema.schema_barrier(
            &f.domain,
            owner.clone(),
            cancel.clone(),
            "schema-barrier-owner".into(),
            Some(client.clone()),
        ),
        policy: Policy(None),
        sequence: Arc::new(AtomicI64::new(0)),
    };
    let parent = e.barrier.context.clone();
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_millis(100));
    f.insert(99501, JobState::Queueing);
    assert!(
        scheduler()
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
            .is_err(),
        "unsynchronized follower must hold the committed step"
    );
    let committed = f.queue(99501).unwrap();
    assert_eq!(committed.state, JobState::Done);
    assert!(committed.last_schema_version > 0);
    assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
    assert!(f.reader().get_history_ddl_job(99501).unwrap().is_none());
    // A fresh scheduler and executor must recover the committed version before
    // a terminal step can delete the queue or add history.
    owner_runtime.block_on(owner.Close());
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    e.barrier = schema.schema_barrier(
        &f.domain,
        owner.clone(),
        cancel,
        "schema-barrier-owner".into(),
        Some(client.clone()),
    );
    let parent = e.barrier.context.clone();
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_millis(100));
    assert!(
        scheduler()
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
            .is_err()
    );
    assert!(f.queue(99501).is_some());
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_secs(2));
    first
        .UpdateSelfVersion(Context::Background(), 0, committed.last_schema_version)
        .unwrap();
    second
        .UpdateSelfVersion(Context::Background(), 0, committed.last_schema_version)
        .unwrap();
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert!(f.queue(99501).is_none());
    assert_eq!(
        f.reader()
            .get_history_ddl_job(99501)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=1")
            .unwrap()[0][0],
        "barrier existing row"
    );
    // No-op requests must not wait on an unrelated stale follower or allocate
    // another schema version. They still finish through the durable history path.
    let before = version(&f);
    second
        .UpdateSelfVersion(Context::Background(), 0, 0)
        .unwrap();
    f.insert(99504, JobState::Queueing);
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_millis(100));
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert_eq!(f.queue(99504).unwrap().last_schema_version, 0);
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
            .unwrap(),
        1
    );
    assert_eq!(version(&f), before);
    assert_eq!(
        f.reader()
            .get_history_ddl_job(99504)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    owner_runtime.block_on(owner.Close());
    schema.close().unwrap();
    second.Close();
}

struct BarrierMdlRestore(bool, bool);
impl Drop for BarrierMdlRestore {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::SetEnableMDL(self.0);
        astersql_ddl_schemaver::SetMDLEnabled(self.1);
    }
}
fn barrier_set_mdl(enabled: bool) -> BarrierMdlRestore {
    let restore = BarrierMdlRestore(
        astersql_sessionctx_vardef::IsMDLEnabled(),
        astersql_ddl_schemaver::IsMDLEnabled(),
    );
    astersql_sessionctx_vardef::SetEnableMDL(enabled);
    astersql_ddl_schemaver::SetMDLEnabled(enabled);
    restore
}
fn barrier_wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "schema barrier observation timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn normal_ddl_plan_schema_barrier_mdl_old_transaction_and_second_node_hold_history() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{
        Context, DDLAllSchemaVersionsByJob, EtcdClient, MemoryEtcdClient, NewEtcdSyncer,
    };
    use astersql_domain_serverinfo as info;
    let f = Fixture::new();
    let _restore = barrier_set_mdl(true);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'mdl existing row')")
        .unwrap();
    let client = Arc::new(MemoryEtcdClient::default());
    let first = NewEtcdSyncer(client.clone(), "mdl-first");
    let second = NewEtcdSyncer(client.clone(), "mdl-second");
    second.Init(Context::Background()).unwrap();
    let infos = Arc::new(info::MemoryEtcdClient::default());
    for (id, port) in [("mdl-first", 4000), ("mdl-second", 4001)] {
        let mut server = info::ServerInfo {
            StaticInfo: info::StaticInfo {
                ID: id.into(),
                IP: "127.0.0.1".into(),
                Port: port,
                StartTimestamp: 1,
                ServerIDGetter: Some(Arc::new(|| 1)),
                ..Default::default()
            },
            ..Default::default()
        };
        info::EtcdClient::Put(
            infos.as_ref(),
            &info::Context::Background(),
            &info::serverInfoKeyPath(id),
            server.Marshal().unwrap(),
            None,
        )
        .unwrap();
    }
    first.SetServerInfoSyncer(Some(Arc::from(info::NewSyncer(
        "mdl-first".into(),
        Arc::new(|| 1),
        Some(infos),
        Arc::new(info::NoopMinStartTSReporter),
    ))));
    let schema = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        first.clone(),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    schema.start().unwrap();
    let old = super::ConcreteSession::new(f.domain.clone());
    // Forward the actual SQL transaction signal through Domain's connection
    // registry hook; never seed synthetic lock versions or a completion flag.
    let manager: Arc<dyn astersql_session_sessmgr::InfoSchemaCoordinator> =
        Arc::new(NormalConnectionCoordinator {
            mdl: old.transaction_mdl(),
            kills: std::sync::atomic::AtomicUsize::new(0),
        });
    f.domain.set_schema_coordinator(Arc::downgrade(&manager));
    let user_query = |sql: &str| {
        let mut rows = Vec::new();
        for mut set in old.execute(sql).unwrap() {
            while let Some(row) = set.next_row().unwrap() {
                rows.push(row);
            }
            set.close().unwrap();
        }
        rows
    };
    user_query("BEGIN");
    assert_eq!(
        user_query("SELECT payload FROM test.normal_ddl_target WHERE id=1")[0][0],
        "mdl existing row"
    );
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    f.insert(99502, JobState::Queueing);
    let mut e = NormalDdlExecutor {
        barrier: schema.schema_barrier(
            &f.domain,
            owner.clone(),
            cancel,
            "schema-barrier-owner".into(),
            Some(client.clone()),
        ),
        policy: Policy(Some("schema-barrier-owner".into())),
        sequence: Arc::new(AtomicI64::new(0)),
    };
    e.barrier.context = e
        .barrier
        .context
        .WithTimeout(std::time::Duration::from_secs(15));
    let pool = f.pool.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = scheduler().schedule_persisted(
            &mut pool.acquire().unwrap(),
            &Lease(AtomicBool::new(true)),
            &mut e,
            0,
        );
        tx.send((result, e)).unwrap();
    });
    barrier_wait_for(|| {
        f.queue(99502)
            .is_some_and(|job| job.state == JobState::Done)
    });
    let committed = f.queue(99502).unwrap();
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT version,table_ids,owner_id FROM mysql.tidb_mdl_info WHERE job_id=99502")
        .unwrap();
    assert_eq!(
        rows[0],
        vec![
            committed.last_schema_version.to_string(),
            f.table.to_string(),
            "schema-barrier-owner".into()
        ]
    );
    barrier_wait_for(|| schema.syncer.mdlCheckContains(f.table));
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    );
    assert!(
        client
            .Get(
                &Context::Background(),
                &format!("{DDLAllSchemaVersionsByJob}/99502/mdl-first"),
                false
            )
            .unwrap()
            .Kvs
            .is_empty(),
        "old real SQL transaction must prevent local MDL publication"
    );
    assert!(f.reader().get_history_ddl_job(99502).unwrap().is_none());
    user_query("ROLLBACK");
    barrier_wait_for(|| {
        client
            .Get(
                &Context::Background(),
                &format!("{DDLAllSchemaVersionsByJob}/99502/mdl-first"),
                false,
            )
            .unwrap()
            .Kvs
            .first()
            .is_some_and(|kv| kv.Value == committed.last_schema_version.to_string().as_bytes())
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "second live node still has not acknowledged"
    );
    second
        .UpdateSelfVersion(Context::Background(), 99502, committed.last_schema_version)
        .unwrap();
    let (result, mut e) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(result.unwrap(), 1);
    worker.join().unwrap();
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT job_id FROM mysql.tidb_mdl_info WHERE job_id=99502")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        scheduler()
            .schedule_persisted(
                &mut f.pool.acquire().unwrap(),
                &Lease(AtomicBool::new(true)),
                &mut e,
                0
            )
            .unwrap(),
        1
    );
    assert!(f.queue(99502).is_none());
    assert_eq!(
        f.reader()
            .get_history_ddl_job(99502)
            .unwrap()
            .unwrap()
            .state,
        JobState::Synced
    );
    assert!(
        client
            .Get(
                &Context::Background(),
                &format!("{DDLAllSchemaVersionsByJob}/99502/"),
                true
            )
            .unwrap()
            .Kvs
            .is_empty()
    );
    owner_runtime.block_on(owner.Close());
    schema.close().unwrap();
    second.Close();
}

#[test]
fn normal_ddl_plan_schema_barrier_owner_loss_close_and_deadline_interrupt_wait() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{Context, MemoryEtcdClient, NewEtcdSyncer};
    for scenario in 0..3 {
        let f = Fixture::new();
        let _restore = barrier_set_mdl(false);
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES (1,'cancel existing row')")
            .unwrap();
        let client = Arc::new(MemoryEtcdClient::default());
        let follower = NewEtcdSyncer(client.clone(), "cancel-follower");
        follower.Init(Context::Background()).unwrap();
        let schema = super::session_factory::prepare_normal_schema_runtime(
            &f.domain,
            NewEtcdSyncer(client.clone(), "cancel-owner"),
            std::time::Duration::from_millis(50),
        )
        .unwrap();
        let (owner_runtime, owner, cancel) = schema_barrier_owner();
        let mut e = NormalDdlExecutor {
            barrier: schema.schema_barrier(
                &f.domain,
                owner.clone(),
                cancel,
                "schema-barrier-owner".into(),
                Some(client),
            ),
            policy: Policy(None),
            sequence: Arc::new(AtomicI64::new(0)),
        };
        e.barrier.context = e
            .barrier
            .context
            .WithTimeout(std::time::Duration::from_millis(if scenario == 2 {
                500
            } else {
                5000
            }));
        f.insert(99503, JobState::Queueing);
        let pool = f.pool.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(scheduler().schedule_persisted(
                &mut pool.acquire().unwrap(),
                &Lease(AtomicBool::new(true)),
                &mut e,
                0,
            ))
            .unwrap();
        });
        barrier_wait_for(|| {
            f.queue(99503)
                .is_some_and(|job| job.state == JobState::Done)
        });
        if scenario == 0 {
            owner_runtime.block_on(owner.RetireOwner());
        }
        if scenario == 1 {
            schema.close().unwrap();
        }
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        worker.join().unwrap();
        assert!(f.queue(99503).is_some());
        assert!(f.reader().get_history_ddl_job(99503).unwrap().is_none());
        assert_eq!(mode(&f), astersql_meta_model::TableMode::TableModeImport);
        owner_runtime.block_on(owner.Close());
        schema.close().unwrap();
        follower.Close();
    }
}

#[test]
fn normal_ddl_plan_schema_barrier_mdl_cleanup_preserves_other_owner_and_system_exception() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    let f = Fixture::new();
    let _restore = barrier_set_mdl(true);
    let client = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    let schema = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        astersql_ddl_schemaver::NewEtcdSyncer(client.clone(), "cleanup"),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    let mut barrier = schema.schema_barrier(
        &f.domain,
        owner.clone(),
        cancel,
        "current-owner".into(),
        Some(client),
    );
    let session = f.pool.acquire().unwrap();
    session.query(format!("INSERT INTO mysql.tidb_mdl_info (job_id,version,table_ids,owner_id) VALUES (99505,{},'{}','previous-owner')",version(&f),f.table)).unwrap();
    let mut job = Job {
        id: 99505,
        schema_name: "test".into(),
        table_id: f.table,
        state: JobState::Synced,
        ..Default::default()
    };
    barrier
        .wait(&job, 0, &Lease(AtomicBool::new(true)))
        .unwrap();
    assert_eq!(
        session
            .query("SELECT owner_id FROM mysql.tidb_mdl_info WHERE job_id=99505")
            .unwrap()[0][0],
        "previous-owner"
    );
    job.schema_name = "mysql".into();
    barrier
        .wait(&job, 0, &Lease(AtomicBool::new(true)))
        .unwrap();
    assert!(
        session
            .query("SELECT job_id FROM mysql.tidb_mdl_info WHERE job_id=99505")
            .unwrap()
            .is_empty()
    );
    owner_runtime.block_on(owner.Close());
    schema.close().unwrap();
}

#[test]
fn normal_ddl_plan_schema_barrier_service_stop_joins_inflight_worker() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    use astersql_ddl_schemaver::{Context, MemoryEtcdClient, NewEtcdSyncer};
    use astersql_domain::domain::{DdlService, StartMode};
    let f = Fixture::new();
    let _restore = barrier_set_mdl(false);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'service existing row')")
        .unwrap();
    let client = Arc::new(MemoryEtcdClient::default());
    let follower = NewEtcdSyncer(client.clone(), "service-follower");
    follower.Init(Context::Background()).unwrap();
    let schema = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        NewEtcdSyncer(client.clone(), "service-owner"),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let (owner_runtime, owner, cancellation) = schema_barrier_owner();
    let make_schema = schema.clone();
    let domain = Arc::downgrade(&f.domain);
    let make_owner = owner.clone();
    let make_cancel = cancellation.clone();
    let service = super::normal_ddl_service::NormalDdlService::new(
        owner.clone(),
        owner_runtime.clone(),
        cancellation,
        f.pool.clone(),
        schema.clone(),
        Arc::new(move || {
            Ok(Box::new(NormalDdlExecutor {
                barrier: make_schema.schema_barrier(
                    &domain.upgrade().ok_or("normal Domain closed")?,
                    make_owner.clone(),
                    make_cancel.clone(),
                    "schema-barrier-owner".into(),
                    Some(client.clone()),
                ),
                policy: Policy(None),
                sequence: Arc::new(AtomicI64::new(0)),
            }))
        }),
        Arc::new(|_| Err("unused submit interface".into())),
        Arc::new(|| {}),
        true,
    )
    .with_schema_runtime(schema.clone());
    f.insert(99506, JobState::Queueing);
    service.start(StartMode::Normal).unwrap();
    barrier_wait_for(|| {
        f.queue(99506)
            .is_some_and(|job| job.state == JobState::Done)
    });
    assert!(f.reader().get_history_ddl_job(99506).unwrap().is_none());
    let start = std::time::Instant::now();
    service.stop().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    service.stop().unwrap();
    assert_eq!(schema.active_loop_count(), 0);
    assert!(!owner.IsOwner());
    assert!(f.queue(99506).is_some());
    assert!(f.reader().get_history_ddl_job(99506).unwrap().is_none());
    follower.Close();
}

#[test]
fn normal_ddl_plan_schema_barrier_publication_deadline_matches_go_mdl_branch() {
    let _serial = NORMAL_SCHEMA_RUNTIME_TEST_LOCK.lock().unwrap();
    for mdl in [false, true] {
        let f = Fixture::new();
        let _restore = barrier_set_mdl(mdl);
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES (1,'publication existing row')")
            .unwrap();
        let client = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
        let schema = super::session_factory::prepare_normal_schema_runtime(
            &f.domain,
            astersql_ddl_schemaver::NewEtcdSyncer(client.clone(), "publication"),
            std::time::Duration::from_millis(50),
        )
        .unwrap();
        let (owner_runtime, owner, cancel) = schema_barrier_owner();
        let mut e = NormalDdlExecutor {
            barrier: schema.schema_barrier(
                &f.domain,
                owner.clone(),
                cancel,
                "schema-barrier-owner".into(),
                Some(client.clone()),
            ),
            policy: Policy(mdl.then(|| "schema-barrier-owner".into())),
            sequence: Arc::new(AtomicI64::new(0)),
        };
        f.insert(99507, JobState::Queueing);
        e.barrier.context = e
            .barrier
            .context
            .WithTimeout(std::time::Duration::from_millis(100));
        client.FailPuts(usize::MAX);
        let result = scheduler().schedule_persisted(
            &mut f.pool.acquire().unwrap(),
            &Lease(AtomicBool::new(true)),
            &mut e,
            0,
        );
        client.FailPuts(0);
        assert_eq!(
            result.is_err(),
            mdl,
            "Go propagates MDL publication errors but accepts non-MDL publication deadline expiration: {result:?}"
        );
        assert_eq!(f.queue(99507).unwrap().state, JobState::Done);
        assert!(f.reader().get_history_ddl_job(99507).unwrap().is_none());
        owner_runtime.block_on(owner.Close());
        schema.close().unwrap();
    }
}
