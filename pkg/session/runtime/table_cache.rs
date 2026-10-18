// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Canonical KV bridge for Go's cached-table buffer and remote lease protocol.
use super::system_session::{SystemSessionLease, SystemSessionPool};
use super::{ConcreteSession, SessionError, SessionResult, kv};
use astersql_domain::Domain;
use astersql_table_tables::cache::{CacheData, CachedTable, MemBuffer, StateRemote, lease_from_ts};
use astersql_table_tables::state_remote::{
    CachedTableLockType, LockRow, RemoteStore, StateRemoteHandle,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

struct SqlStore {
    domain: Arc<Domain>,
    session: SystemSessionLease,
}
impl Drop for SqlStore {
    fn drop(&mut self) {
        let _ = self.session.query("ROLLBACK");
    }
}
impl RemoteStore for SqlStore {
    type Error = String;
    fn current_ts(&mut self) -> Result<u64, String> {
        self.domain
            .storage()
            .with_storage(|s| s.CurrentVersion("global"))
            .map(|v| v.Ver)
            .map_err(|e| e.to_string())
    }
    fn load(&mut self, id: i64, _: bool) -> Result<LockRow, String> {
        let rows = self.session.query(format!("SELECT lock_type, lease, oldReadLease FROM mysql.table_cache_meta WHERE tid={id} FOR UPDATE"))?;
        let row = rows
            .first()
            .ok_or_else(|| format!("table_cache_meta tid not exist {id}"))?;
        let lock_type = match row[0].as_str() {
            "NONE" => CachedTableLockType::None,
            "READ" => CachedTableLockType::Read,
            "INTEND" => CachedTableLockType::Intend,
            "WRITE" => CachedTableLockType::Write,
            value => return Err(format!("invalid cached table lock type {value}")),
        };
        Ok(LockRow {
            lock_type,
            lease: row[1].parse::<u64>().map_err(|e| e.to_string())?,
            old_read_lease: row[2].parse::<u64>().map_err(|e| e.to_string())?,
        })
    }
    fn update(&mut self, id: i64, row: LockRow) -> Result<(), String> {
        self.session.query(format!("UPDATE mysql.table_cache_meta SET lock_type='{}', lease={}, oldReadLease={} WHERE tid={id}", row.lock_type.as_str(), row.lease, row.old_read_lease))?;
        Ok(())
    }
}
struct SqlRemote(Weak<Domain>);
impl SqlRemote {
    fn run<T>(
        &self,
        op: impl FnOnce(&mut StateRemoteHandle<SqlStore>) -> Result<T, String>,
    ) -> Result<T, String> {
        let domain = self.0.upgrade().ok_or("cached table Domain closed")?;
        let session = SystemSessionPool::new(domain.clone()).acquire()?;
        session.query("BEGIN PESSIMISTIC")?;
        let mut handle = StateRemoteHandle::new(SqlStore { domain, session });
        let result = op(&mut handle)?;
        handle.store().session.query("COMMIT")?;
        Ok(result)
    }
}
impl StateRemote for SqlRemote {
    type Error = String;
    fn lock_for_read(&mut self, id: i64, lease: u64) -> Result<bool, String> {
        self.run(|h| h.lock_for_read(id, lease))
    }
    fn renew_read_lease(&mut self, id: i64, old: u64, new: u64) -> Result<u64, String> {
        self.run(|h| h.renew_read_lease(id, old, new))
    }
    fn lock_for_write(&mut self, id: i64, duration: Duration) -> Result<u64, String> {
        // Commit INTEND before waiting, so readers cannot extend the old lease.
        loop {
            let (wait, lease) = self.run(|h| h.lock_for_write_once(id, duration))?;
            if wait.is_zero() {
                return Ok(lease);
            }
            std::thread::sleep(wait);
        }
    }
    fn renew_write_lease(&mut self, id: i64, lease: u64) -> Result<bool, String> {
        self.run(|h| h.renew_write_lease(id, lease))
    }
}
struct Entry {
    domain: Weak<Domain>,
    table: CachedTable<SqlRemote>,
    loading: AtomicBool,
}
type Entries = HashMap<(usize, i64, i64), Arc<Entry>>;
fn entry(domain: &Arc<Domain>, id: i64) -> Arc<Entry> {
    static ENTRIES: OnceLock<Mutex<Entries>> = OnceLock::new();
    let mut entries = ENTRIES.get_or_init(Default::default).lock().unwrap();
    let owner = Arc::as_ptr(domain) as usize;
    let schema_version = domain.info_schema().SchemaMetaVersion();
    entries.retain(|(node, table, version), e| {
        e.domain.strong_count() != 0
            && !(*node == owner && *table == id && *version != schema_version)
    });
    entries
        .entry((
            Arc::as_ptr(domain) as usize,
            id,
            domain.info_schema().SchemaMetaVersion(),
        ))
        .or_insert_with(|| {
            let weak = Arc::downgrade(domain);
            Arc::new(Entry {
                domain: weak.clone(),
                table: CachedTable::new(id, SqlRemote(weak)),
                loading: AtomicBool::new(false),
            })
        })
        .clone()
}
fn enabled(domain: &Domain, id: i64) -> bool {
    domain
        .info_schema()
        .TableByID(id)
        .and_then(|t| t.ModelMeta().ok())
        .is_some_and(|t| t.TableCacheStatusType == astersql_meta_model::TableCacheStatusEnable)
}
fn duration() -> Duration {
    Duration::from_secs(astersql_sessionctx_vardef::TableCacheLease.Load().max(1) as u64)
}
/// Return only a loaded buffer whose snapshot and read lease cover this TS.
/// A miss schedules the same full-table KV load as Go, including index keys.
pub(super) fn read(
    domain: &Arc<Domain>,
    id: i64,
    ts: u64,
    allow_load: bool,
) -> Option<Arc<MemBuffer>> {
    if !enabled(domain, id) {
        return None;
    }
    let entry = entry(domain, id);
    let (buffer, loading, renew) = entry.table.try_read_from_cache(ts, duration());
    if buffer.is_some() {
        if renew && !entry.loading.swap(true, Ordering::AcqRel) {
            std::thread::spawn(move || {
                if let Err(error) = entry.table.renew_lease(ts, duration()) {
                    eprintln!("renew cached table {id}: {error}");
                }
                entry.loading.store(false, Ordering::Release);
            });
        }
        return buffer;
    }
    if allow_load && !loading && !entry.loading.swap(true, Ordering::AcqRel) {
        let domain = domain.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                if !entry.table.update_lock_for_read(ts, duration())? {
                    return Ok(());
                }
                let lease = lease_from_ts(ts, duration());
                entry.table.install_cache(CacheData {
                    start: ts,
                    lease,
                    mem_buffer: None,
                });
                let version = domain
                    .storage()
                    .with_storage(|s| s.CurrentVersion("global"))
                    .map_err(|e| e.to_string())?;
                let snapshot = domain.storage().with_storage(|s| s.GetSnapshot(version));
                let prefix = astersql_tablecodec::GenTablePrefix(id);
                let mut iter = snapshot
                    .Iter(
                        kv::Key(prefix.0.clone()),
                        Some(kv::Key(prefix.PrefixNext().0)),
                    )
                    .map_err(|e| e.to_string())?;
                let mut buffer = MemBuffer::new();
                let result = (|| -> Result<(), String> {
                    while iter.Valid() {
                        buffer.insert(iter.Key().0, iter.Value());
                        iter.Next().map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                iter.Close();
                result?;
                entry.table.install_cache(CacheData {
                    start: version.Ver,
                    lease,
                    mem_buffer: Some(Arc::new(buffer)),
                });
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("load cached table {id}: {error}");
            }
            entry.loading.store(false, Ordering::Release);
        });
    }
    None
}

pub(super) struct BufferSource(pub Arc<MemBuffer>);
impl kv::Getter for BufferSource {
    fn Get(
        &self,
        _: &kv::Context,
        key: kv::Key,
        _: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, astersql_errors::SharedError> {
        self.0
            .get(&key.0)
            .cloned()
            .map(|v| kv::NewValueEntry(v, 0))
            .ok_or_else(|| kv::ErrNotExist.FastGenByArgs(&[]))
    }
}
struct BufferIterator {
    rows: std::vec::IntoIter<(Vec<u8>, Vec<u8>)>,
    current: Option<(Vec<u8>, Vec<u8>)>,
}
impl BufferIterator {
    fn new(rows: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        let mut rows = rows.into_iter();
        let current = rows.next();
        Self { rows, current }
    }
}
impl kv::Iterator for BufferIterator {
    fn Valid(&self) -> bool {
        self.current.is_some()
    }
    fn Key(&self) -> kv::Key {
        kv::Key(self.current.as_ref().unwrap().0.clone())
    }
    fn Value(&self) -> Vec<u8> {
        self.current.as_ref().unwrap().1.clone()
    }
    fn Next(&mut self) -> Result<(), astersql_errors::SharedError> {
        self.current = self.rows.next();
        Ok(())
    }
    fn Close(&mut self) {
        self.current = None;
        self.rows = Vec::new().into_iter();
    }
}
impl kv::Retriever for BufferSource {
    fn Iter(
        &self,
        start: kv::Key,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        let rows = self
            .0
            .range(start.0..)
            .take_while(|(k, _)| end.as_ref().is_none_or(|end| *k < &end.0))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(Box::new(BufferIterator::new(rows)))
    }
    fn IterReverse(
        &self,
        start: Option<kv::Key>,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        let rows = self
            .0
            .iter()
            .rev()
            .filter(|(k, _)| {
                start.as_ref().is_none_or(|s| *k < &s.0) && end.as_ref().is_none_or(|e| *k >= &e.0)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(Box::new(BufferIterator::new(rows)))
    }
}

/// Keep write leases alive through commit and validate the actual commit TS.
pub(super) struct WriteLeases {
    stop: Arc<(Mutex<bool>, std::sync::Condvar)>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl Drop for WriteLeases {
    fn drop(&mut self) {
        *self.stop.0.lock().unwrap() = true;
        self.stop.1.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
impl ConcreteSession {
    #[cfg(test)]
    pub(crate) fn CachedTableBufferForTest(&self, id: i64) -> Option<Arc<MemBuffer>> {
        let version = self
            .domain
            .storage()
            .with_storage(|store| store.CurrentVersion("global"))
            .unwrap();
        read(&self.domain, id, version.Ver, true)
    }
    pub(super) fn cached_table_write_leases(
        &self,
        transaction: &mut dyn kv::Transaction,
        ids: impl IntoIterator<Item = i64>,
    ) -> SessionResult<Option<WriteLeases>> {
        let mut ids = ids
            .into_iter()
            .filter(|id| enabled(&self.domain, *id))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(None);
        }
        let stop = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let mut guard = WriteLeases {
            stop: stop.clone(),
            workers: Vec::new(),
        };
        let (send, receive) = std::sync::mpsc::channel();
        let mut leases = Vec::new();
        // Go starts one WriteLockAndKeepAlive per table before waiting for
        // acquisition. A table waiting for readers must not delay renewal of
        // another table's already acquired write lease.
        for id in ids {
            let lease = Arc::new(AtomicU64::new(0));
            leases.push(lease.clone());
            let mut remote = SqlRemote(Arc::downgrade(&self.domain));
            let signal = stop.clone();
            let ready = send.clone();
            guard.workers.push(std::thread::spawn(move || {
                let acquired = remote.lock_for_write(id, Duration::from_secs(5));
                if let Ok(value) = &acquired {
                    lease.store(*value, Ordering::Release);
                }
                let success = acquired.is_ok();
                let _ = ready.send(acquired.map(|_| ()));
                // Acquisition notification is complete. The renewal worker
                // outlives it; retaining this sender would deadlock the waiter
                // which collects all acquisition results before committing.
                drop(ready);
                if !success {
                    return;
                }
                loop {
                    let stopped = signal.0.lock().unwrap();
                    let (stopped, _) = signal
                        .1
                        .wait_timeout_while(stopped, Duration::from_millis(2500), |s| !*s)
                        .unwrap();
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    // Go extends the old lease by the write-lease duration.
                    let old = lease.load(Ordering::Acquire);
                    let next = lease_from_ts(old, Duration::from_secs(5));
                    match remote.renew_write_lease(id, next) {
                        Ok(true) => lease.store(next, Ordering::Release),
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("renew cached table write lease {id}: {error}");
                            return;
                        }
                    }
                }
            }));
        }
        drop(send);
        let mut acquisition_error = None;
        for result in receive {
            if let Err(error) = result {
                acquisition_error = Some(error);
            }
        }
        if let Some(error) = acquisition_error {
            return Err(SessionError::new(error));
        }
        let old = transaction
            .GetOption(kv::SchemaChecker)
            .and_then(|v| v.downcast_ref::<kv::TransactionSchemaChecker>())
            .cloned();
        transaction.SetOption(
            kv::SchemaChecker,
            Some(Box::new(kv::TransactionSchemaChecker(Arc::new(
                move |ts| {
                    if leases
                        .iter()
                        .any(|lease| ts >= lease.load(Ordering::Acquire))
                    {
                        return Err(astersql_errors::New(
                            "cached table write lease expired before commit",
                        ));
                    }
                    if let Some(old) = &old {
                        (old.0)(ts)?;
                    }
                    Ok(())
                },
            )))),
        );
        // client-go/txnkv/transaction/2pc.go disables both optimizations when
        // commitTSUpperBoundCheck is installed. Use the actual 2PC commit TS.
        transaction.SetOption(kv::EnableAsyncCommit, Some(Box::new(false)));
        transaction.SetOption(kv::Enable1PC, Some(Box::new(false)));
        Ok(Some(guard))
    }
}

struct CacheSnapshot {
    snapshot: Box<dyn kv::Snapshot>,
    buffer: Option<BufferSource>,
    variables: Arc<astersql_sessionctx_variable::session::SessionVars>,
}
impl CacheSnapshot {
    fn source(&self) -> &dyn kv::Retriever {
        if let Some(buffer) = &self.buffer {
            self.variables.MarkReadFromTableCache();
            buffer
        } else {
            self.snapshot.as_ref()
        }
    }
}
impl kv::Getter for CacheSnapshot {
    fn Get(
        &self,
        ctx: &kv::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, astersql_errors::SharedError> {
        self.source().Get(ctx, key, options)
    }
}
impl kv::Retriever for CacheSnapshot {
    fn Iter(
        &self,
        start: kv::Key,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        self.source().Iter(start, end)
    }
    fn IterReverse(
        &self,
        start: Option<kv::Key>,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        self.source().IterReverse(start, end)
    }
}
impl kv::Snapshot for CacheSnapshot {
    fn BatchGet(
        &self,
        ctx: &kv::Context,
        keys: &[kv::Key],
        options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, astersql_errors::SharedError> {
        if self.buffer.is_none() {
            return self.snapshot.BatchGet(ctx, keys, options);
        }
        let mut result = HashMap::new();
        for key in keys {
            match self.source().Get(ctx, key.clone(), &[]) {
                Ok(value) => {
                    result.insert(String::from_utf8_lossy(&key.0).into_owned(), value);
                }
                Err(error) if kv::ErrNotExist.Equal(Some(&error)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
    fn SetOption(&mut self, option: i32, value: Option<Box<dyn std::any::Any>>) {
        self.snapshot.SetOption(option, value);
    }
    fn SnapCacheSize(&self) -> usize {
        self.snapshot.SnapCacheSize()
    }
}
impl ConcreteSession {
    pub(super) fn cached_table_snapshot(
        &self,
        store: &dyn kv::Storage,
        table: &astersql_meta_model::TableInfo,
        version: kv::Version,
    ) -> Box<dyn kv::Snapshot> {
        Box::new(CacheSnapshot {
            snapshot: store.GetSnapshot(version),
            buffer: read(&self.domain, table.ID, version.Ver, {
                let context = &self.session_vars.StmtCtx;
                !context.InInsertStmt
                    && !context.InUpdateStmt
                    && !context.InDeleteStmt
                    && !context.InExplainStmt
            })
            .map(BufferSource),
            variables: self.session_vars.clone(),
        })
    }
}
