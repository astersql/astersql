// Copyright 2026 AsterSQL.

//! Thread-confined SQL sessions leased to cross-keyspace DDL workers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use astersql_ddl_jobsubmit as jobsubmit;
use astersql_ddl_systable as systable;
use astersql_domain::Domain;
use astersql_domain_crossks::SessionPool;
use astersql_util_codec as codec;

use super::{ConcreteSession, kv};

const CROSS_KS_SESSION_POOL_SIZE: usize = astersql_domain_crossks::CROSS_KEYSPACE_SESSION_POOL_SIZE;

enum Command {
    Query(String, mpsc::SyncSender<Result<Vec<Vec<String>>, String>>),
    Metadata(
        MetadataCommand,
        mpsc::SyncSender<Result<MetadataValue, String>>,
    ),
    Stop,
}

enum MetadataCommand {
    ReadBdrRoleAndStartTs,
    TransactionStartTs,
    LockGlobalId(u64),
    CurrentVersion,
    SetSnapshotTs(u64),
    GenerateGlobalIds(usize),
}

enum MetadataValue {
    RoleAndStartTs(String, u64),
    Timestamp(u64),
    Ids(Vec<i64>),
    Unit,
}

fn global_id_key() -> kv::Key {
    let key = codec::EncodeBytes(vec![b'm'], b"NextGlobalID");
    kv::Key(codec::EncodeUint(key, u64::from(b's')))
}

fn bdr_role_key() -> kv::Key {
    let key = codec::EncodeBytes(vec![b'm'], b"BDRRole");
    kv::Key(codec::EncodeUint(key, u64::from(b's')))
}

struct Worker {
    sender: mpsc::Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// A fixed-size system session pool. Every SQL session stays on its owning
/// worker thread; a lease preserves that affinity across a transaction.
pub struct CrossKSSessionPool {
    workers: Vec<Worker>,
    available: Mutex<Vec<usize>>,
    ready: Condvar,
    closed: AtomicBool,
}

impl CrossKSSessionPool {
    pub fn new(domain: Arc<Domain>) -> Arc<Self> {
        Self::try_new(domain).expect("start cross-keyspace system session pool")
    }

    pub fn try_new(domain: Arc<Domain>) -> Result<Arc<Self>, String> {
        let mut workers: Vec<Worker> = Vec::with_capacity(CROSS_KS_SESSION_POOL_SIZE);
        for index in 0..CROSS_KS_SESSION_POOL_SIZE {
            let (sender, receiver) = mpsc::channel();
            let domain = Arc::clone(&domain);
            let thread = thread::Builder::new()
                .name(format!("crossks-system-session-{index}"))
                .spawn(move || run_worker(domain, receiver))
                .map_err(|error| {
                    for worker in &workers {
                        let _ = worker.sender.send(Command::Stop);
                    }
                    for worker in &workers {
                        if let Some(thread) =
                            worker.thread.lock().expect("worker lock poisoned").take()
                        {
                            let _ = thread.join();
                        }
                    }
                    format!("start cross-keyspace system session worker: {error}")
                })?;
            workers.push(Worker {
                sender,
                thread: Mutex::new(Some(thread)),
            });
        }
        Ok(Arc::new(Self {
            workers,
            available: Mutex::new((0..CROSS_KS_SESSION_POOL_SIZE).rev().collect()),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
        }))
    }

    /// Borrow one persistent SQL session until the returned lease is dropped.
    pub fn acquire(self: &Arc<Self>) -> Result<CrossKSSessionLease, String> {
        let mut available = self
            .available
            .lock()
            .expect("crossks session pool lock poisoned");
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err("cross-keyspace session pool is closed".into());
            }
            if let Some(index) = available.pop() {
                return Ok(CrossKSSessionLease {
                    pool: Arc::clone(self),
                    index,
                });
            }
            available = self
                .ready
                .wait(available)
                .expect("crossks session pool lock poisoned");
        }
    }
}

impl SessionPool for CrossKSSessionPool {
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.ready.notify_all();
        for worker in &self.workers {
            let _ = worker.sender.send(Command::Stop);
        }
        for worker in &self.workers {
            if let Some(thread) = worker.thread.lock().expect("worker lock poisoned").take() {
                let _ = thread.join();
            }
        }
    }
}

fn run_worker(domain: Arc<Domain>, receiver: mpsc::Receiver<Command>) {
    let session = ConcreteSession::new(domain);
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Query(sql, reply) => {
                let result = (|| {
                    let mut rows = Vec::new();
                    for mut result_set in
                        session.execute(&sql).map_err(|error| error.to_string())?
                    {
                        while let Some(row) =
                            result_set.next_row().map_err(|error| error.to_string())?
                        {
                            rows.push(row);
                        }
                    }
                    Ok(rows)
                })();
                let _ = reply.send(result);
            }
            Command::Metadata(command, reply) => {
                let _ = reply.send(run_metadata(&session, command));
            }
            Command::Stop => break,
        }
    }
}

