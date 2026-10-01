// Copyright 2026 AsterSQL.

//! ConcreteSession adapters for the existing system and DDL session pools.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{ConcreteSession, kv};
use astersql_ddl_jobsubmit as jobsubmit;
use astersql_ddl_session as ddl;
use astersql_ddl_systable as systable;
use astersql_domain::Domain;
use astersql_session_syssession as sys;
use astersql_util_codec as codec;

fn sys_error(error: impl std::fmt::Display) -> sys::SessionError {
    sys::SessionError::new(error.to_string())
}
fn ddl_error(error: impl std::fmt::Display) -> ddl::SessionError {
    ddl::SessionError::Sql(error.to_string())
}
fn job_error(error: impl std::fmt::Display) -> jobsubmit::Error {
    // The DDL session ABI carries text errors. Canonical KV retryable errors
    // preserve TiDB's explicit retry marker through that boundary.
    let message = error.to_string();
    let kind = if message.contains(kv::TxnRetryableMark) {
        jobsubmit::ErrorKind::Retryable
    } else {
        jobsubmit::ErrorKind::Storage
    };
    jobsubmit::Error { kind, message }
}
fn job_kv_error(error: kv::Error) -> jobsubmit::Error {
    let kind = if kv::ErrWriteConflict.Equal(Some(&error))
        || kv::ErrWriteConflictInTiDB.Equal(Some(&error))
    {
        jobsubmit::ErrorKind::WriteConflict
    } else if kv::IsTxnRetryableError(Some(&error)) {
        jobsubmit::ErrorKind::Retryable
    } else {
        jobsubmit::ErrorKind::Storage
    };
    jobsubmit::Error {
        kind,
        message: error.to_string(),
    }
}
fn cleanup(session: &mut ConcreteSession) {
    let _ = session.execute("ROLLBACK");
}
fn query(session: &ConcreteSession, sql: &str) -> sys::Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    for mut result in session.execute(sql).map_err(sys_error)? {
        while let Some(row) = result.next_row().map_err(sys_error)? {
            rows.push(row);
        }
        result.close().map_err(sys_error)?;
    }
    Ok(rows)
}
fn bound_sql(sql: &str, args: &[sys::SqlValue]) -> sys::Result<String> {
    let literals = args
        .iter()
        .map(|arg| {
            if let Some(value) = arg.downcast_ref::<String>() {
                Ok(format!(
                    "'{}'",
                    value.replace('\\', "\\\\").replace('\'', "''")
                ))
            } else if let Some(value) = arg.downcast_ref::<i64>() {
                Ok(value.to_string())
            } else if let Some(value) = arg.downcast_ref::<u64>() {
                Ok(value.to_string())
            } else {
                Err(sys_error("unsupported internal SQL argument type"))
            }
        })
        .collect::<sys::Result<Vec<_>>>()?;
    super::bind_parameter_markers(sql, &literals).map_err(sys_error)
}

