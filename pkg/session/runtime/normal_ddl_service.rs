// Copyright 2026 AsterSQL.

//! Normal Domain DDL lifecycle. Cross-keyspace factories never instantiate this service.
use super::system_session::SystemSessionPool;
use astersql_ddl::job_scheduler::JobScheduler;
use astersql_ddl::job_worker::{DurableJobExecutor, JobLease, JobWorker, WorkerType};
use astersql_domain::domain::{DdlService, StartMode};
use astersql_owner::manager::{Context, Manager};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

type ExecutorFactory = dyn Fn() -> Result<Box<dyn DurableJobExecutor>, String> + Send + Sync;
type SubmitTableMode = dyn Fn(&str) -> Result<(), String> + Send + Sync;
type CancelSchemaWait = dyn Fn() + Send + Sync;

pub struct DomainSchemaLoader(pub std::sync::Weak<astersql_domain::Domain>);
impl astersql_ddl::SchemaLoader for DomainSchemaLoader {
    fn reload(&self) -> Result<(), astersql_ddl::SchemaLoaderError> {
        self.0
            .upgrade()
            .ok_or_else(|| astersql_ddl::SchemaLoaderError::new("normal Domain is closed"))?
            .reload()
            .map(|_| ())
            .map_err(|error| astersql_ddl::SchemaLoaderError::new(error.to_string()))
    }
}

struct Lease {
    owner: Arc<dyn Manager>,
    cancellation: Context,
}
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        self.owner.IsOwner()
    }
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}
struct Worker {
    stop: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}
struct Lifecycle {
    started: bool,
    closed: bool,
    worker: Option<Worker>,
}