fn run_metadata(
    session: &ConcreteSession,
    command: MetadataCommand,
) -> Result<MetadataValue, String> {
    if matches!(command, MetadataCommand::ReadBdrRoleAndStartTs)
        && session.state.borrow().transaction.is_none()
    {
        session
            .execute("BEGIN PESSIMISTIC")
            .map_err(|error| error.to_string())?;
        let result = run_metadata(session, command);
        let rollback = session
            .execute("ROLLBACK")
            .map_err(|error| error.to_string());
        return result.and_then(|value| rollback.map(|_| value));
    }
    if matches!(command, MetadataCommand::CurrentVersion) {
        return session
            .domain
            .storage_handle()
            .with_storage(|store| store.CurrentVersion("global"))
            .map(|version| MetadataValue::Timestamp(version.Ver))
            .map_err(|error| error.to_string());
    }
    let mut state = session.state.borrow_mut();
    let transaction = state
        .transaction
        .as_mut()
        .ok_or_else(|| "cross-keyspace DDL requires an active transaction".to_owned())?;
    match command {
        MetadataCommand::ReadBdrRoleAndStartTs => {
            let role = match transaction.Get(&kv::Context::default(), bdr_role_key(), &[]) {
                Ok(value) => String::from_utf8(value.Value).map_err(|error| error.to_string())?,
                Err(error) if kv::IsErrNotFound(&error) => "none".to_owned(),
                Err(error) => return Err(error.to_string()),
            };
            Ok(MetadataValue::RoleAndStartTs(role, transaction.StartTS()))
        }
        MetadataCommand::TransactionStartTs => Ok(MetadataValue::Timestamp(transaction.StartTS())),
        MetadataCommand::LockGlobalId(for_update_ts) => {
            transaction.SetOption(kv::SnapshotTS, Some(Box::new(for_update_ts)));
            transaction
                .LockKeys(
                    &kv::Context::default(),
                    &mut kv::LockCtx::default(),
                    &[global_id_key()],
                )
                .map_err(|error| error.to_string())?;
            Ok(MetadataValue::Unit)
        }
        MetadataCommand::SetSnapshotTs(timestamp) => {
            transaction.SetOption(kv::SnapshotTS, Some(Box::new(timestamp)));
            Ok(MetadataValue::Unit)
        }
        MetadataCommand::GenerateGlobalIds(count) => {
            let count = i64::try_from(count).map_err(|error| error.to_string())?;
            let last = kv::IncInt64(transaction.as_mut(), &global_id_key(), count)
                .map_err(|error| error.to_string())?;
            Ok(MetadataValue::Ids((last - count + 1..=last).collect()))
        }
        MetadataCommand::CurrentVersion => unreachable!(),
    }
}

/// One thread-affine system session borrowed from the pool.
pub struct CrossKSSessionLease {
    pool: Arc<CrossKSSessionPool>,
    index: usize,
}

