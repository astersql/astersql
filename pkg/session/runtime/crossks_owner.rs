// Copyright 2026 AsterSQL.

//! Target-keyspace DDL owner and durable AlterTableMode job execution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_domain::Domain;
use astersql_domain_crossks::{HistoryJobState, Lifecycle, ManagerError};
use astersql_domain_serverinfo::{Context, EtcdClient};
use astersql_meta_model::{
    TableMode,
    group_3::{ACTION_ALTER_TABLE_MODE, Job as ModelJob, JobState},
};
use astersql_owner::Manager as ElectionManager;
use tokio::runtime::{Builder, Runtime};

use super::crossks_schema::CrossKSSchemaSyncer;
use super::crossks_session_pool::{CrossKSSessionLease, CrossKSSessionPool};

const DDL_OWNER_KEY: &str = astersql_ddl_util::DDLOwnerKey;
const DDL_OWNER_LEASE_SECONDS: i32 = 45;

fn sql_blob(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 3);
    out.push_str("x'");
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut out, "{byte:02x}").expect("writing hex to string");
    }
    out.push('\'');
    out
}

fn sql_name(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

fn model_mode(value: i64) -> Result<TableMode, String> {
    match value {
        0 => Ok(TableMode::TableModeNormal),
        1 => Ok(TableMode::TableModeImport),
        2 => Ok(TableMode::TableModeRestore),
        _ => Err(format!("invalid AlterTableMode target {value}")),
    }
}

/// One target keyspace's elected DDL owner. The lease keeps the owner claim
/// alive; the SQL queue is durable and can be resumed by the next owner.
pub struct CrossKSDdlOwner {
    domain: Arc<Domain>,
    pool: Arc<CrossKSSessionPool>,
    etcd: Arc<dyn EtcdClient>,
    id: String,
    lease: Mutex<Option<i64>>,
    schema: Mutex<Option<Arc<CrossKSSchemaSyncer>>>,
    election: Option<Arc<dyn ElectionManager>>,
    election_runtime: Mutex<Option<Runtime>>,
    wake: (Mutex<bool>, Condvar),
    stopped: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CrossKSDdlOwner {
    pub fn new(
        domain: Arc<Domain>,
        pool: Arc<CrossKSSessionPool>,
        etcd: Arc<dyn EtcdClient>,
        id: String,
    ) -> Arc<Self> {
        Self::new_inner(domain, pool, etcd, id, None)
    }

    /// Use the shared Go-compatible etcd election protocol in production.
    pub fn new_with_election(
        domain: Arc<Domain>,
        pool: Arc<CrossKSSessionPool>,
        etcd: Arc<dyn EtcdClient>,
        id: String,
        election: Arc<dyn ElectionManager>,
    ) -> Arc<Self> {
        Self::new_inner(domain, pool, etcd, id, Some(election))
    }

    fn new_inner(
        domain: Arc<Domain>,
        pool: Arc<CrossKSSessionPool>,
        etcd: Arc<dyn EtcdClient>,
        id: String,
        election: Option<Arc<dyn ElectionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            domain,
            pool,
            etcd,
            id,
            lease: Mutex::new(None),
            schema: Mutex::new(None),
            election,
            election_runtime: Mutex::new(None),
            wake: (Mutex::new(false), Condvar::new()),
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    /// Elect the owner and start polling the durable queue. A competing
    /// server leaves its queue untouched and retries after the lease changes.
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        if let Some(election) = &self.election {
            let runtime = Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|error| format!("start DDL owner election runtime: {error}"))?;
            runtime
                .block_on(election.CampaignOwner(&[]))
                .map_err(|error| format!("campaign target DDL owner: {error}"))?;
            *self
                .election_runtime
                .lock()
                .expect("DDL owner election lock poisoned") = Some(runtime);
        }
        let owner = Arc::clone(self);
        let thread = thread::Builder::new()
            .name(format!("crossks-ddl-owner-{}", self.id))
            .spawn(move || owner.run())
            .map_err(|error| {
                self.release_ownership();
                format!("start cross-keyspace DDL owner: {error}")
            })?;
        *self.thread.lock().expect("DDL owner lock poisoned") = Some(thread);
        Ok(())
    }

    pub(crate) fn acquire_ownership(&self) -> Result<bool, String> {
        if let Some(election) = &self.election {
            return Ok(election.IsOwner());
        }
        let current_lease = *self.lease.lock().expect("DDL owner lease lock poisoned");
        if let Some(lease) = current_lease {
            let owner = self
                .etcd
                .Get(&Context::Background(), DDL_OWNER_KEY, false)
                .map_err(|error| error.to_string())?;
            if owner.first().is_some_and(|value| {
                value.value == self.id.as_bytes() && value.lease == Some(lease)
            }) {
                return Ok(true);
            }
            self.release_ownership();
        }
        let context = Context::Background();
        let lease = self
            .etcd
            .GrantLease(&context, DDL_OWNER_LEASE_SECONDS)
            .map_err(|error| error.to_string())?;
        let acquired = self
            .etcd
            .CompareAndPut(
                &context,
                DDL_OWNER_KEY,
                None,
                self.id.as_bytes().to_vec(),
                lease,
            )
            .map_err(|error| error.to_string());
        match acquired {
            Ok(true) => {
                *self.lease.lock().expect("DDL owner lease lock poisoned") = Some(lease);
                Ok(true)
            }
            other => {
                let _ = self.etcd.RevokeLease(&context, lease);
                other
            }
        }
    }

    fn run(&self) {
        'owner: while !self.stopped.load(Ordering::Acquire) {
            if self.acquire_ownership() == Ok(true) {
                while !self.stopped.load(Ordering::Acquire) {
                    // This worker only implements AlterTableMode. Relinquish
                    // Go's shared DDL election when another job type appears
                    // so a full DDL owner can take over its queue.
                    if self.foreign_jobs_present() == Ok(true) {
                        break 'owner;
                    }
                    match self.process_one() {
                        Ok(true) => continue,
                        Ok(false) | Err(_) => break,
                    }
                }
            }
            let (wake, ready) = &self.wake;
            let pending = wake.lock().expect("DDL owner wake lock poisoned");
            let (mut pending, _) = ready
                .wait_timeout(pending, Duration::from_millis(100))
                .expect("DDL owner wake lock poisoned");
            *pending = false;
        }
        self.release_ownership();
    }

    fn foreign_jobs_present(&self) -> Result<bool, String> {
        let lease = self.pool.acquire()?;
        Ok(!lease.query(format!(
            "SELECT job_id FROM mysql.tidb_ddl_job WHERE type <> {ACTION_ALTER_TABLE_MODE} LIMIT 1"
        ))?.is_empty())
    }

    pub fn notify(&self) {
        *self.wake.0.lock().expect("DDL owner wake lock poisoned") = true;
        self.wake.1.notify_all();
    }

    pub fn install_schema_syncer(&self, syncer: Arc<CrossKSSchemaSyncer>) {
        *self.schema.lock().expect("DDL owner schema lock poisoned") = Some(syncer);
    }

    fn release_ownership(&self) {
        if let Some(election) = &self.election {
            if let Some(runtime) = self
                .election_runtime
                .lock()
                .expect("DDL owner election lock poisoned")
                .take()
            {
                runtime.block_on(election.Close());
            }
            return;
        }
        if let Some(lease) = self
            .lease
            .lock()
            .expect("DDL owner lease lock poisoned")
            .take()
        {
            let context = Context::Background();
            let _ =
                self.etcd
                    .CompareAndDelete(&context, DDL_OWNER_KEY, (self.id.as_bytes(), lease));
            let _ = self.etcd.RevokeLease(&context, lease);
        }
    }

    /// Execute one persisted table-mode job. The row is claimed before
    /// applying metadata and moved to history in a separate transaction;
    /// replaying after a crash is safe because setting the same mode is idempotent.
    pub fn process_one(&self) -> Result<bool, String> {
        if !self.acquire_ownership()? {
            return Err("cross-keyspace DDL owner lease is absent".into());
        }
        let lease = self.pool.acquire()?;
        let rows = lease.query(format!(
            "SELECT job_id FROM mysql.tidb_ddl_job WHERE type = {ACTION_ALTER_TABLE_MODE} ORDER BY job_id"
        ))?;
        for row in rows {
            let Some(id) = row.first() else {
                continue;
            };
            let job_id: i64 = id
                .parse()
                .map_err(|error| format!("decode DDL job ID: {error}"))?;
            if self.process_job(&lease, job_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn process_job(&self, lease: &CrossKSSessionLease, job_id: i64) -> Result<bool, String> {
        lease.query("BEGIN PESSIMISTIC")?;
        let result = (|| {
            let rows = lease.query(format!(
                "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id = {job_id} FOR UPDATE"
            ))?;
            let Some(raw) = rows.first().and_then(|row| row.first()) else {
                return Ok(None);
            };
            let mut job = ModelJob::decode(raw.as_bytes()).map_err(|error| error.to_string())?;
            if job.tp != ACTION_ALTER_TABLE_MODE || job.state == JobState::Paused {
                return Ok(None);
            }
            job.state = JobState::Running;
            let encoded = job.encode(false).map_err(|error| error.to_string())?;
            lease.query(format!(
                "UPDATE mysql.tidb_ddl_job SET processing = 1, job_meta = {} WHERE job_id = {job_id}",
                sql_blob(&encoded)
            ))?;
            Ok(Some(job))
        })();
        let Some(mut job) = (match result {
            Ok(job) => {
                lease.query("COMMIT")?;
                job
            }
            Err(error) => {
                let _ = lease.query("ROLLBACK");
                return Err(error);
            }
        }) else {
            return Ok(false);
        };
        let result = (|| {
            let args: serde_json::Value =
                serde_json::from_slice(&job.raw_args).map_err(|error| error.to_string())?;
            let mode = args
                .get("table_mode")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| "AlterTableMode job has no table_mode".to_owned())
                .and_then(model_mode)?;
            self.domain
                .ddl_set_table_mode_by_ids(job.schema_id, job.table_id, mode)
                .map_err(|error| error.to_string())
        })();
        if result.is_ok() {
            if let Some(syncer) = self
                .schema
                .lock()
                .expect("DDL owner schema lock poisoned")
                .as_ref()
            {
                syncer.publish_global()?;
                syncer.wait_all_versions_with_cancel(Duration::from_secs(90), &self.stopped)?;
            }
        }
        job.state = if result.is_ok() {
            JobState::Synced
        } else {
            JobState::Cancelled
        };
        if let Err(error) = &result {
            job.error = Some(error.to_string());
            job.error_count += 1;
        }
        let encoded = job.encode(false).map_err(|error| error.to_string())?;
        lease.query("BEGIN PESSIMISTIC")?;
        let persist = (|| {
            lease.query(format!(
                "INSERT INTO mysql.tidb_ddl_history(job_id, job_meta, db_name, table_name, schema_ids, table_ids) VALUES ({job_id}, {}, {}, {}, '{}', '{}')",
                sql_blob(&encoded),
                sql_name(&job.schema_name),
                sql_name(&job.table_name),
                job.schema_id,
                job.table_id,
            ))?;
            lease.query(format!(
                "DELETE FROM mysql.tidb_ddl_job WHERE job_id = {job_id}"
            ))?;
            lease.query("COMMIT")?;
            Ok::<(), String>(())
        })();
        if persist.is_err() {
            let _ = lease.query("ROLLBACK");
        }
        persist?;
        if let Err(error) = result {
            return Err(format!("AlterTableMode job {job_id} failed: {error}"));
        }
        Ok(true)
    }

    pub fn history_job(&self, job_id: i64) -> Result<Option<HistoryJobState>, String> {
        let lease = self.pool.acquire()?;
        let rows = lease.query(format!(
            "SELECT job_meta FROM mysql.tidb_ddl_history WHERE job_id = {job_id}"
        ))?;
        let Some(raw) = rows.first().and_then(|row| row.first()) else {
            return Ok(None);
        };
        let job = ModelJob::decode(raw.as_bytes()).map_err(|error| error.to_string())?;
        Ok(Some(match job.state {
            JobState::Synced => HistoryJobState::Synced,
            JobState::Cancelled | JobState::RollbackDone => HistoryJobState::Failed(
                job.error
                    .map_or_else(|| job.state.to_string(), |error| error.to_string()),
            ),
            state => HistoryJobState::Unexpected(state.to_string()),
        }))
    }

    pub fn close(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.wake.1.notify_all();
        if let Some(thread) = self.thread.lock().expect("DDL owner lock poisoned").take() {
            let _ = thread.join();
        }
        self.release_ownership();
    }
}

impl Lifecycle for CrossKSDdlOwner {
    fn close(&self) -> Result<(), ManagerError> {
        self.close();
        Ok(())
    }
}
