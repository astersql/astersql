// Copyright 2026 AsterSQL.

use super::{CreateAnalyzeSession, system_session::SystemSessionPool};
use astersql_ddl::job_scheduler::JobScheduler;
use astersql_ddl::job_worker::{
    DurableJobExecutor, DurableJobSession, DurableJobStep, JobLease, JobWorker, WorkerType,
};
use astersql_meta_model::group_3::{Job, JobState, JobVersion};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn scheduler() -> JobScheduler {
    JobScheduler::new(
        JobWorker::new(WorkerType::General),
        JobWorker::new(WorkerType::AddIndex),
    )
}
struct Lease {
    owner: AtomicBool,
    cancelled: AtomicBool,
}
impl Default for Lease {
    fn default() -> Self {
        Self {
            owner: AtomicBool::new(true),
            cancelled: AtomicBool::new(false),
        }
    }
}
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        self.owner.load(Ordering::Acquire)
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

// This transaction callback exercises only the task-7 boundary: retain the
// complete Job and write a real Go-layout meta key in the same SQL transaction.
// No TableMode execution, history or schema synchronization is simulated.
struct TransactionStep {
    lease: Arc<Lease>,
    retire: bool,
    cancel: bool,
    fail: bool,
    fail_sync: bool,
    seen: Vec<JobState>,
    recovered: Vec<JobState>,
    concurrently_pause: Option<Arc<SystemSessionPool>>,
    overlapping_pause: Option<Arc<SystemSessionPool>>,
}
impl TransactionStep {
    fn new(lease: Arc<Lease>) -> Self {
        Self {
            lease,
            retire: false,
            cancel: false,
            fail: false,
            fail_sync: false,
            seen: vec![],
            recovered: vec![],
            concurrently_pause: None,
            overlapping_pause: None,
        }
    }
}
impl DurableJobExecutor for TransactionStep {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, job: &Job) -> Result<bool, String> {
        if let Some(pool) = self.concurrently_pause.take() {
            let mut changed = read(&pool, job.id);
            changed.state = JobState::Paused;
            let bytes = changed.encode(false).unwrap();
            pool.acquire()?.query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta = X'{}' WHERE job_id = {}",
                hex(&bytes),
                job.id
            ))?;
        }
        Ok(job.state != JobState::Paused)
    }
    fn recover(&mut self, job: &Job, _: &dyn JobLease) -> Result<(), String> {
        self.recovered.push(job.state);
        Ok(())
    }
    fn step(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &mut Job,
    ) -> Result<DurableJobStep, String> {
        self.seen.push(job.state);
        session.with_transaction(Box::new(|txn| {
            use astersql_util_codec::{EncodeBytes, EncodeUint};
            let key = astersql_kv::Key(EncodeUint(
                EncodeBytes(vec![b'm'], b"NextGlobalID"),
                u64::from(b's'),
            ));
            let value = astersql_kv::IncInt64(txn, &key, 1).map_err(|e| e.to_string())?;
            Ok(value.to_be_bytes().to_vec())
        }))?;
        // Queueing/None are claimed; cancellation is delivered intact rather
        // than being mistaken for a fresh Running job.
        if matches!(job.state, JobState::Queueing | JobState::None) {
            job.state = JobState::Running;
        }
        job.error_count += 1;
        if let Some(pool) = self.overlapping_pause.take() {
            let mut changed = read(&pool, job.id);
            changed.state = JobState::Paused;
            let bytes = changed.encode(false).unwrap();
            pool.acquire()?.query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta = X'{}' WHERE job_id = {}",
                hex(&bytes),
                job.id
            ))?;
        }

        if self.retire {
            self.lease.owner.store(false, Ordering::Release);
        }
        if self.cancel {
            self.lease.cancelled.store(true, Ordering::Release);
        }
        if self.fail {
            return Err("injected step failure".into());
        }
        Ok(DurableJobStep {
            schema_version: 0,
            update_raw_args: false,
            removed: false,
        })
    }
    fn wait_synced(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
        if self.fail_sync {
            Err("injected sync interruption".into())
        } else {
            Ok(())
        }
    }
}

fn insert(pool: &SystemSessionPool, id: i64, state: JobState) {
    let lease = pool.acquire().unwrap();
    use astersql_ddl_jobsubmit::{
        AlterTableModeTarget, SessionVariables, TableMode, build_alter_table_mode_job,
        table_mode_args,
    };
    let (built, args, noop) = build_alter_table_mode_job(
        SessionVariables {
            cdc_write_source: 41,
            sql_mode: 7,
        },
        AlterTableModeTarget {
            current_mode: TableMode::Normal,
            target_mode: TableMode::Import,
            schema_id: 1,
            table_id: 2,
            schema_name: "test".into(),
            table_name: "target".into(),
        },
    )
    .unwrap();
    assert!(!noop);
    let mut submitted = built.unwrap();
    submitted.id = id;
    let mut job = Job::decode(&submitted.encode(&table_mode_args(args.unwrap()))).unwrap();
    job.state = state;
    assert_eq!(job.version, JobVersion::V2);
    let encoded = hex(&job.encode(false).unwrap());
    lease.query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES ({id}, 0, '1', '2', X'{encoded}', 75, 0)")).unwrap();
}
fn read(pool: &SystemSessionPool, id: i64) -> Job {
    let rows = pool
        .acquire()
        .unwrap()
        .query(format!(
            "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id = {id}"
        ))
        .unwrap();
    Job::decode(rows[0][0].as_bytes()).unwrap()
}
fn global_id(domain: &astersql_domain::Domain) -> i64 {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let key = astersql_kv::Key(EncodeUint(
        EncodeBytes(vec![b'm'], b"NextGlobalID"),
        u64::from(b's'),
    ));
    let value =
        astersql_kv::GetInt64(&astersql_kv::Context::default(), txn.as_ref(), &key).unwrap();
    txn.Rollback().unwrap();
    value
}