struct ConcreteSystemContext {
    id: u64,
    worker: sys::ThreadBoundSession<ConcreteSession>,
}
impl sys::SessionContext for ConcreteSystemContext {
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
    fn close(&mut self) {
        self.worker.close();
    }
    fn on_became_owner(&mut self) -> sys::Result<()> {
        let id = self.id;
        self.worker.call(move |session| {
            session.connection_id.store(id, Ordering::Relaxed);
            session.SetInRestrictedSQL(true);
            query(session, "SET autocommit = 1").map(|_| ())
        })
    }
    fn on_resign_owner(&mut self) -> sys::Result<()> {
        Ok(())
    }
    fn has_pending_transaction(&self) -> bool {
        self.worker
            .call(|session| Ok(session.state.borrow().transaction.is_some()))
            .unwrap_or(true)
    }
    fn rollback_transaction(&mut self) -> sys::Result<()> {
        self.worker
            .call(|session| query(session, "ROLLBACK").map(|_| ()))
    }
    fn reset_state(&mut self) -> sys::Result<()> {
        self.worker
            .call(|session| session.reset_connection().map_err(sys_error))
    }
    fn register_internal_session(&mut self) -> bool {
        // The enclosing DDL pool registers this real, stable ID on Get.
        ddl::internal_session_ids().contains(&self.id)
    }
    fn unregister_internal_session(&mut self) {}
    fn execute(&mut self, sql: &str) -> sys::Result<Vec<sys::RecordSet>> {
        let sql = sql.to_owned();
        self.worker
            .call(move |session| Ok(vec![Box::new(query(session, &sql)?) as sys::RecordSet]))
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[sys::SqlValue],
    ) -> sys::Result<sys::RecordSet> {
        let sql = bound_sql(sql, args)?;
        self.worker
            .call(move |session| Ok(Box::new(query(session, &sql)?) as sys::RecordSet))
    }
    fn execute_statement(&mut self, statement: &dyn Any) -> sys::Result<sys::RecordSet> {
        self.execute_internal(
            statement
                .downcast_ref::<String>()
                .ok_or_else(|| sys_error("invalid system SQL statement"))?,
            &[],
        )
    }
    fn parse_with_params(
        &mut self,
        sql: &str,
        args: &[sys::SqlValue],
    ) -> sys::Result<sys::Statement> {
        let sql = bound_sql(sql, args)?;
        self.worker.call(move |_| {
            super::parse(&sql).map_err(sys_error)?;
            Ok(Box::new(sql) as sys::Statement)
        })
    }
    fn exec_restricted_statement(&mut self, statement: &dyn Any) -> sys::Result<Vec<sys::Row>> {
        self.exec_restricted_sql(
            statement
                .downcast_ref::<String>()
                .ok_or_else(|| sys_error("invalid system SQL statement"))?,
            &[],
        )
    }
    fn exec_restricted_sql(
        &mut self,
        sql: &str,
        args: &[sys::SqlValue],
    ) -> sys::Result<Vec<sys::Row>> {
        let rows = self
            .execute_internal(sql, args)?
            .downcast::<Vec<Vec<String>>>()
            .map_err(|_| sys_error("invalid system rows"))?;
        Ok(rows
            .into_iter()
            .map(|row| Box::new(row) as sys::Row)
            .collect())
    }
}

struct ConcreteDdlContext {
    closed: AtomicBool,
    id: u64,
    session: Arc<sys::Session>,
    variables: Arc<ddl::SessionVariables>,
}
impl ConcreteDdlContext {
    fn call<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut ConcreteSession) -> sys::Result<R> + Send + 'static,
    ) -> sys::Result<R> {
        self.session.WithSessionContext(move |context| {
            let concrete = context
                .as_any_mut()
                .and_then(|context| context.downcast_mut::<ConcreteSystemContext>())
                .ok_or_else(|| sys_error("invalid ConcreteSession adapter"))?;
            concrete.worker.call(operation)
        })
    }
}
struct Rows(Vec<ddl::Row>);
impl ddl::RecordSet for Rows {
    fn drain(&mut self, _: usize) -> Result<Vec<ddl::Row>, ddl::SessionError> {
        Ok(std::mem::take(&mut self.0))
    }
    fn close(&mut self) -> Result<(), ddl::SessionError> {
        self.0.clear();
        Ok(())
    }
}
impl ddl::SessionContext for ConcreteDdlContext {
    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
    fn session_id(&self) -> u64 {
        self.id
    }
    fn session_variables(&self) -> Arc<ddl::SessionVariables> {
        Arc::clone(&self.variables)
    }
    fn enter_new_transaction(&self, mode: ddl::TransactionMode) -> Result<(), ddl::SessionError> {
        self.call(move |session| {
            query(
                session,
                if mode == ddl::TransactionMode::Pessimistic {
                    "BEGIN PESSIMISTIC"
                } else {
                    "BEGIN OPTIMISTIC"
                },
            )
            .map(|_| ())
        })
        .map_err(ddl_error)
    }
    fn statement_commit(&self, _: &ddl::ExecutionContext) {
        // ConcreteSession writes directly to the KV transaction mem-buffer;
        // there is no separate statement buffer to flush (see canonical StmtCommit).
    }
    fn commit_transaction(&self, _: &ddl::ExecutionContext) -> Result<(), ddl::SessionError> {
        self.call(|session| query(session, "COMMIT").map(|_| ()))
            .map_err(ddl_error)?;
        self.variables.set_in_transaction(false);
        Ok(())
    }
    fn transaction(&self, activate: bool) -> Result<Option<ddl::Transaction>, ddl::SessionError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.call(move |session| {
            if activate && session.state.borrow().transaction.is_none() {
                query(session, "BEGIN")?;
            }
            Ok(session
                .state
                .borrow()
                .transaction
                .as_ref()
                .map(|txn| ddl::Transaction {
                    start_ts: txn.StartTS(),
                    valid: txn.Valid(),
                }))
        })
        .map_err(ddl_error)
    }
    fn statement_rollback(&self, _: &ddl::ExecutionContext, _: bool) {
        // The canonical SQL executor owns statement cleanup. The DDL wrapper
        // follows this hook with rollback_transaction for an abandoned job.
    }
    fn rollback_transaction(&self, _: &ddl::ExecutionContext) {
        if self
            .call(|session| query(session, "ROLLBACK").map(|_| ()))
            .is_err()
        {
            self.session.AvoidReuse();
        }
        self.variables.set_in_transaction(false);
    }
    fn execute_internal(
        &self,
        _: &ddl::ExecutionContext,
        sql: &str,
        args: &[ddl::SqlValue],
    ) -> Result<Option<Box<dyn ddl::RecordSet>>, ddl::SessionError> {
        let literals = args
            .iter()
            .map(|value| match value {
                ddl::SqlValue::Null => "NULL".into(),
                ddl::SqlValue::Integer(value) => value.to_string(),
                ddl::SqlValue::Unsigned(value) => value.to_string(),
                ddl::SqlValue::Bool(value) => u8::from(*value).to_string(),
                ddl::SqlValue::String(value) => {
                    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
                }
                ddl::SqlValue::Bytes(value) => format!(
                    "X'{}'",
                    value
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                ),
            })
            .collect::<Vec<_>>();
        let sql = super::bind_parameter_markers(sql, &literals).map_err(ddl_error)?;
        let rows = self
            .call(move |session| query(session, &sql))
            .map_err(ddl_error)?;
        Ok(Some(Box::new(Rows(
            rows.into_iter()
                .map(|row| ddl::Row {
                    values: row.into_iter().map(ddl::SqlValue::String).collect(),
                })
                .collect(),
        ))))
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.session.Close();
    }
}

