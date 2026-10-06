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
// The DDL string-row ABI carries reorg key columns as hex. Decode the
// runtime's binary representation before converting, preserving non-UTF8 keys.
fn query_reorg(session: &ConcreteSession, sql: &str) -> sys::Result<Vec<Vec<String>>> {
    if !sql.starts_with("select ele_id,HEX(ele_type),HEX(start_key),HEX(end_key),physical_id from mysql.tidb_ddl_reorg") {
        return query(session,sql);
    }
    let sql = sql
        .replace("HEX(ele_type)", "ele_type")
        .replace("HEX(start_key)", "start_key")
        .replace("HEX(end_key)", "end_key");
    let mut rows = query(session, &sql)?;
    for row in &mut rows {
        for value in row.iter_mut().take(4).skip(1) {
            let bytes = super::row_codec::binary_runtime_bytes(value)
                .unwrap_or_else(|| value.as_bytes().to_vec());
            *value = bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
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

struct AnalyzeProgress {
    start: std::time::Instant,
    timeout: std::time::Duration,
    result: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}
#[derive(Default)]
struct MViewBuildContexts {
    owner_epoch: u64,
    completed: HashMap<i64, (u64, i64)>,
    index_cloud_uris: HashMap<i64, String>,
    analyzes: HashMap<i64, AnalyzeProgress>,
    dxf_worker: Option<Arc<super::modify_column_dist_backfill::NodeService>>,
}
struct ConcreteDdlContext {
    mview_builds: Arc<Mutex<MViewBuildContexts>>,
    storage_class_transitions:
        Arc<astersql_ddl::storage_class_transition::StorageClassTransitionManager>,
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
            .call(move |session| query_reorg(session, &sql))
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
    mview_builds: Arc<Mutex<MViewBuildContexts>>,
    storage_class_transitions:
        Arc<astersql_ddl::storage_class_transition::StorageClassTransitionManager>,
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
            mview_builds: self.mview_builds.clone(),
            storage_class_transitions: self.storage_class_transitions.clone(),
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
    builds: Arc<Mutex<MViewBuildContexts>>,
    storage_class_transitions:
        Arc<astersql_ddl::storage_class_transition::StorageClassTransitionManager>,
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
            mview_builds: Arc::new(Mutex::new(MViewBuildContexts::default())),
            storage_class_transitions: Arc::new(
                astersql_ddl::storage_class_transition::StorageClassTransitionManager::default(),
            ),
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
            builds: resources.mview_builds.clone(),
            storage_class_transitions: resources.storage_class_transitions.clone(),
            pool: Arc::new(ddl::Pool::new(resources)),
        })
    }
    pub(crate) fn storage_class_transition_manager(
        &self,
    ) -> &astersql_ddl::storage_class_transition::StorageClassTransitionManager {
        &self.storage_class_transitions
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
    pub fn start_dxf_worker(&self) -> Result<(), String> {
        if !astersql_sessionctx_vardef::EnableDistTask.Load() {
            return Ok(());
        }
        let lease = self.acquire()?;
        let builds = self.builds.clone();
        lease
            .concrete()
            .call(move |session| {
                let mut contexts = builds.lock().map_err(sys_error)?;
                if contexts.dxf_worker.is_none() {
                    contexts.dxf_worker = Some(
                        super::modify_column_dist_backfill::NodeService::start(session)
                            .map_err(sys_error)?,
                    );
                }
                Ok(())
            })
            .map_err(|e| e.to_string())
    }
    pub fn stop_dxf_worker(&self) {
        let worker = self.builds.lock().unwrap().dxf_worker.take();
        if let Some(worker) = worker {
            worker.stop();
        }
    }
    pub fn close(&self) {
        self.stop_dxf_worker();
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
    pub(super) fn persistent_history(
        &self,
        id: i64,
    ) -> Result<Option<astersql_meta_model::group_3::Job>, String> {
        self.concrete()
            .call(move |session| {
                let snapshot = session
                    .domain
                    .storage_handle()
                    .with_storage(|store| {
                        let version = store.CurrentVersion("global")?;
                        Ok::<_, kv::Error>(store.GetSnapshot(version))
                    })
                    .map_err(sys_error)?;
                astersql_meta::SnapshotReader::new(snapshot)
                    .get_history_ddl_job(id)
                    .map_err(sys_error)
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(test)]
    pub(super) fn index_cloud_storage_uri_for_test(&self, job_id: i64) -> Option<String> {
        self.concrete()
            .mview_builds
            .lock()
            .unwrap()
            .index_cloud_uris
            .get(&job_id)
            .cloned()
    }
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

#[path = "create_table_resources.rs"]
mod create_table_resources;

struct ConcreteJobExecutionContext<'a>(
    &'a mut ConcreteSession,
    Arc<ddl::Pool>,
    Arc<Mutex<MViewBuildContexts>>,
    Arc<astersql_ddl::storage_class_transition::StorageClassTransitionManager>,
);
impl astersql_ddl::index::ReorgIndexEnvironment for ConcreteJobExecutionContext<'_> {
    fn load_cloud_storage_uri(&mut self, job_id: i64) -> Result<String, String> {
        let configured = astersql_sessionctx_vardef::CloudStorageURI.Load();
        let uri = astersql_ddl::index::resolve_cloud_storage_uri(
            &configured,
            astersql_util_sem_compat::IsEnabled(),
            || {
                self.0
                    .domain
                    .storage_handle()
                    .with_storage(|store| Some(store.GetClusterID()))
            },
        );
        self.2
            .lock()
            .map_err(|_| "index reorg context poisoned")?
            .index_cloud_uris
            .insert(job_id, uri.clone());
        Ok(uri)
    }
    fn after_load_cloud_storage_uri(&mut self, _: &mut astersql_meta_model::group_3::Job) {
        astersql_testkit_testfailpoint::inject(
            "github.com/pingcap/tidb/pkg/ddl/afterLoadCloudStorageURI",
        );
    }
    fn ingest_initialized(&self) -> bool {
        astersql_ddl::index::initialized_disk_root().is_some()
    }
    fn pre_check_ingest_disk(&mut self) -> Result<(), String> {
        astersql_ddl::index::initialized_disk_root()
            .ok_or("ingest environment is not initialized")?
            .pre_check_usage()
    }
}
impl astersql_ddl::job_worker::JobExecutionContext for ConcreteJobExecutionContext<'_> {
    fn cached_storage_class_observation(
        &mut self,
        operation: &astersql_ddl::storage_class_transition::StorageClassTransitionOperation,
    ) -> Option<astersql_ddl::storage_class_transition::StorageClassTransitionStatus> {
        self.3.cached_observation(operation)
    }
    fn reorg_index_environment(
        &mut self,
    ) -> Result<&mut dyn astersql_ddl::index::ReorgIndexEnvironment, String> {
        Ok(self)
    }

    fn build_create_mview_data(
        &mut self,
        job: &mut astersql_meta_model::group_3::Job,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<(u64, i64), String> {
        if let Some(result) = self
            .2
            .lock()
            .map_err(|_| "materialized view reorg context poisoned")?
            .completed
            .get(&job.id)
            .copied()
        {
            return Ok(result);
        }
        let job = astersql_meta_model::group_3::Job::decode(
            &job.encode(false).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let table = table.clone();
        let context = self.1.get().map_err(|e| e.to_string())?;
        let independent = SystemSessionLease {
            metadata_error: None,
            pool: self.1.clone(),
            context,
        };
        let result = independent.concrete().call(move |session| {
            let info = table.MaterializedView.as_ref().ok_or_else(||sys_error("create materialized view: invalid metadata"))?;
            let reorg = job.reorg_meta.as_ref().ok_or_else(||sys_error("create materialized view: missing reorg metadata"))?;
            if info.SQLContent.is_empty() { return Err(sys_error("create materialized view: invalid select sql")); }
            let schema = job.schema_name.replace('`',"``");
            let name = table.Name.O.replace('`',"``");
            if !query(session,&format!("SELECT 1 FROM `{schema}`.`{name}` LIMIT 1"))?.is_empty() { return Err(sys_error("create materialized view: detected residual build rows on retry")); }
            let mode=astersql_parser_mysql::r#const::Str2SQLMode.iter().filter(|(_,flag)| flag.0 != 0 && (reorg.SQLMode & flag.0 as u64) == flag.0 as u64).map(|(name,_)|*name).collect::<Vec<_>>().join(",");
            let old_db=session.state.borrow().current_database.clone();
            let old_mode=session.state.borrow().sql_mode.clone();
            let old_tz=session.time_zone.borrow().clone();
            let mut restore=Vec::new();
            let result = (|| {
                session.state.borrow_mut().current_database=job.schema_name.clone();
                session.state.borrow_mut().sql_mode=mode;
                if let Some(location)=&reorg.Location {
                    let zone=if location.name.is_empty() { format!("{}{:02}:{:02}",if location.offset<0{"-"}else{"+"},location.offset.abs()/3600,(location.offset.abs()%3600)/60) } else {location.name.clone()};
                    *session.time_zone.borrow_mut()=super::session::RuntimeTimeZone::parse(&zone).ok_or_else(||sys_error("invalid build timezone"))?;
                }
                for (name,value) in &job.session_vars {
                    let target=match name.as_str() {
                        "tidb_mview_maintain_mem_quota" => "tidb_mem_quota_query",
                        "tidb_mview_maintain_isolation_read_engines" => "tidb_isolation_read_engines",
                        "tidb_mview_maintain_import_threads" | "tidb_mview_maintain_import_disk_quota" => continue,
                        _ => name,
                    };
                    let old=session.session_vars.SetHintSystemVarWithOldState(target,value).map_err(sys_error)?;
                    restore.push((target.to_owned(),old));
                }
                // One SQL statement observes a real transaction snapshot. Keeping
                // that transaction explicit exposes the same query start TSO even
                // for empty input; physical IMPORT remains independent of it.
                query(session,"BEGIN")?;
                let read_ts=session.state.borrow().transaction.as_ref().ok_or_else(||sys_error("build transaction missing"))?.StartTS();
                let store_name=session.domain.storage().with_storage(|store|store.Name());
                let sql=if store_name == "TiKV" {
                    let threads=job.session_vars.get("tidb_mview_maintain_import_threads").map(String::as_str).unwrap_or("1");
                    let quota=job.session_vars.get("tidb_mview_maintain_import_disk_quota").map(String::as_str).unwrap_or("50GiB").replace('\'',"''");
                    format!("IMPORT INTO `{schema}`.`{name}` FROM ({}) WITH disable_precheck, thread={threads}, disk_quota='{quota}'",info.SQLContent)
                } else { format!("REPLACE INTO `{schema}`.`{name}` {}",info.SQLContent) };
                let _precision=super::relational_value::BuildDivisionPrecision::enter(info.DefinitionDivPrecisionIncrement).map_err(sys_error)?;
                query(session, &sql)?;
                query(session,"COMMIT")?;
                if read_ts==0 { return Err(sys_error("create materialized view: invalid build read tso")); }
                let count=query(session,&format!("SELECT COUNT(*) FROM `{schema}`.`{name}`"))?.first().and_then(|r|r.first()).ok_or_else(||sys_error("build row count missing"))?.parse::<i64>().map_err(sys_error)?;
                Ok((read_ts,count))
            })();
            if result.is_err(){ let _=query(session,"ROLLBACK"); }
            session.state.borrow_mut().current_database=old_db;
            session.state.borrow_mut().sql_mode=old_mode;
            *session.time_zone.borrow_mut()=old_tz;
            for (name,value) in restore.into_iter().rev() { session.session_vars.SetHintSystemVarWithOldState(&name,&value).map_err(sys_error)?; }
            result
        }).map_err(|e|e.to_string());
        if let Ok(result) = result {
            self.2
                .lock()
                .map_err(|_| "materialized view reorg context poisoned")?
                .completed
                .insert(job.id, result);
        }
        result
    }

    fn prewrite_create_mview_refresh(&mut self, id: i64) -> Result<u64, String> {
        let context = self.1.get().map_err(|e| e.to_string())?;
        let mut independent = SystemSessionLease {
            metadata_error: None,
            pool: self.1.clone(),
            context,
        };
        use astersql_ddl::job_worker::DurableJobSession;
        independent.begin()?;
        let result = (|| {
            independent.query(format!(
                "SELECT 1 FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={id} LIMIT 1"
            ))?;
            let bytes = independent
                .with_transaction(Box::new(|txn| Ok(txn.StartTS().to_le_bytes().to_vec())))?;
            let start =
                u64::from_le_bytes(bytes.try_into().map_err(|_| "invalid init refresh tso")?);
            if start == 0 {
                return Err("create materialized view: invalid init refresh tso".into());
            }
            independent.query(format!("INSERT INTO mysql.tidb_mview_refresh_info (MVIEW_ID,LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS) VALUES ({id},{start},NULL) ON DUPLICATE KEY UPDATE LAST_SUCCESS_READ_TSO=VALUES(LAST_SUCCESS_READ_TSO),LAST_SUCCESS_REFRESH_END_UNIX_SECONDS=VALUES(LAST_SUCCESS_REFRESH_END_UNIX_SECONDS)"))?;
            independent.commit()?;
            Ok(start)
        })();
        if result.is_err() {
            independent.rollback();
        }
        result.map_err(|e: String| {
            if (e.contains("1146") && e.contains("tidb_mview_refresh_info")) || e.contains("unknown DML table tidb_mview_refresh_info") {
                astersql_util_dbterror::ErrInvalidDDLJob.GenWithStackByArgs(&["create materialized view: required system table mysql.tidb_mview_refresh_info does not exist".into()]).to_string()
            } else { e }
        })
    }

    fn migrate_mview_refresh_info(
        &mut self,
        args: &astersql_meta_model::group_2::RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs,
    ) -> Result<(), String> {
        let rows = self.query(
            &format!(
                "SELECT IFNULL(CAST(LAST_SUCCESS_READ_TSO AS CHAR), 'NULL') FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                args.OldMViewID
            ),
            "mview-refresh-cutover-read-refresh-info",
        )?;
        if rows.len() != 1 {
            return Err("[ddl:8204]refresh materialized view complete OUT OF PLACE cutover: refresh info row missing in mysql.tidb_mview_refresh_info".into());
        }
        let observed = rows[0].first().map(String::as_str).unwrap_or("NULL");
        let stale = if args.ExpectedLastSuccessReadTSONull {
            observed != "NULL"
        } else {
            observed == "NULL"
                || observed.parse::<u64>().ok() != Some(args.ExpectedLastSuccessReadTSO)
        };
        if stale {
            return Err("[ddl:8204]refresh materialized view complete OUT OF PLACE cutover: stale LAST_SUCCESS_READ_TSO detected before cutover".into());
        }
        let next = if args.ShouldUpdateNextRefreshUnixSeconds {
            args.NextRefreshUnixSeconds
                .map_or_else(|| "NULL".to_owned(), |v| v.to_string())
        } else {
            "NEXT_REFRESH_UNIX_SECONDS".to_owned()
        };
        self.query(
            &format!(
                "UPDATE mysql.tidb_mview_refresh_info SET MVIEW_ID={}, LAST_SUCCESS_READ_TSO={}, LAST_SUCCESS_REFRESH_END_UNIX_SECONDS=UNIX_TIMESTAMP(), NEXT_REFRESH_UNIX_SECONDS={} WHERE MVIEW_ID={}",
                args.ShadowTableID, args.BuildReadTSO, next, args.OldMViewID
            ),
            "mview-refresh-cutover-update-refresh-info",
        )?;
        Ok(())
    }

    fn finish_create_mview_refresh(
        &mut self,
        _schema: &str,
        table: &astersql_meta_model::TableInfo,
        read_ts: u64,
    ) -> Result<(), String> {
        let info = table
            .MaterializedView
            .as_ref()
            .ok_or("create materialized view: invalid metadata")?;
        let evaluate = |expression: &str| -> Result<Option<i64>, String> {
            if expression.trim().is_empty() {
                return Ok(None);
            }
            let mut parser = astersql_parser::New();
            let statement = parser
                .ParseOneStmt(&format!("select {expression}"), "utf8mb4", "utf8mb4_bin")
                .map_err(|error| error.to_string())?;
            let select = statement
                .as_any()
                .downcast_ref::<super::ast::SelectStmt>()
                .ok_or("materialized view schedule is not a scalar expression")?;
            let expression = select
                .Fields
                .Fields
                .first()
                .and_then(|field| field.Expr.as_ref())
                .ok_or("materialized view schedule has no expression")?;
            super::mview_ddl::mlog_schedule_unix_seconds_with_mode(
                expression,
                info.RefreshScheduleSQLMode,
            )
            .map_err(|error| error.to_string())
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs() as i64;
        let start = evaluate(&info.RefreshStartWith)?;
        let next = evaluate(&info.RefreshNext)?;
        let scheduled = match (start, info.RefreshNext.trim().is_empty()) {
            (Some(start), false) if start < now.saturating_add(10) => next,
            (Some(start), _) => Some(start),
            (None, _) => next,
        };
        let next_sql = scheduled.map_or("NULL".to_owned(), |value| value.to_string());
        self.query(
            &format!("INSERT INTO mysql.tidb_mview_refresh_info (MVIEW_ID,LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS,NEXT_REFRESH_UNIX_SECONDS) VALUES ({},{},{},{}) ON DUPLICATE KEY UPDATE LAST_SUCCESS_READ_TSO=VALUES(LAST_SUCCESS_READ_TSO),LAST_SUCCESS_REFRESH_END_UNIX_SECONDS=VALUES(LAST_SUCCESS_REFRESH_END_UNIX_SECONDS),NEXT_REFRESH_UNIX_SECONDS=VALUES(NEXT_REFRESH_UNIX_SECONDS)", table.ID, read_ts, now, next_sql),
            "mview-refresh-info-upsert",
        )?;
        Ok(())
    }

    fn delete_create_mview_refresh(&mut self, id: i64) -> Result<(), String> {
        if let Err(error) = self.query(
            &format!("DELETE FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={id}"),
            "mview-refresh-info-delete",
        ) {
            if !error.contains("tidb_mview_refresh_info") {
                return Err(error);
            }
        }
        let _ = self.query(
            &format!("DELETE FROM mysql.tidb_mview_refresh_alert WHERE MVIEW_ID={id}"),
            "mview-refresh-alert-delete",
        );
        Ok(())
    }

    fn derive_create_mlog_schedule(
        &mut self,
        schema: &str,
        log: &astersql_meta_model::TableInfo,
    ) -> Result<(Option<i64>, bool), String> {
        let info = log
            .MaterializedViewLog
            .as_ref()
            .ok_or("materialized view log metadata missing")?;
        derive_create_mlog_schedule(schema, &log.Name.O, info)
    }

    fn masking_policy_timestamp(&mut self) -> Result<String, String> {
        Ok(chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string())
    }
    fn update_table_labels(
        &mut self,
        old_schema: &str,
        old_name: &str,
        new_schema: &str,
        table: &astersql_meta_model::TableInfo,
        delete_old: bool,
    ) -> Result<(), String> {
        create_table_resources::update_labels(
            &self.0.domain,
            old_schema,
            old_name,
            new_schema,
            table,
            delete_old,
        )
    }
    fn delete_drop_table_ttl(&mut self, table: i64) -> Result<(), String> {
        if let Some(manager) = self.0.domain.external_workload_manager() {
            manager
                .lock()
                .map_err(|e| e.to_string())?
                .DeleteTTLTableInfo(&astersql_extworkload::context::Background(), table)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn cleanup_drop_table_resources(
        &mut self,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        astersql_domain_infosync::DeleteTiFlashTableSyncProgress(table).map_err(|e| e.to_string())
    }
    fn delete_drop_table_affinity(
        &mut self,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        create_table_resources::delete_affinity(&self.0.domain, table)
    }
    fn drop_table_rule_ids(
        &mut self,
        schema: &str,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<Vec<String>, String> {
        let keyspace = self
            .0
            .domain
            .storage_handle()
            .with_storage(|s| s.DDLKeyspaceID())
            .map_err(|e| e.to_string())?;
        let prefix = if astersql_config_kerneltype::IsNextGen() && keyspace != u32::MAX {
            format!("keyspace/{keyspace}/schema/{schema}/{}", table.Name.L)
        } else {
            format!("schema/{schema}/{}", table.Name.L)
        };
        let mut rules: Vec<String> = table
            .GetPartitionInfo()
            .map(|p| {
                p.Definitions
                    .iter()
                    .map(|d| format!("{prefix}/{}", d.Name.L))
                    .collect()
            })
            .unwrap_or_default();
        rules.push(prefix);
        Ok(rules)
    }
    fn configure_create_table_replica(
        &mut self,
        t: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        create_table_resources::configure_replica(&self.0.domain, t)
    }
    fn put_create_table_bundles(
        &mut self,
        b: &[astersql_ddl_placement::Bundle],
    ) -> Result<(), String> {
        create_table_resources::put_bundles(&self.0.domain, b)
    }
    fn check_create_table_columnar(
        &mut self,
        t: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        create_table_resources::check_columnar(&self.0.domain, t)
    }
    fn create_table_affinity(&mut self, t: &astersql_meta_model::TableInfo) -> Result<(), String> {
        create_table_resources::create_affinity(&self.0.domain, t)
    }
    fn rebase_create_table_ids(
        &mut self,
        db: i64,
        t: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        create_table_resources::rebase_ids(&self.0.domain, db, t)
    }
    fn register_create_table_ttl(
        &mut self,
        t: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        if !t.TTLInfo.as_ref().is_some_and(|v| v.Enable) {
            return Ok(());
        }
        if let Some(m) = self.0.domain.external_workload_manager() {
            m.lock()
                .map_err(|e| e.to_string())?
                .RegisterTTLTableInfo(
                    &astersql_extworkload::context::Background(),
                    t.ID,
                    astersql_sessionctx_vardef::EnableTTLJob.Load(),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn backfill_modified_column(
        &mut self,
        table: &astersql_meta_model::TableInfo,
        old: &astersql_meta_model::ColumnInfo,
        new: &astersql_meta_model::ColumnInfo,
        physical: i64,
        start: &[u8],
        end: &[u8],
        limit: usize,
        mode: u64,
        location: Option<&astersql_meta_model::TimeZoneLocation>,
    ) -> Result<(Vec<u8>, i64), String> {
        super::modify_column_backfill::batch(
            self.0, table, old, new, physical, start, end, limit, mode, location,
        )
    }
    fn backfill_prepared_indexes(
        &mut self,
        request: astersql_ddl::backfilling::IndexBackfillBatch,
    ) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
        backfill_index_batch(self.0, request)
    }
    fn ingest_modified_indexes(
        &mut self,
        request: astersql_ddl::backfilling::IndexBackfillBatch,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
        if job.reorg_meta.as_ref().is_some_and(|meta| meta.IsDistReorg) {
            let worker = {
                let mut contexts = self.2.lock().map_err(|e| e.to_string())?;
                if contexts.dxf_worker.is_none() {
                    contexts.dxf_worker = Some(
                        super::modify_column_dist_backfill::NodeService::start(self.0)?,
                    );
                }
                contexts.dxf_worker.as_ref().unwrap().clone()
            };
            let cloud_storage_uri = if job
                .reorg_meta
                .as_ref()
                .is_some_and(|meta| meta.UseCloudStorage)
            {
                let cached = self
                    .2
                    .lock()
                    .map_err(|e| e.to_string())?
                    .index_cloud_uris
                    .get(&job.id)
                    .cloned()
                    .unwrap_or_default();
                if cached.is_empty() {
                    astersql_ddl::index::ReorgIndexEnvironment::load_cloud_storage_uri(
                        self, job.id,
                    )?
                } else {
                    cached
                }
            } else {
                String::new()
            };
            super::modify_column_dist_backfill::run(
                self.0,
                request,
                job,
                false,
                worker,
                cloud_storage_uri,
            )
        } else {
            backfill_index_batch_with_ingest(self.0, request, Some(job.id))
        }
    }
    fn analyze_modified_table(
        &mut self,
        job: &mut astersql_meta_model::group_3::Job,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<i8, String> {
        use astersql_meta_model::group_3::{
            AnalyzeStateDone, AnalyzeStateFailed, AnalyzeStateRunning, AnalyzeStateTimeout,
        };
        let mut contexts = self.2.lock().map_err(|_| "DDL analyze context poisoned")?;
        if !contexts.analyzes.contains_key(&job.id) {
            let since = (job.start_ts != 0)
                .then(|| chrono::DateTime::from_timestamp_millis((job.start_ts >> 18) as i64))
                .flatten()
                .unwrap_or_else(chrono::Utc::now)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let schema = job.schema_name.replace('\'', "''");
            let name = table.Name.O.replace('\'', "''");
            let metadata = ConcreteSession::new(self.0.domain.clone());
            metadata.SetInRestrictedSQL(true);
            query(&metadata, "SET SESSION time_zone='UTC'").map_err(|error| error.to_string())?;
            // Status reads are separate from the heavy analysis.
            let states = query(&metadata, &format!("SELECT state FROM mysql.analyze_jobs WHERE table_schema='{schema}' AND table_name='{name}' AND start_time >= '{since}'")).unwrap_or_default();
            let observed = states
                .iter()
                .filter_map(|row| row.first())
                .find_map(|state| {
                    if state.eq_ignore_ascii_case("running") {
                        Some(AnalyzeStateRunning)
                    } else if state.eq_ignore_ascii_case("failed") {
                        Some(AnalyzeStateFailed)
                    } else if state.eq_ignore_ascii_case("finished") {
                        Some(AnalyzeStateDone)
                    } else {
                        None
                    }
                });
            if let Some(state) = observed.filter(|state| *state != AnalyzeStateRunning) {
                return Ok(state);
            }
            let elapsed = if job.real_start_ts == 0 {
                0
            } else {
                chrono::Utc::now()
                    .timestamp_millis()
                    .saturating_sub((job.real_start_ts >> 18) as i64)
                    .max(0) as u64
            };
            let timeout = std::time::Duration::from_millis(60_000.max(elapsed.saturating_mul(2)));
            if observed == Some(AnalyzeStateRunning) {
                contexts.analyzes.insert(
                    job.id,
                    AnalyzeProgress {
                        start: std::time::Instant::now(),
                        timeout,
                        result: None,
                    },
                );
                return Ok(AnalyzeStateRunning);
            }
            let domain = self.0.domain.clone();
            let schema = job.schema_name.replace('`', "``");
            let name = table.Name.O.replace('`', "``");
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut session = ConcreteSession::new(domain);
                let result = (|| {
                    let vars = Arc::get_mut(&mut session.session_vars).ok_or_else(|| {
                        "new DDL analyze session variables are shared".to_string()
                    })?;
                    vars.EnableDDLAnalyzeExecOpt = true;
                    session.SetInRestrictedSQL(true);
                    query(&session, &format!("ANALYZE TABLE `{schema}`.`{name}`"))
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })();
                let _ = send.send(result);
            });
            contexts.analyzes.insert(
                job.id,
                AnalyzeProgress {
                    start: std::time::Instant::now(),
                    timeout,
                    result: Some(receive),
                },
            );
            return Ok(AnalyzeStateRunning);
        }
        let progress = contexts.analyzes.get(&job.id).unwrap();
        let result = progress
            .result
            .as_ref()
            .and_then(|result| match result.try_recv() {
                Ok(result) => Some(if result.is_ok() {
                    AnalyzeStateDone
                } else {
                    AnalyzeStateFailed
                }),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(AnalyzeStateFailed),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        if let Some(state) = result {
            contexts.analyzes.remove(&job.id);
            return Ok(state);
        }
        if progress.start.elapsed() > progress.timeout {
            contexts.analyzes.remove(&job.id);
            return Ok(AnalyzeStateTimeout);
        }
        if progress.result.is_none() {
            let metadata = ConcreteSession::new(self.0.domain.clone());
            metadata.SetInRestrictedSQL(true);
            let schema = job.schema_name.replace('\'', "''");
            let name = table.Name.O.replace('\'', "''");
            let since = (job.start_ts != 0)
                .then(|| chrono::DateTime::from_timestamp_millis((job.start_ts >> 18) as i64))
                .flatten()
                .unwrap_or_else(chrono::Utc::now)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            if let Ok(rows) = query(
                &metadata,
                &format!(
                    "SELECT state FROM mysql.analyze_jobs WHERE table_schema='{schema}' AND table_name='{name}' AND start_time >= '{since}'"
                ),
            ) {
                for row in rows {
                    if row.first().is_some_and(|state| {
                        state.eq_ignore_ascii_case("failed")
                            || state.eq_ignore_ascii_case("finished")
                    }) {
                        let state = if row[0].eq_ignore_ascii_case("finished") {
                            AnalyzeStateDone
                        } else {
                            AnalyzeStateFailed
                        };
                        contexts.analyzes.remove(&job.id);
                        return Ok(state);
                    }
                }
            }
        }
        Ok(AnalyzeStateRunning)
    }
    fn merge_modified_indexes(
        &mut self,
        request: astersql_ddl::backfilling::IndexBackfillBatch,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
        if job.reorg_meta.as_ref().is_some_and(|meta| meta.IsDistReorg) {
            let worker = {
                let mut contexts = self.2.lock().map_err(|e| e.to_string())?;
                if contexts.dxf_worker.is_none() {
                    contexts.dxf_worker = Some(
                        super::modify_column_dist_backfill::NodeService::start(self.0)?,
                    );
                }
                contexts.dxf_worker.as_ref().unwrap().clone()
            };
            super::modify_column_dist_backfill::run(
                self.0,
                request,
                job,
                true,
                worker,
                String::new(),
            )
        } else {
            super::modify_column_backfill::merge(self.0, request)
        }
    }
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        if label == "query-masking-policy"
            && astersql_testkit_testfailpoint::eval_bool(
                "github.com/pingcap/tidb/pkg/ddl/mockMissingMaskingPolicySysTable",
            )
        {
            return Err("[schema:1146]Table 'mysql.tidb_masking_policy' doesn't exist".into());
        }
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
        query_reorg(self.0, sql).map_err(|e| e.to_string())
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

impl astersql_ddl::delete_range::DeleteRangeExecutor for SystemSessionLease {
    fn current_version(&mut self) -> Result<u64, String> {
        jobsubmit::Session::current_version(self).map_err(|e| e.to_string())
    }
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.query_with_label(sql.to_owned(), "ddl_delete_range")
            .map(|_| ())
    }
}
impl astersql_ddl::job_worker::DurableJobSession for SystemSessionLease {
    fn transaction_size(&mut self) -> Result<usize, String> {
        self.concrete()
            .call(|session| {
                session
                    .inner
                    .state
                    .borrow()
                    .transaction
                    .as_ref()
                    .map(|transaction| transaction.Size())
                    .ok_or_else(|| sys_error("active transaction required"))
            })
            .map_err(|error| error.to_string())
    }

    fn report_ddl_job_ru(&mut self, job: &astersql_meta_model::group_3::Job) {
        let resource_group = job
            .reorg_meta
            .as_ref()
            .map(|meta| meta.ResourceGroupName.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME);
        let resource_group = resource_group.to_owned();
        let ru = job.ru;
        let _ = self.concrete().call(move |session| {
            if let Some(reporter) = session.inner.domain.ruv2_consumption_reporter() {
                reporter.report_ruv2_consumption(&resource_group, ru, 0.0, 0.0);
            }
            Ok(())
        });
    }

    fn bind_owner_epoch(&mut self, epoch: u64) -> Result<(), String> {
        let mut contexts = self
            .concrete()
            .mview_builds
            .lock()
            .map_err(|_| "materialized view reorg context poisoned")?;
        if contexts.owner_epoch != epoch {
            contexts.completed.clear();
            contexts.index_cloud_uris.clear();
            contexts.owner_epoch = epoch;
        }
        Ok(())
    }

    fn backfill_index_batch(
        &mut self,
        request: astersql_ddl::backfilling::IndexBackfillBatch,
    ) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
        self.concrete()
            .call(move |session| backfill_index_batch(session, request).map_err(sys_error))
            .map_err(|e| e.to_string())
    }
    fn register_delete_ranges(
        &mut self,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        let context = self.pool.get().map_err(|e| e.to_string())?;
        let mut gc = SystemSessionLease {
            metadata_error: None,
            pool: self.pool.clone(),
            context,
        };
        astersql_ddl::delete_range::add_persistent_delete_range_job(&mut gc, job)
    }

    fn with_execution_context(
        &mut self,
        operation: astersql_ddl::job_worker::ExecutionOperation,
    ) -> Result<Vec<u8>, String> {
        let pool = self.pool.clone();
        let builds = self.concrete().mview_builds.clone();
        let storage_class_transitions = self.concrete().storage_class_transitions.clone();
        self.concrete()
            .call(move |session| {
                if session.state.borrow().transaction.is_none() {
                    return Err(sys_error("active transaction required"));
                }
                operation(&mut ConcreteJobExecutionContext(
                    session,
                    pool,
                    builds,
                    storage_class_transitions,
                ))
                .map_err(sys_error)
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
    fn owner_epoch(&self) -> u64 {
        self.owner.OwnerEpoch()
    }
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

/// Go addIndexTxnWorker.BackfillData/fetchRowColVals: stream the transaction's
/// snapshot, decode full catalog rows, check unique handles and lock source rows.
/// The outer worker owns commit/retry and publishes statistics only after commit.
fn backfill_index_batch(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    backfill_index_batch_with_ingest(session, request, None)
}
pub(super) fn backfill_index_batch_with_ingest(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
    ingest_job: Option<i64>,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    backfill_index_batch_with_ingest_options(
        session,
        request,
        ingest_job,
        astersql_kv::SSTImportOptions::default(),
    )
}
pub(super) struct IndexBackfillRecords {
    pub(super) context: astersql_ddl::backfilling::BackfillTaskContext,
    pub(super) records: Vec<(
        kv::Key,
        astersql_meta_model::IndexInfo,
        Vec<(kv::Key, Vec<u8>, bool)>,
    )>,
}

pub(super) fn backfill_index_batch_with_ingest_options(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
    ingest_job: Option<i64>,
    options: astersql_kv::SSTImportOptions,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    let batch = generate_index_backfill_records(session, request, ingest_job.is_some())?;
    write_index_backfill_records(session, batch, ingest_job, options)
}

/// The scan/encode stage produces owned bytes and metadata. KV handles remain
/// thread-local; the writer reconstructs them from the original record key.
pub(super) fn generate_index_backfill_records(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
    ingest: bool,
) -> Result<IndexBackfillRecords, String> {
    use super::row_codec::{
        datum_to_runtime_value, origin_default_runtime_value, relational_index_value_rows,
        runtime_value_to_datum,
    };
    use astersql_ddl::backfilling::BackfillTaskContext;
    let mode = astersql_parser_mysql::r#const::SQLMode(request.sql_mode);
    let strict = mode.HasStrictMode();
    let flags = astersql_types::scalar::StrictFlags
        .WithTruncateAsWarning(!strict)
        .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
        .WithIgnoreZeroInDate(
            !mode.HasNoZeroInDateMode() || !strict || mode.HasAllowInvalidDatesMode(),
        )
        .WithIgnoreZeroDateErr(!mode.HasNoZeroDateMode() || !strict);
    let mut state = session.state.borrow_mut();
    let txn = state
        .transaction
        .as_mut()
        .ok_or("active transaction required")?;
    let table = astersql_meta::TransactionMutator::new(txn.as_mut())
        .get_table(request.schema_id, request.table_id)?
        .ok_or("DDL backfill table missing")?;
    let indexes = request
        .index_ids
        .iter()
        .map(|id| {
            table
                .Indices
                .iter()
                .find(|idx| idx.ID == *id)
                .cloned()
                .ok_or("DDL backfill index missing".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if indexes
        .iter()
        .any(|index| index.State != astersql_meta_model::SchemaState::WriteReorganization)
    {
        return Err("DDL backfill index is not in write reorganization".into());
    }
    // Transactional add-index explicitly rejects partial indexes in Go.
    if !ingest
        && indexes
            .iter()
            .any(|index| !index.ConditionExprString.is_empty())
    {
        return Err("[ddl:8200]Unsupported add partial index without fast reorg".into());
    }
    txn.SetOption(kv::Priority, Some(Box::new(request.task.priority)));
    txn.SetOption(
        kv::ResourceGroupName,
        Some(Box::new(request.resource_group)),
    );
    txn.SetOption(kv::RequestSourceInternal, Some(Box::new(true)));
    txn.SetOption(
        kv::RequestSourceType,
        Some(Box::new(kv::InternalTxnDDL.to_owned())),
    );
    let fields = table
        .Columns
        .iter()
        .map(|col| (col.ID, Box::new(col.FieldType.clone())))
        .collect::<HashMap<_, _>>();
    let handle_ids = if table.PKIsHandle {
        table
            .GetPkColInfo()
            .map(|col| vec![col.ID])
            .unwrap_or_default()
    } else if table.IsCommonHandle {
        table
            .Indices
            .iter()
            .find(|idx| idx.Primary)
            .map(|idx| {
                idx.Columns
                    .iter()
                    .map(|col| table.Columns[col.Offset as usize].ID)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let prefix = astersql_tablecodec::GenTableRecordPrefix(request.task.physical_table_id).0;
    if !request.task.start_key.starts_with(&prefix)
        || request.task.end_key > kv::Key(prefix.clone()).PrefixNext().0
    {
        return Err("backfill range does not belong to physical table".into());
    }
    let mut iterator = txn
        .GetSnapshot()
        .Iter(
            kv::Key(request.task.start_key.clone()),
            Some(kv::Key(request.task.end_key.clone())),
        )
        .map_err(|e| e.to_string())?;
    let mut context = BackfillTaskContext {
        finish_ts: txn.StartTS(),
        ..Default::default()
    };
    let generated = (|| {
        // A bounded batch buffer contains encoded index records, never an
        // in-memory substitute for the source table or MVCC snapshot.
        let mut records = Vec::new();
        while iterator.Valid() && context.scan_count < request.batch_size as i64 {
            let row_key = iterator.Key();
            if !row_key.0.starts_with(&prefix) {
                break;
            }
            let (_, handle) = astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(
                row_key.0.clone(),
            ))
            .map_err(|e| e.to_string())?;
            txn.LockKeys(
                &kv::Context::default(),
                &mut kv::LockCtx::default(),
                std::slice::from_ref(&row_key),
            )
            .map_err(|e| e.to_string())?;
            let row_value = match txn.Get(&kv::Context::default(), row_key.clone(), &[]) {
                Ok(value) => value.Value,
                Err(error) if kv::IsErrNotFound(&error) => {
                    context.next_key = row_key.Next().0;
                    kv::NextUntil(iterator.as_mut(), |key| !key.0.starts_with(&row_key.0))
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };
            context.scan_count += 1;
            let datums = astersql_tablecodec::DecodeRowToDatumMap(
                Some(row_value),
                fields.clone(),
                Some(astersql_tablecodec::time::UTC),
            )
            .map_err(|e| e.to_string())?;
            let mut datums = astersql_tablecodec::DecodeHandleToDatumMap(
                Some(handle.Copy()),
                handle_ids.clone(),
                fields.clone(),
                Some(astersql_tablecodec::time::UTC),
                Some(datums),
            )
            .map_err(|e| e.to_string())?;
            let mut row = HashMap::new();
            for col in &table.Columns {
                let value = match datums.get(&col.ID) {
                    Some(datum) => {
                        datum_to_runtime_value(datum, Some(col)).map_err(|e| e.to_string())?
                    }
                    None => origin_default_runtime_value(col),
                };
                row.insert(col.Name.L.clone(), value);
            }
            for col in table
                .Columns
                .iter()
                .filter(|col| col.IsGenerated() && !col.GeneratedStored)
            {
                let expr = crate::dml_runtime::ParseGeneratedExpr(&col.GeneratedExprString)
                    .map_err(|e| e.to_string())?;
                let value =
                    crate::dml_runtime::EvalExpr(&expr, &row, None).map_err(|e| e.to_string())?;
                let datum = runtime_value_to_datum(value.as_ref(), col, flags)
                    .map_err(|e| e.to_string())?;
                row.insert(col.Name.L.clone(), value);
                datums.insert(col.ID, datum);
            }
            for index in &indexes {
                if !index.ConditionExprString.is_empty() {
                    let condition =
                        crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)
                            .map_err(|e| e.to_string())?;
                    if !super::row_matches_simple_where(&row, &condition) {
                        continue;
                    }
                }
                let values =
                    if index.MVIndex || index.Columns.iter().any(|part| part.UseChangingType) {
                        relational_index_value_rows(&table, index, &row, flags)
                            .map_err(|e| e.to_string())?
                    } else {
                        vec![
                            index
                                .Columns
                                .iter()
                                .map(|part| {
                                    let col = table
                                        .Columns
                                        .get(part.Offset as usize)
                                        .ok_or("invalid backfill index column offset")?;
                                    datums.get(&col.ID).cloned().map(Ok).unwrap_or_else(|| {
                                        runtime_value_to_datum(
                                            row.get(&col.Name.L).and_then(Option::as_ref),
                                            col,
                                            flags,
                                        )
                                        .map_err(|e| e.to_string())
                                    })
                                })
                                .collect::<Result<Vec<_>, String>>()?,
                        ]
                    };
                let actual_handle: Box<dyn astersql_tablecodec::kv::Handle> = if index.Global
                    && index.GlobalIndexVersion >= astersql_meta_model::GlobalIndexVersionV1
                {
                    Box::new(astersql_tablecodec::kv::NewPartitionHandle(
                        request.task.physical_table_id,
                        handle.Copy(),
                    ))
                } else {
                    handle.Copy()
                };
                let mut effective_columns = table.Columns.clone();
                for part in &index.Columns {
                    if part.UseChangingType {
                        let column = &mut effective_columns[part.Offset as usize];
                        if let Some(changing) = &column.ChangingFieldType {
                            column.FieldType = changing.clone();
                        }
                    }
                }
                let codec_table = astersql_tablecodec::model::TableInfo {
                    Columns: effective_columns,
                    Indices: table.Indices.clone(),
                    PKIsHandle: table.PKIsHandle,
                    IsCommonHandle: table.IsCommonHandle,
                    CommonHandleVersion: table.CommonHandleVersion,
                    ..Default::default()
                };
                let mut entries = Vec::new();
                for values in values {
                    let restored = index.Columns.iter().any(|part| {
                        astersql_tablecodec::types::NeedRestoredDataWithCollate(
                            &codec_table.Columns[part.Offset as usize].FieldType,
                            astersql_tablecodec::collate::NewCollationEnabled(),
                        )
                    });
                    let (key, distinct) = astersql_tablecodec::GenIndexKey(
                        astersql_tablecodec::codec::NewEncoder(
                            astersql_tablecodec::collate::NewCollationEnabled(),
                        ),
                        Some(astersql_tablecodec::time::UTC),
                        Box::new(codec_table.clone()),
                        Box::new(index.clone()),
                        if index.Global {
                            table.ID
                        } else {
                            request.task.physical_table_id
                        },
                        values.clone(),
                        Some(actual_handle.Copy()),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                    let restored_data = astersql_tablecodec::TryGetCommonPkColumnRestoredIds(
                        astersql_tablecodec::collate::NewCollationEnabled(),
                        Box::new(codec_table.clone()),
                    )
                    .iter()
                    .filter_map(|id| datums.get(id).cloned())
                    .collect();
                    let value = astersql_tablecodec::GenIndexValuePortal(
                        astersql_tablecodec::collate::NewCollationEnabled(),
                        Some(astersql_tablecodec::time::UTC),
                        Box::new(codec_table.clone()),
                        Box::new(index.clone()),
                        restored,
                        distinct,
                        false,
                        values,
                        actual_handle.Copy(),
                        if index.Global {
                            request.task.physical_table_id
                        } else {
                            0
                        },
                        restored_data,
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                    entries.push((kv::Key(key), value, distinct));
                }
                records.push((row_key.clone(), index.clone(), entries));
            }
            context.next_key = row_key.Next().0;
            kv::NextUntil(iterator.as_mut(), |key| !key.0.starts_with(&row_key.0))
                .map_err(|e| e.to_string())?;
        }
        context.done = !iterator.Valid() || !iterator.Key().0.starts_with(&prefix);
        if context.done {
            context.next_key = request.task.end_key.clone();
        }
        Ok::<_, String>(records)
    })();
    iterator.Close();
    let records = generated?;
    Ok(IndexBackfillRecords { context, records })
}

pub(super) fn write_index_backfill_records(
    session: &mut ConcreteSession,
    batch: IndexBackfillRecords,
    ingest_job: Option<i64>,
    options: astersql_kv::SSTImportOptions,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    let domain = session.domain.clone();
    let mut state = session.state.borrow_mut();
    let txn = state
        .transaction
        .as_mut()
        .ok_or("active transaction required")?;
    let mut context = batch.context;
    let mut ingest_pairs = Vec::new();
    for (row_key, index, entries) in batch.records {
        let (physical_table_id, handle) =
            astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(row_key.0.clone()))
                .map_err(|e| e.to_string())?;
        let handle: Box<dyn astersql_tablecodec::kv::Handle> = if index.Global
            && index.GlobalIndexVersion >= astersql_meta_model::GlobalIndexVersionV1
        {
            Box::new(astersql_tablecodec::kv::NewPartitionHandle(
                physical_table_id,
                handle,
            ))
        } else {
            handle
        };
        let mut pending = Vec::new();
        for (key, value, distinct) in entries {
            if index.Unique {
                match kv::GetValue(&kv::Context::default(), txn.as_ref(), key.clone()) {
                    Ok(existing) => {
                        if distinct {
                            let old = astersql_tablecodec::DecodeIndexHandle(
                                key.0.clone(),
                                existing,
                                index.Columns.len(),
                            )
                            .map_err(|e| e.to_string())?
                            .ok_or("missing index handle")?;
                            if !old.Equal(handle.as_ref()) {
                                return Err(format!(
                                    "[kv:1062]Duplicate entry for key '{}'",
                                    index.Name.O
                                ));
                            }
                        }
                        continue;
                    }
                    Err(error) if kv::ErrNotExist.Equal(Some(&error)) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            pending.push((key, value));
        }
        if pending.is_empty() {
            continue;
        }
        txn.LockKeys(
            &kv::Context::default(),
            &mut kv::LockCtx::default(),
            &[row_key],
        )
        .map_err(|e| e.to_string())?;
        for (key, value) in pending {
            if ingest_job.is_some() {
                match txn.Get(&kv::Context::default(), key.clone(), &[]) {
                    Ok(existing) if existing.Value == value => continue,
                    Ok(_) => {
                        return Err(format!(
                            "[kv:1062]Duplicate entry for key '{}'",
                            index.Name.O
                        ));
                    }
                    Err(error) if kv::IsErrNotFound(&error) => {}
                    Err(error) => return Err(error.to_string()),
                }
                ingest_pairs.push(astersql_lightning_verification::KvPair {
                    key: key.0,
                    val: value,
                });
            } else {
                txn.Set(key, value).map_err(|e| e.to_string())?;
            }
        }
        context.added_count += 1;
    }
    if let Some(job_id) = ingest_job {
        super::modify_column_backfill::ingest_with_options(domain, job_id, ingest_pairs, options)?;
    }
    Ok(context)
}

/// Go deriveMaterializedScheduleNextUnixSecondsForDDL. A fresh context keeps
/// the worker's timezone, SQL mode and statement error policy untouched.
fn derive_create_mlog_schedule(
    schema: &str,
    table: &str,
    info: &astersql_meta_model::MaterializedViewLogInfo,
) -> Result<(Option<i64>, bool), String> {
    use astersql_expression_exprstatic::{
        NewEvalContext, NewExprContext, WithErrLevelMap, WithEvalCtx, WithLocation, WithSQLMode,
        WithTypeFlags, errctx,
    };
    let start = info.PurgeStartWith.trim();
    let next = info.PurgeNext.trim();
    if start.is_empty() && next.is_empty() {
        return Ok((None, true));
    }
    let mode = info.PurgeScheduleSQLMode;
    let flags = astersql_types::scalar::StrictFlags
        .WithTruncateAsWarning(!mode.HasStrictMode())
        .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
        .WithIgnoreZeroInDate(!mode.HasStrictMode() || mode.HasAllowInvalidDatesMode())
        .WithCastTimeToYearThroughConcat(true);
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    for group in [
        errctx::ErrGroup::ErrGroupTruncate,
        errctx::ErrGroup::ErrGroupBadNull,
        errctx::ErrGroup::ErrGroupNoDefault,
    ] {
        levels[group as usize] = errctx::ResolveErrLevel(false, !mode.HasStrictMode());
    }
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] =
        errctx::ResolveErrLevel(!mode.HasErrorForDivisionByZeroMode(), !mode.HasStrictMode());
    let eval = std::sync::Arc::new(NewEvalContext(vec![
        WithSQLMode(mode),
        WithLocation(chrono_tz::UTC),
        WithTypeFlags(flags),
        WithErrLevelMap(levels),
    ]));
    let context = NewExprContext(vec![WithEvalCtx(eval.clone())]);
    let evaluate = |sql: &str| -> Result<Option<chrono::NaiveDateTime>, String> {
        let mut parser = astersql_parser::New();
        parser.SetSQLMode(mode);
        let stmt = parser
            .ParseOneStmt(&format!("select ({sql})"), "utf8mb4", "utf8mb4_bin")
            .map_err(|e| e.to_string())?;
        let select = stmt
            .as_any()
            .downcast_ref::<super::ast::SelectStmt>()
            .ok_or("schedule is not a scalar expression")?;
        let expr = select
            .Fields
            .Fields
            .first()
            .and_then(|f| f.Expr.as_ref())
            .ok_or("missing schedule expression")?;
        let built = astersql_planner_core::PlannerBuildSimpleExpr(&context, expr, Vec::new())
            .map_err(|e| e.to_string())?;
        let value = built
            .Eval(eval.as_ref(), astersql_expression::chunk::Row::default())
            .map_err(|e| e.to_string())?;
        if value.IsNull() {
            return Ok(None);
        }
        if value.Kind() != astersql_types::datum::KindMysqlTime {
            return Err("materialized schedule expression expected DATE/DATETIME/TIMESTAMP".into());
        }
        let time = value.GetMysqlTime();
        if ![
            astersql_parser_mysql::r#type::TypeDate,
            astersql_parser_mysql::r#type::TypeDatetime,
            astersql_parser_mysql::r#type::TypeTimestamp,
        ]
        .contains(&time.Type())
        {
            return Err("materialized schedule expression expected DATE/DATETIME/TIMESTAMP".into());
        }
        super::parse_runtime_datetime(&time.String())
            .map(Some)
            .ok_or_else(|| format!("invalid materialized schedule time {}", time.String()))
    };
    let now = evaluate("NOW(6)")?
        .ok_or("create materialized view: failed to evaluate refresh schedule expression")?;
    let (result, clause) = if !start.is_empty() {
        match evaluate(start)? {
            None => (None, "START WITH"),
            Some(start_at)
                if !next.is_empty() && start_at < now + chrono::Duration::seconds(10) =>
            {
                (evaluate(next)?, "NEXT")
            }
            Some(start_at) => (Some(start_at), "START WITH"),
        }
    } else {
        (evaluate(next)?, "NEXT")
    };
    if result.is_none() {
        super::BgLogger().log(if next.is_empty(){super::LogLevel::Warn}else{super::LogLevel::Error},
            "create materialized view log: purge schedule expression evaluated to NULL, updating NEXT_PURGE_UNIX_SECONDS to NULL",
            [super::LogField::String("schemaName".into(),schema.into()),super::LogField::String("tableName".into(),table.into()),super::LogField::String("nullExprClause".into(),clause.into()),super::LogField::String("purgeStartWith".into(),start.into()),super::LogField::String("purgeNext".into(),next.into())]);
    }
    Ok((result.map(|t| t.and_utc().timestamp()), true))
}

#[cfg(test)]
#[path = "normal_ddl_create_materialized_view_log_test.rs"]
mod normal_ddl_create_materialized_view_log_test;