pub struct NormalDdlService {
    owner: Arc<dyn Manager>,
    owner_runtime: Arc<tokio::runtime::Runtime>,
    cancellation: Context,
    pool: Arc<SystemSessionPool>,
    schema_loader: Arc<dyn astersql_ddl::SchemaLoader>,
    executor: Arc<ExecutorFactory>,
    submit: Arc<SubmitTableMode>,
    cancel_schema_wait: Arc<CancelSchemaWait>,
    campaign_enabled: bool,
    lifecycle: Mutex<Lifecycle>,
    last_error: Arc<Mutex<Option<String>>>,
}
impl NormalDdlService {
    pub fn new(
        owner: Arc<dyn Manager>,
        owner_runtime: Arc<tokio::runtime::Runtime>,
        cancellation: Context,
        pool: Arc<SystemSessionPool>,
        schema_loader: Arc<dyn astersql_ddl::SchemaLoader>,
        executor: Arc<ExecutorFactory>,
        submit: Arc<SubmitTableMode>,
        cancel_schema_wait: Arc<CancelSchemaWait>,
        campaign_enabled: bool,
    ) -> Self {
        Self {
            owner,
            owner_runtime,
            cancellation,
            pool,
            schema_loader,
            executor,
            submit,
            cancel_schema_wait,
            campaign_enabled,
            lifecycle: Mutex::new(Lifecycle {
                started: false,
                closed: false,
                worker: None,
            }),
            last_error: Arc::new(Mutex::new(None)),
        }
    }
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }
}
impl DdlService for NormalDdlService {
    fn start(&self, mode: StartMode) -> Result<(), String> {
        let mut state = self.lifecycle.lock().unwrap();
        if state.closed {
            return Err("normal DDL service is closed".into());
        }
        if state.started {
            return Ok(());
        }
        if matches!(mode, StartMode::Upgrade) && !self.campaign_enabled {
            return Err("DDL must be enabled when upgrading".into());
        }
        if self.campaign_enabled {
            if matches!(mode, StartMode::Upgrade) {
                self.owner_runtime
                    .block_on(self.owner.ForceToBeOwner(&self.cancellation))
                    .map_err(|e| e.to_string())?;
            }
            self.owner_runtime
                .block_on(self.owner.CampaignOwner(&[]))
                .map_err(|e| e.to_string())?;
            let pool = self.pool.clone();
            let owner = self.owner.clone();
            let cancellation = self.cancellation.clone();
            let make_executor = self.executor.clone();
            let schema_loader = self.schema_loader.clone();
            let error = self.last_error.clone();
            let (stop, receiver) = mpsc::channel();
            let thread = match thread::Builder::new()
                .name("normal-ddl-scheduler".into())
                .spawn(move || {
                    let lease = Lease {
                        owner,
                        cancellation,
                    };
                    let mut scheduler = JobScheduler::new(
                        JobWorker::new(WorkerType::General),
                        JobWorker::new(WorkerType::AddIndex),
                    );
                    let mut executor = None;
                    let mut was_owner = false;
                    loop {
                        if lease.is_cancelled() {
                            break;
                        }
                        if lease.is_owner() {
                            if !was_owner {
                                scheduler.must_reload_schemas_with(
                                    schema_loader.as_ref(),
                                    Duration::from_millis(200),
                                    || lease.is_cancelled() || !lease.is_owner(),
                                );
                                if lease.is_cancelled() || !lease.is_owner() {
                                    continue;
                                }
                                was_owner = true;
                            }
                            let result = (|| {
                                if executor.is_none() {
                                    executor = Some(make_executor()?);
                                }
                                let mut session = pool.acquire()?;
                                scheduler.schedule_persisted(
                                    &mut session,
                                    &lease,
                                    executor.as_mut().unwrap().as_mut(),
                                    0,
                                )
                            })();
                            match result {
                                Ok(_) => *error.lock().unwrap() = None,
                                Err(message) => {
                                    eprintln!("normal DDL scheduler: {message}");
                                    *error.lock().unwrap() = Some(message);
                                }
                            }
                        }
                        if !lease.is_owner() {
                            was_owner = false;
                            executor = None;
                            scheduler.close();
                            scheduler = JobScheduler::new(
                                JobWorker::new(WorkerType::General),
                                JobWorker::new(WorkerType::AddIndex),
                            );
                        }
                        match receiver.recv_timeout(Duration::from_millis(300)) {
                            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                    scheduler.close();
                }) {
                Ok(thread) => thread,
                Err(error) => {
                    self.owner_runtime.block_on(self.owner.CampaignCancel());
                    return Err(error.to_string());
                }
            };
            state.worker = Some(Worker { stop, thread });
        }
        state.started = true;
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        let worker = {
            let mut state = self.lifecycle.lock().unwrap();
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.worker.take()
        };
        self.cancellation.cancel();
        (self.cancel_schema_wait)();
        let joined = if let Some(worker) = worker {
            let _ = worker.stop.send(());
            worker
                .thread
                .join()
                .map_err(|_| "normal DDL scheduler panicked".to_owned())
        } else {
            Ok(())
        };
        self.owner_runtime.block_on(self.owner.Close());
        self.pool.close();
        joined
    }
    fn owner_id(&self) -> Option<String> {
        self.owner_runtime
            .block_on(self.owner.GetOwnerID(&self.cancellation))
            .ok()
    }
    fn alter_table_mode(&self, target: &str) -> Result<(), String> {
        if self.lifecycle.lock().unwrap().closed {
            return Err("normal DDL service is closed".into());
        }
        (self.submit)(target)
    }
}
impl Drop for NormalDdlService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Ordinary Domains fence both user transactions and borrowed internal SQL
/// sessions. Cross-keyspace Domains have only the latter.
pub struct NormalSchemaCoordinator {
    pub domain: std::sync::Weak<astersql_domain::Domain>,
    pub internal: Arc<astersql_domain_crossks::SchemaCoordinator>,
}
impl astersql_infoschema_issyncer::InfoSchemaCoordinator for NormalSchemaCoordinator {
    fn CheckOldRunningTxn(
        &self,
        jobs: &mut std::collections::HashMap<i64, astersql_infoschema_issyncer::JobMDL>,
    ) {
        self.internal.CheckOldRunningTxn(jobs);
        if let Some(manager) = self
            .domain
            .upgrade()
            .and_then(|domain| domain.schema_coordinator())
        {
            let mut shared = jobs
                .iter()
                .map(|(id, job)| {
                    (
                        *id,
                        Arc::new(astersql_session_sessmgr::mdldef::JobMDL {
                            ver: job.Ver,
                            table_ids: job.TableIDs.clone(),
                        }),
                    )
                })
                .collect();
            manager.CheckOldRunningTxn(&mut shared);
            jobs.retain(|id, _| shared.contains_key(id));
        }
    }
    fn KillNonFlashbackClusterConn(&self) {
        if let Some(manager) = self
            .domain
            .upgrade()
            .and_then(|domain| domain.schema_coordinator())
        {
            manager.KillNonFlashbackClusterConn();
        }
    }
}