struct ResourceState {
    closed: bool,
    borrowed: HashMap<u64, Weak<ConcreteDdlContext>>,
}
struct SystemResources {
    callbacks: SystemSessionCallbacks,
    pool: sys::AdvancedSessionPool,
    state: Mutex<ResourceState>,
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
impl Drop for SystemResources {
    fn drop(&mut self) {
        ddl::ResourcePool::close(self);
    }
}
impl ddl::ResourcePool for SystemResources {
    fn get(&self) -> Result<ddl::Resource, ddl::SessionError> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(ddl::SessionError::PoolClosed);
        }
        let session = Arc::new(self.pool.Get().map_err(ddl_error)?);
        let id = session
            .WithSessionContext(|context| {
                context
                    .as_any_mut()
                    .and_then(|context| context.downcast_mut::<ConcreteSystemContext>())
                    .map(|context| context.id)
                    .ok_or_else(|| sys_error("invalid system adapter"))
            })
            .map_err(ddl_error)?;
        let context = Arc::new(ConcreteDdlContext {
            closed: AtomicBool::new(false),
            id,
            session,
            variables: Arc::new(ddl::SessionVariables::default()),
        });
        state.borrowed.insert(context.id, Arc::downgrade(&context));
        (self.callbacks.borrowed)(context.clone());
        Ok(ddl::Resource::Session(context))
    }
    fn put(&self, context: Option<Arc<dyn ddl::SessionContext>>) {
        if let Some(context) = context {
            let concrete = context
                .as_any()
                .unwrap()
                .downcast_ref::<ConcreteDdlContext>()
                .unwrap();
            let mut state = self.state.lock().unwrap();
            (self.callbacks.returned)(concrete.id);
            state.borrowed.remove(&concrete.id);
            self.pool.Put(&concrete.session);
        }
    }
    fn destroy(&self, context: Arc<dyn ddl::SessionContext>) {
        (self.callbacks.destroyed)(context.session_id());
        self.state
            .lock()
            .unwrap()
            .borrowed
            .remove(&context.session_id());
        context.close();
    }
    fn kind(&self) -> ddl::ResourcePoolKind {
        ddl::ResourcePoolKind::Destroyable
    }
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        self.pool.Close();
        for context in state.borrowed.values().filter_map(Weak::upgrade) {
            (self.callbacks.destroyed)(context.id);
            ddl::SessionContext::close(context.as_ref());
        }
    }
}