#[test]
fn crossks_align_durable_scheduler_reloads_sql_job_after_restart() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(domain.clone());
    insert(&pool, 79001, JobState::Queueing);
    let original = read(&pool, 79001);
    let before = global_id(&domain);
    let owner = Arc::new(Lease::default());
    let mut executor = TransactionStep::new(owner.clone());
    let mut lease = pool.acquire().unwrap();
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0)
            .unwrap(),
        1
    );
    let claimed = read(&pool, 79001);
    assert_eq!(claimed.state, JobState::Running);
    assert_eq!(claimed.raw_args, original.raw_args);
    assert_eq!(claimed.cdc_write_source, 41);
    assert_eq!(global_id(&domain), before + 1);
    // A new scheduler recovers the Running wire Job from SQL rather than RAM.
    let mut restarted = TransactionStep::new(owner.clone());
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut restarted, 0)
            .unwrap(),
        1
    );
    assert_eq!(restarted.recovered, vec![JobState::Running]);
    assert_eq!(read(&pool, 79001).error_count, 2);
    drop(lease);
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_durable_scheduler_owner_loss_cancel_and_error_roll_back_meta_and_job() {
    for failure in 0..4 {
        let (domain, _) = CreateAnalyzeSession().unwrap();
        let pool = SystemSessionPool::new(domain.clone());
        insert(&pool, 79002, JobState::Queueing);
        let before = global_id(&domain);
        let owner = Arc::new(Lease::default());
        let mut executor = TransactionStep::new(owner.clone());
        executor.retire = failure == 0;
        executor.cancel = failure == 1;
        executor.fail = failure == 2;
        if failure == 3 {
            owner.owner.store(false, Ordering::Release);
        }
        let mut lease = pool.acquire().unwrap();
        let result = scheduler().schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0);
        if failure == 3 {
            assert_eq!(result.unwrap(), 0);
            assert!(executor.seen.is_empty());
        } else {
            assert!(result.is_err());
        }
        assert_eq!(read(&pool, 79002).state, JobState::Queueing);
        assert_eq!(read(&pool, 79002).error_count, 0);
        assert_eq!(global_id(&domain), before);
        drop(lease);
        pool.close();
        domain.close();
    }
}

#[test]
fn crossks_align_durable_scheduler_preserves_pause_cancel_and_concurrent_admin_change() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(domain.clone());
    insert(&pool, 79003, JobState::Paused);
    insert(&pool, 79004, JobState::Cancelling);
    let owner = Arc::new(Lease::default());
    let mut executor = TransactionStep::new(owner.clone());
    let mut lease = pool.acquire().unwrap();
    // The earlier paused job contributes a pending conflict, as in Go.
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0)
            .unwrap(),
        0
    );
    assert!(executor.seen.is_empty());
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 79004)
            .unwrap(),
        1
    );
    assert_eq!(executor.seen, vec![JobState::Cancelling]);
    assert_eq!(read(&pool, 79004).state, JobState::Cancelling);
    executor.concurrently_pause = Some(pool.clone());
    let before = global_id(&domain);
    let err = scheduler()
        .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 79004)
        .unwrap_err();
    assert!(err.contains("job meta changed by others"), "{err}");
    assert_eq!(read(&pool, 79004).state, JobState::Paused);
    assert_eq!(global_id(&domain), before);
    drop(lease);
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_durable_scheduler_recovers_committed_step_after_sync_interruption() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(domain.clone());
    insert(&pool, 79005, JobState::Queueing);
    let owner = Arc::new(Lease::default());
    let mut executor = TransactionStep::new(owner.clone());
    executor.fail_sync = true;
    let mut lease = pool.acquire().unwrap();
    assert!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0)
            .is_err()
    );
    assert_eq!(read(&pool, 79005).state, JobState::Running);
    executor.fail_sync = false;
    assert_eq!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0)
            .unwrap(),
        1
    );
    assert_eq!(
        executor.recovered,
        vec![JobState::Queueing, JobState::Running]
    );
    drop(lease);
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_durable_scheduler_uses_normal_owner_manager_live_state() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let manager = astersql_owner::NewMockManager(
        astersql_owner::Context::new(),
        "durable-normal-owner",
        None,
        "/test/durable-normal-owner",
    );
    let cancellation = Arc::new(astersql_session_syssession::CancellationToken::default());
    let lease = super::system_session::DdlOwnerLease {
        owner: manager.clone(),
        cancellation: cancellation.clone(),
    };
    assert!(!lease.is_owner());
    runtime.block_on(manager.CampaignOwner(&[])).unwrap();
    assert!(lease.is_owner());
    runtime.block_on(manager.RetireOwner());
    assert!(!lease.is_owner());
    cancellation.cancel();
    assert!(lease.is_cancelled());
    runtime.block_on(manager.Close());
}

#[test]
fn crossks_align_durable_scheduler_conflicting_commit_rolls_back_metadata() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(domain.clone());
    insert(&pool, 79006, JobState::Queueing);
    let before = global_id(&domain);
    let owner = Arc::new(Lease::default());
    let mut executor = TransactionStep::new(owner.clone());
    executor.overlapping_pause = Some(pool.clone());
    let mut lease = pool.acquire().unwrap();
    assert!(
        scheduler()
            .schedule_persisted(&mut lease, owner.as_ref(), &mut executor, 0)
            .is_err()
    );
    assert_eq!(read(&pool, 79006).state, JobState::Paused);
    assert_eq!(global_id(&domain), before);
    drop(lease);
    pool.close();
    domain.close();
}