impl CrossKSSessionLease {
    /// Execute SQL and return text rows from every result set.
    pub fn query(&self, sql: impl Into<String>) -> Result<Vec<Vec<String>>, String> {
        if self.pool.closed.load(Ordering::Acquire) {
            return Err("cross-keyspace session pool is closed".into());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pool.workers[self.index]
            .sender
            .send(Command::Query(sql.into(), sender))
            .map_err(|_| "cross-keyspace system session worker stopped".to_owned())?;
        receiver
            .recv()
            .map_err(|_| "cross-keyspace system session worker stopped".to_owned())?
    }

    fn metadata(&self, command: MetadataCommand) -> Result<MetadataValue, String> {
        if self.pool.closed.load(Ordering::Acquire) {
            return Err("cross-keyspace session pool is closed".into());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pool.workers[self.index]
            .sender
            .send(Command::Metadata(command, sender))
            .map_err(|_| "cross-keyspace system session worker stopped".to_owned())?;
        receiver
            .recv()
            .map_err(|_| "cross-keyspace system session worker stopped".to_owned())?
    }
}

impl Drop for CrossKSSessionLease {
    fn drop(&mut self) {
        if !self.pool.closed.load(Ordering::Acquire) {
            self.pool
                .available
                .lock()
                .expect("crossks session pool lock poisoned")
                .push(self.index);
            self.pool.ready.notify_one();
        }
    }
}

fn job_error(message: String) -> jobsubmit::Error {
    jobsubmit::Error {
        kind: jobsubmit::ErrorKind::Storage,
        message,
    }
}

impl jobsubmit::Session for CrossKSSessionLease {
    fn begin(&mut self) -> Result<(), jobsubmit::Error> {
        self.query("BEGIN PESSIMISTIC")
            .map(|_| ())
            .map_err(job_error)
    }

    fn rollback(&mut self) {
        let _ = self.query("ROLLBACK");
    }

    fn commit(&mut self) -> Result<(), jobsubmit::Error> {
        self.query("COMMIT").map(|_| ()).map_err(job_error)
    }

    fn read_bdr_role_and_start_ts(&mut self) -> Result<(String, u64), jobsubmit::Error> {
        match self
            .metadata(MetadataCommand::ReadBdrRoleAndStartTs)
            .map_err(job_error)?
        {
            MetadataValue::RoleAndStartTs(role, start_ts) => Ok((role, start_ts)),
            _ => unreachable!(),
        }
    }

    fn transaction_start_ts(&self) -> Result<u64, jobsubmit::Error> {
        match self
            .metadata(MetadataCommand::TransactionStartTs)
            .map_err(job_error)?
        {
            MetadataValue::Timestamp(ts) => Ok(ts),
            _ => unreachable!(),
        }
    }

    fn set_pessimistic(&mut self) {
        // begin() starts a pessimistic transaction on the owning SQL thread.
    }

    fn lock_global_id_key(&mut self, for_update_ts: u64) -> Result<(), jobsubmit::Error> {
        self.metadata(MetadataCommand::LockGlobalId(for_update_ts))
            .map(|_| ())
            .map_err(job_error)
    }

    fn current_version(&self) -> Result<u64, jobsubmit::Error> {
        match self
            .metadata(MetadataCommand::CurrentVersion)
            .map_err(job_error)?
        {
            MetadataValue::Timestamp(ts) => Ok(ts),
            _ => unreachable!(),
        }
    }

    fn set_snapshot_ts(&mut self, timestamp: u64) {
        let _ = self.metadata(MetadataCommand::SetSnapshotTs(timestamp));
    }

    fn generate_global_ids(&mut self, count: usize) -> Result<Vec<i64>, jobsubmit::Error> {
        match self
            .metadata(MetadataCommand::GenerateGlobalIds(count))
            .map_err(job_error)?
        {
            MetadataValue::Ids(ids) => Ok(ids),
            _ => unreachable!(),
        }
    }

    fn execute(&mut self, sql: &str, _label: &str) -> Result<(), jobsubmit::Error> {
        self.query(sql).map(|_| ()).map_err(job_error)
    }
}

/// Jobsubmit's borrow/return boundary over the cross-keyspace SQL workers.
pub struct CrossKSJobSessionPool {
    inner: Arc<CrossKSSessionPool>,
}

impl CrossKSJobSessionPool {
    pub fn new(inner: Arc<CrossKSSessionPool>) -> Self {
        Self { inner }
    }
}

impl jobsubmit::SessionPool for CrossKSJobSessionPool {
    fn get(&self) -> Result<Box<dyn jobsubmit::Session>, jobsubmit::Error> {
        self.inner
            .acquire()
            .map(|lease| Box::new(lease) as Box<dyn jobsubmit::Session>)
            .map_err(job_error)
    }

    fn put(&self, session: Box<dyn jobsubmit::Session>) {
        drop(session);
    }
}

fn system_table_error(message: String) -> systable::Error {
    systable::Error::Execute(message)
}

impl systable::Session for CrossKSSessionLease {
    fn execute(
        &mut self,
        _context: &systable::Context,
        sql: &str,
        _label: &str,
    ) -> Result<Vec<systable::Row>, systable::Error> {
        self.query(sql)
            .map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        systable::Row(
                            row.into_iter()
                                .map(|value| {
                                    value.parse::<i64>().map_or_else(
                                        |_| systable::Value::Bytes(value.into_bytes()),
                                        systable::Value::Int,
                                    )
                                })
                                .collect(),
                        )
                    })
                    .collect()
            })
            .map_err(system_table_error)
    }
}

/// System-table manager session pool over the same thread-confined SQL workers.
pub struct CrossKSSystemTablePool {
    inner: Arc<CrossKSSessionPool>,
}

impl CrossKSSystemTablePool {
    pub fn new(inner: Arc<CrossKSSessionPool>) -> Self {
        Self { inner }
    }
}

impl systable::SessionPool for CrossKSSystemTablePool {
    fn get(&self) -> Result<Box<dyn systable::Session>, systable::Error> {
        self.inner
            .acquire()
            .map(|lease| Box::new(lease) as Box<dyn systable::Session>)
            .map_err(systable::Error::Pool)
    }

    fn put(&self, session: Box<dyn systable::Session>) {
        drop(session);
    }
}

/// Real flashback-job guard backed by the target keyspace DDL system table.
pub struct CrossKSFlashbackGuard {
    manager: Arc<dyn systable::Manager>,
}

impl CrossKSFlashbackGuard {
    pub fn new(manager: Arc<dyn systable::Manager>) -> Self {
        Self { manager }
    }
}

impl jobsubmit::SystemTableManager for CrossKSFlashbackGuard {
    fn has_flashback_cluster_job(&self, min_job_id: i64) -> Result<bool, jobsubmit::Error> {
        self.manager
            .has_flashback_cluster_job(&systable::Context::default(), min_job_id)
            .map_err(|error| job_error(error.to_string()))
    }
}

/// Monotonic minimum-job-ID cache used by jobsubmit's flashback guard.
pub struct CrossKSMinJobId {
    refresher: Arc<systable::MinJobIdRefresher>,
}

impl CrossKSMinJobId {
    pub fn new(refresher: Arc<systable::MinJobIdRefresher>) -> Self {
        Self { refresher }
    }
}

impl jobsubmit::MinJobIdProvider for CrossKSMinJobId {
    fn current_min_job_id(&self) -> i64 {
        self.refresher.current_min_job_id()
    }
}