/// Registration hooks supplied by the owning coordinator, in Go Get/Put/Destroy order.
/// The DDL pool also maintains its ordinary internal-session registry.
pub struct SystemSessionCallbacks {
    pub borrowed: Arc<dyn Fn(Arc<dyn ddl::SessionContext>) + Send + Sync>,
    pub returned: Arc<dyn Fn(u64) + Send + Sync>,
    pub destroyed: Arc<dyn Fn(u64) + Send + Sync>,
}
impl Default for SystemSessionCallbacks {
    fn default() -> Self {
        Self {
            borrowed: Arc::new(|_| {}),
            returned: Arc::new(|_| {}),
            destroyed: Arc::new(|_| {}),
        }
    }
}

/// A single common system pool, wrapped by the existing DDL pool.
/// Go's five-session capacity limits idle resources, not concurrent borrowers.
pub struct SystemSessionPool {
    pool: Arc<ddl::Pool>,
}
impl SystemSessionPool {
    pub fn new(domain: Arc<Domain>) -> Arc<Self> {
        Self::new_with_callbacks(domain, SystemSessionCallbacks::default())
    }
    pub fn new_with_callbacks(domain: Arc<Domain>, callbacks: SystemSessionCallbacks) -> Arc<Self> {
        Self::new_with_validator(domain, callbacks, None)
    }
    pub(crate) fn new_with_validator(
        domain: Arc<Domain>,
        callbacks: SystemSessionCallbacks,
        validator: Option<Arc<astersql_infoschema_isvalidator::Validator>>,
    ) -> Arc<Self> {
        let resources = Arc::new(SystemResources {
            callbacks,
            pool: sys::NewAdvancedSessionPool(5, move || {
                let domain = Arc::clone(&domain);
                let validator = validator.clone();
                Ok(Box::new(ConcreteSystemContext {
                    id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                    worker: sys::ThreadBoundSession::new(
                        move || {
                            let session = ConcreteSession::new(domain);
                            *session.schema_validator.borrow_mut() = validator;
                            Ok(session)
                        },
                        cleanup,
                    )?,
                }))
            }),
            state: Mutex::new(ResourceState {
                closed: false,
                borrowed: HashMap::new(),
            }),
        });
        Arc::new(Self {
            pool: Arc::new(ddl::Pool::new(resources)),
        })
    }
    pub fn acquire(&self) -> Result<SystemSessionLease, String> {
        self.acquire_with_cancellation(&sys::CancellationToken::default())
    }
    pub fn acquire_with_cancellation(
        &self,
        cancellation: &sys::CancellationToken,
    ) -> Result<SystemSessionLease, String> {
        if cancellation.is_cancelled() {
            return Err("system session acquisition cancelled".into());
        }
        let context = self.pool.get().map_err(|error| error.to_string())?;
        let lease = SystemSessionLease {
            pool: Arc::clone(&self.pool),
            context,
            metadata_error: None,
        };
        if cancellation.is_cancelled() {
            drop(lease);
            return Err("system session acquisition cancelled".into());
        }
        Ok(lease)
    }
    pub fn close(&self) {
        self.pool.close();
    }
}
impl astersql_domain_crossks::SessionPool for SystemSessionPool {
    fn close(&self) {
        self.close();
    }
}

pub struct SystemSessionLease {
    metadata_error: Option<String>,
    pool: Arc<ddl::Pool>,
    context: Arc<dyn ddl::SessionContext>,
}
impl SystemSessionLease {
    fn concrete(&self) -> &ConcreteDdlContext {
        self.context.as_any().unwrap().downcast_ref().unwrap()
    }
    pub fn session_id(&self) -> u64 {
        self.context.session_id()
    }
    pub fn query(&self, sql: impl Into<String>) -> Result<Vec<Vec<String>>, String> {
        self.query_with_label(sql.into(), "system")
    }
    fn query_with_label(&self, sql: String, label: &str) -> Result<Vec<Vec<String>>, String> {
        if let Some(error) = &self.metadata_error {
            return Err(error.clone());
        }
        let rows = ddl::Session::new(Arc::clone(&self.context))
            .execute(&ddl::ExecutionContext::default(), &sql, label, &[])
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .map(|row| {
                row.values
                    .into_iter()
                    .map(|value| {
                        if let ddl::SqlValue::String(value) = value {
                            value
                        } else {
                            unreachable!()
                        }
                    })
                    .collect()
            })
            .collect())
    }
}
impl Drop for SystemSessionLease {
    fn drop(&mut self) {
        // Cancellation and an abandoned transaction have the same cleanup path.
        self.context
            .rollback_transaction(&ddl::ExecutionContext::default());
        if self.pool.put(Arc::clone(&self.context)).is_err() {
            self.context.close();
            let _ = self.pool.destroy(Arc::clone(&self.context));
        }
    }
}

