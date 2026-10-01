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
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(h: &[u8], f: &[u8]) -> astersql_kv::Key {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    astersql_kv::Key(EncodeBytes(
        EncodeUint(EncodeBytes(vec![b'm'], h), b'h' as u64),
        f,
    ))
}
struct Fixture {
    domain: Arc<astersql_domain::Domain>,
    pool: Arc<SystemSessionPool>,
    db: i64,
    table: i64,
}
impl Fixture {
    fn new() -> Self {
        let (domain, _) = CreateAnalyzeSession().unwrap();
        domain.set_global_system_variable("tidb_cdc_write_source", "9");
        let pool = SystemSessionPool::new(domain.clone());
        pool.acquire()
            .unwrap()
            .query("CREATE TABLE test.normal_ddl_target (id int primary key, payload varchar(40))")
            .unwrap();
        let db = domain
            .info_schema()
            .AllSchemas()
            .into_iter()
            .find(|s| s.name.lower == "test")
            .unwrap()
            .id;
        let table = domain
            .table_by_name("test", "normal_ddl_target")
            .unwrap()
            .ID;
        // Seed non-empty, complete Go table metadata in the actual MVCC Store.
        let mut txn = domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        let info = domain.table_by_name("test", "normal_ddl_target").unwrap();
        txn.Set(
            hash(
                format!("DB:{db}").as_bytes(),
                format!("Table:{table}").as_bytes(),
            ),
            astersql_meta_model::EncodeTableInfo(&info).unwrap(),
        )
        .unwrap();
        let dbinfo = astersql_meta_model::DBInfo {
            ID: db,
            Name: astersql_meta_model::ast::NewCIStr("test"),
            State: astersql_meta_model::SchemaState::Public,
            ..Default::default()
        };
        txn.Set(
            hash(b"DBs", format!("DB:{db}").as_bytes()),
            astersql_meta_model::EncodeDBInfo(&dbinfo).unwrap(),
        )
        .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        Self {
            domain,
            pool,
            db,
            table,
        }
    }
    fn insert(&self, id: i64, state: JobState) {
        use astersql_ddl_jobsubmit::{
            AlterTableModeTarget, SessionVariables, TableMode, build_alter_table_mode_job,
            table_mode_args,
        };
        let (job, args, noop) = build_alter_table_mode_job(
            SessionVariables {
                cdc_write_source: 41,
                sql_mode: 7,
            },
            AlterTableModeTarget {
                current_mode: TableMode::Normal,
                target_mode: TableMode::Import,
                schema_id: self.db,
                table_id: self.table,
                schema_name: "test".into(),
                table_name: "normal_ddl_target".into(),
            },
        )
        .unwrap();
        assert!(!noop);
        let mut built = job.unwrap();
        built.id = id;
        let mut job = Job::decode(&built.encode(&table_mode_args(args.unwrap()))).unwrap();
        job.state = state;
        self.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES ({id},0,'{}','{}',X'{}',75,0)",self.db,self.table,hex(&job.encode(false).unwrap()))).unwrap();
    }
    fn queue(&self, id: i64) -> Option<Job> {
        let rows = self
            .pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id={id}"
            ))
            .unwrap();
        rows.first()
            .map(|r| astersql_meta::decode_go_history_job(r[0].as_bytes()).unwrap())
    }
    fn reader(&self) -> astersql_meta::SnapshotReader {
        let snapshot = self
            .domain
            .storage_handle()
            .with_storage(|s| {
                let v = s.CurrentVersion("global")?;
                Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
            })
            .unwrap();
        astersql_meta::SnapshotReader::new(snapshot)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.pool.close();
        self.domain.close();
    }
}
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