fn meta_key(name: &[u8]) -> kv::Key {
    kv::Key(codec::EncodeUint(
        codec::EncodeBytes(vec![b'm'], name),
        u64::from(b's'),
    ))
}
impl jobsubmit::Session for SystemSessionLease {
    fn begin(&mut self) -> Result<(), jobsubmit::Error> {
        ddl::Session::new(Arc::clone(&self.context))
            .begin_pessimistic(&ddl::ExecutionContext::default())
            .map_err(job_error)?;
        self.concrete()
            .call(|session| {
                let mut state = session.state.borrow_mut();
                let txn = state
                    .transaction
                    .as_mut()
                    .ok_or_else(|| sys_error("active transaction required"))?;
                txn.SetOption(kv::RequestSourceInternal, Some(Box::new(true)));
                txn.SetOption(
                    kv::RequestSourceType,
                    Some(Box::new(kv::InternalTxnDDL.to_owned())),
                );
                Ok(())
            })
            .map_err(job_error)
    }
    fn rollback(&mut self) {
        ddl::Session::new(Arc::clone(&self.context)).rollback();
    }
    fn commit(&mut self) -> Result<(), jobsubmit::Error> {
        if let Some(error) = &self.metadata_error {
            return Err(job_error(error));
        }
        ddl::Session::new(Arc::clone(&self.context))
            .commit(&ddl::ExecutionContext::default())
            .map_err(job_error)
    }
    fn read_bdr_role_and_start_ts(&mut self) -> Result<(String, u64), jobsubmit::Error> {
        self.concrete()
            .call(|session| {
                let context =
                    kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
                let read = |txn: &dyn kv::Transaction| -> Result<(String, u64), kv::Error> {
                    let role = match txn.Get(&context, meta_key(b"BDRRole"), &[]) {
                        Ok(value) => String::from_utf8(value.Value)
                            .map_err(|e| kv::errors::New(e.to_string()))?,
                        Err(error) if kv::IsErrNotFound(&error) => "none".into(),
                        Err(error) => return Err(error),
                    };
                    Ok((role, txn.StartTS()))
                };
                // Existing transaction affinity is preserved for callers already
                // in a transaction. SubmitBatch instead uses Go's separate,
                // retried metadata transaction before allocating IDs.
                if let Some(txn) = session.state.borrow().transaction.as_ref() {
                    return read(txn.as_ref()).map_err(sys_error);
                }
                let mut value = None;
                session
                    .domain
                    .storage_handle()
                    .with_storage(|store| {
                        kv::RunInNewTxn(&context, store, true, |_, txn| {
                            value = Some(read(txn)?);
                            Ok(())
                        })
                    })
                    .map_err(sys_error)?;
                value.ok_or_else(|| sys_error("BDR metadata transaction did not run"))
            })
            .map_err(job_error)
    }

    fn transaction_start_ts(&self) -> Result<u64, jobsubmit::Error> {
        self.context
            .transaction(false)
            .map_err(job_error)?
            .map(|txn| txn.start_ts)
            .ok_or_else(|| job_error("active transaction required"))
    }
    fn set_pessimistic(&mut self) {
        // begin() already entered the real pessimistic SQL/KV transaction.
    }
    fn current_version(&self) -> Result<u64, jobsubmit::Error> {
        self.concrete()
            .call(|session| {
                session
                    .domain
                    .storage_handle()
                    .with_storage(|store| store.CurrentVersion("global"))
                    .map(|version| version.Ver)
                    .map_err(sys_error)
            })
            .map_err(job_error)
    }
    fn lock_global_id_key(&mut self, for_update_ts: u64) -> Result<(), jobsubmit::Error> {
        self.concrete()
            .call(move |session| {
                let mut state = session.state.borrow_mut();
                let wait_timeout =
                    i64::try_from(state.innodb_lock_wait_timeout_secs.saturating_mul(1000))
                        .unwrap_or(i64::MAX);
                let txn = state
                    .transaction
                    .as_mut()
                    .ok_or_else(|| sys_error("active transaction required"))?;
                txn.SetOption(kv::SnapshotTS, Some(Box::new(for_update_ts)));
                Ok(txn
                    .LockKeys(
                        &kv::Context::default(),
                        &mut kv::LockCtx {
                            WaitTimeoutMs: wait_timeout,
                            ..Default::default()
                        },
                        &[meta_key(b"NextGlobalID")],
                    )
                    .map_err(job_kv_error))
            })
            .map_err(job_error)?
    }
    fn set_snapshot_ts(&mut self, timestamp: u64) {
        let result = self.concrete().call(move |session| {
            session
                .state
                .borrow_mut()
                .transaction
                .as_mut()
                .ok_or_else(|| sys_error("active transaction required"))?
                .SetOption(kv::SnapshotTS, Some(Box::new(timestamp)));
            Ok(())
        });
        if let Err(error) = result {
            // The jobsubmit ABI cannot return this error here. Preserve it for
            // the next SQL/allocation/commit, and prevent returning a dirty resource.
            self.metadata_error = Some(error.to_string());
            self.concrete().session.AvoidReuse();
        }
    }
    fn generate_global_ids(&mut self, count: usize) -> Result<Vec<i64>, jobsubmit::Error> {
        if let Some(error) = &self.metadata_error {
            return Err(job_error(error));
        }
        let count = i64::try_from(count).map_err(job_error)?;
        self.concrete()
            .call(move |session| {
                let mut state = session.state.borrow_mut();
                let txn = state
                    .transaction
                    .as_mut()
                    .ok_or_else(|| sys_error("active transaction required"))?;
                let last = kv::IncInt64(txn.as_mut(), &meta_key(b"NextGlobalID"), count)
                    .map_err(sys_error)?;
                Ok((last - count + 1..=last).collect())
            })
            .map_err(job_error)
    }
    fn execute(&mut self, sql: &str, label: &str) -> Result<(), jobsubmit::Error> {
        self.query_with_label(sql.to_owned(), label)
            .map(|_| ())
            .map_err(job_error)
    }
}
impl systable::Session for SystemSessionLease {
    fn execute(
        &mut self,
        _: &systable::Context,
        sql: &str,
        label: &str,
    ) -> Result<Vec<systable::Row>, systable::Error> {
        self.query_with_label(sql.to_owned(), label)
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
            .map_err(systable::Error::Execute)
    }
}
impl jobsubmit::SessionPool for SystemSessionPool {
    fn get(&self) -> Result<Box<dyn jobsubmit::Session>, jobsubmit::Error> {
        self.acquire()
            .map(|lease| Box::new(lease) as Box<dyn jobsubmit::Session>)
            .map_err(job_error)
    }
    fn put(&self, session: Box<dyn jobsubmit::Session>) {
        drop(session);
    }
}
impl systable::SessionPool for SystemSessionPool {
    fn get(&self) -> Result<Box<dyn systable::Session>, systable::Error> {
        self.acquire()
            .map(|lease| Box::new(lease) as Box<dyn systable::Session>)
            .map_err(systable::Error::Pool)
    }
    fn put(&self, session: Box<dyn systable::Session>) {
        drop(session);
    }
}

impl astersql_infoschema_issyncer::MDLSessionPool for SystemSessionPool {
    fn ReadMDLRows(
        &self,
        min_job: i64,
        version: i64,
    ) -> Result<
        HashMap<i64, astersql_infoschema_issyncer::JobMDL>,
        astersql_infoschema_issyncer::SyncError,
    > {
        let lease = self
            .acquire()
            .map_err(astersql_infoschema_issyncer::SyncError)?;
        if let Err(error) = lease.query("rollback") {
            // Go closes this borrowed resource on rollback failure. Mark it
            // unusable so the lease's existing return path destroys it.
            lease.concrete().session.AvoidReuse();
            return Err(astersql_infoschema_issyncer::SyncError(error));
        }
        let rows = lease.query(format!("select job_id, version, table_ids from mysql.tidb_mdl_info where job_id >= {min_job} and version <= {version}"))
            .map_err(astersql_infoschema_issyncer::SyncError)?;
        rows.into_iter()
            .map(|row| {
                let invalid =
                    || astersql_infoschema_issyncer::SyncError("invalid tidb_mdl_info row".into());
                if row.len() != 3 {
                    return Err(invalid());
                }
                let id = row[0].parse().map_err(|_| invalid())?;
                let ver = row[1].parse().map_err(|_| invalid())?;
                let tables = row[2]
                    .split(',')
                    .map(|value| value.parse::<i64>().unwrap_or(0))
                    .collect();
                Ok((
                    id,
                    astersql_infoschema_issyncer::JobMDL {
                        Ver: ver,
                        TableIDs: tables,
                    },
                ))
            })
            .collect()
    }
}

struct TableModeBdrPolicy;
impl jobsubmit::BdrPolicy for TableModeBdrPolicy {
    fn is_denied(&self, role: &str, tp: jobsubmit::JobType, _: &jobsubmit::JobArgs) -> bool {
        // These options are exclusively for BuildAlterTableModeJob.
        if tp != jobsubmit::JobType::AlterTableMode {
            return true;
        }
        let role = match role {
            "primary" => astersql_ddl_bdr::ast::BDRRole::Primary,
            "secondary" => astersql_ddl_bdr::ast::BDRRole::Secondary,
            "none" | "" => astersql_ddl_bdr::ast::BDRRole::None,
            _ => astersql_ddl_bdr::ast::BDRRole::Unknown,
        };
        astersql_ddl_bdr::IsDenied(role, tp.code() as u8, None)
    }
}
struct SubmitGuard(Arc<dyn systable::Manager>);
impl jobsubmit::SystemTableManager for SubmitGuard {
    fn has_flashback_cluster_job(&self, min_id: i64) -> Result<bool, jobsubmit::Error> {
        self.0
            .has_flashback_cluster_job(&systable::Context::default(), min_id)
            .map_err(job_error)
    }
}
struct SubmitMinId(Arc<systable::MinJobIdRefresher>);
impl jobsubmit::MinJobIdProvider for SubmitMinId {
    fn current_min_job_id(&self) -> i64 {
        self.0.current_min_job_id()
    }
}
impl SystemSessionPool {
    /// Construct Go table-mode submission dependencies without starting an owner.
    /// The caller manages the shared MinJobID refresh loop and serverstate lifecycle.
    pub fn table_mode_submit_options(
        self: &Arc<Self>,
        manager: Arc<dyn systable::Manager>,
        min_id: Arc<systable::MinJobIdRefresher>,
        state: Option<Arc<dyn jobsubmit::ServerState>>,
    ) -> jobsubmit::SubmitOptions {
        jobsubmit::SubmitOptions {
            session_pool: self.clone(),
            system_table_manager: Arc::new(SubmitGuard(manager)),
            min_job_id_provider: Arc::new(SubmitMinId(min_id)),
            server_state: state,
            bdr_policy: Arc::new(TableModeBdrPolicy),
            before_insert_with_assigned_ids: None,
            max_retry_count: kv::MaxRetryCnt.load(Ordering::Relaxed) as usize,
            backoff: Arc::new(|attempt| {
                kv::BackOff(u32::try_from(attempt).unwrap_or(u32::MAX));
            }),
        }
    }
}
impl SystemSessionLease {
    /// Read the real pooled session's variables, including the CDC bypass source.
    pub fn ddl_session_variables(
        &self,
    ) -> Result<astersql_domain_crossks::SessionVariables, String> {
        self.concrete()
            .call(|session| {
                let sql_mode =
                    astersql_parser_mysql::r#const::GetSQLMode(&session.state.borrow().sql_mode)
                        .map_err(sys_error)?;
                let source = session
                    .session_vars
                    .GetHintSystemVar("tidb_cdc_write_source")
                    .map_err(sys_error)?;
                Ok(astersql_domain_crossks::SessionVariables {
                    cdc_write_source: source.parse::<u64>().map_err(sys_error)?,
                    sql_mode: sql_mode.0 as u64,
                })
            })
            .map_err(|e| e.to_string())
    }
}

/// Jobsubmit uses the synchronously refreshed cache of the public state syncer.
pub struct JobSubmitServerState(pub Arc<dyn astersql_ddl_serverstate::Syncer>);
impl jobsubmit::ServerState for JobSubmitServerState {
    fn is_upgrading(&self) -> bool {
        self.0.is_upgrading_state()
    }
}

struct ConcreteJobExecutionContext<'a>(&'a mut ConcreteSession);
impl astersql_ddl::job_worker::JobExecutionContext for ConcreteJobExecutionContext<'_> {
    fn query(&mut self, sql: &str, _: &str) -> Result<Vec<Vec<String>>, String> {
        // A handler may read/write system rows, but transaction boundaries and
        // implicit-commit DDL belong exclusively to the enclosing JobWorker.
        let statements = super::parse(sql).map_err(|e| e.to_string())?;
        if statements.len() != 1
            || statements.iter().any(|statement| {
                let node = statement.as_any();
                !node.is::<super::ast::SelectStmt>()
                    && !node.is::<super::ast::SetOprStmt>()
                    && !node.is::<super::ast::InsertStmt>()
                    && !node.is::<super::ast::UpdateStmt>()
                    && !node.is::<super::ast::DeleteStmt>()
            })
        {
            return Err("DDL execution context requires one transactional DML statement".into());
        }
        query(self.0, sql).map_err(|e| e.to_string())
    }
    fn with_transaction(
        &mut self,
        operation: &mut dyn FnMut(&mut dyn kv::Transaction) -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        let mut state = self.0.state.borrow_mut();
        let txn = state
            .transaction
            .as_mut()
            .ok_or("active transaction required")?;
        operation(txn.as_mut())
    }
}

impl astersql_ddl::job_worker::DurableJobSession for SystemSessionLease {
    fn with_execution_context(
        &mut self,
        operation: astersql_ddl::job_worker::ExecutionOperation,
    ) -> Result<Vec<u8>, String> {
        self.concrete()
            .call(move |session| {
                if session.state.borrow().transaction.is_none() {
                    return Err(sys_error("active transaction required"));
                }
                operation(&mut ConcreteJobExecutionContext(session)).map_err(sys_error)
            })
            .map_err(|e| e.to_string())
    }
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        self.query_with_label(sql.to_owned(), label)
    }
    fn begin(&mut self) -> Result<(), String> {
        ddl::Session::new(Arc::clone(&self.context))
            .begin(&ddl::ExecutionContext::default())
            .map_err(|e| e.to_string())?;
        self.with_transaction(Box::new(|_| Ok(Vec::new())))
            .map(|_| ())
    }
    fn commit(&mut self) -> Result<(), String> {
        if let Some(error) = &self.metadata_error {
            return Err(error.clone());
        }
        self.concrete()
            .call(|session| session.commit_ddl_transaction().map_err(sys_error))
            .map_err(|e| e.to_string())?;
        self.concrete().variables.set_in_transaction(false);
        Ok(())
    }

    fn rollback(&mut self) {
        ddl::Session::new(Arc::clone(&self.context)).rollback();
    }
    fn with_transaction(
        &mut self,
        operation: astersql_ddl::job_worker::TransactionOperation,
    ) -> Result<Vec<u8>, String> {
        self.concrete()
            .call(move |session| {
                let mut state = session.state.borrow_mut();
                let transaction = state
                    .transaction
                    .as_mut()
                    .ok_or_else(|| sys_error("active transaction required"))?;
                transaction.SetOption(kv::RequestSourceInternal, Some(Box::new(true)));
                transaction.SetOption(
                    kv::RequestSourceType,
                    Some(Box::new(kv::InternalTxnDDL.to_owned())),
                );
                operation(transaction.as_mut()).map_err(sys_error)
            })
            .map_err(|e| e.to_string())
    }
}

/// The normal owner's live election state and scheduler lifetime. This adapter
/// does not campaign or create a cross-keyspace owner; the normal DDL owns both.
pub struct DdlOwnerLease {
    pub owner: Arc<dyn astersql_owner::Manager>,
    pub cancellation: Arc<sys::CancellationToken>,
}
impl astersql_ddl::job_worker::JobLease for DdlOwnerLease {
    fn is_owner(&self) -> bool {
        self.owner.IsOwner()
    }
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// Shared transaction MDL state of the real thread-bound system session.
pub(crate) fn transaction_mdl(
    context: &dyn ddl::SessionContext,
) -> Option<Arc<astersql_session_sessmgr::TransactionMDL>> {
    context
        .as_any()?
        .downcast_ref::<ConcreteDdlContext>()?
        .call(|session| Ok(session.transaction_mdl()))
        .ok()
}
