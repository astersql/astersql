// Copyright 2026 AsterSQL.

//! Production target-keyspace runtime assembly for a serving Domain.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_ddl_jobsubmit as jobsubmit;
use astersql_ddl_systable as systable;
use astersql_domain::Domain;
use astersql_domain_crossks::{
    self as crossks, DdlBackend, Lifecycle, ManagerError, RuntimeFactory, SessionPool,
};
use astersql_domain_serverinfo::{self as serverinfo, Context, EtcdClient};
use astersql_store_copr::{NetworkPdKeyspaceClient, NetworkSecurity};

use super::{
    CanonicalSessionFactory,
    crossks_job_submit::CrossKSJobSubmitter,
    crossks_owner::CrossKSDdlOwner,
    crossks_schema::{CrossKSSchemaSyncer, CrossKSStateSyncer},
    crossks_session_pool::{
        CrossKSFlashbackGuard, CrossKSMinJobId, CrossKSSessionPool, CrossKSSystemTablePool,
    },
    crossks_store::{CrossKSStore, open_target_store_with_tls},
};

type StoreOpener = dyn Fn(&[String], &str) -> Result<Arc<CrossKSStore>, ManagerError> + Send + Sync;
type EtcdProvider = dyn Fn(&str) -> Result<Arc<dyn EtcdClient>, ManagerError> + Send + Sync;

#[derive(Clone)]
pub struct CrossKSProductionRuntimeFactory {
    pd_endpoints: Vec<String>,
    etcd_endpoints: Vec<String>,
    tls_files: Option<(String, String, String)>,
    store_opener: Arc<StoreOpener>,
    etcd_provider: Option<Arc<EtcdProvider>>,
    pending_etcd: Arc<Mutex<HashMap<String, Arc<serverinfo::RealEtcdClient>>>>,
}

impl CrossKSProductionRuntimeFactory {
    pub fn new(
        pd_endpoints: Vec<String>,
        etcd_endpoints: Vec<String>,
        tls_files: Option<(String, String, String)>,
    ) -> Self {
        let store_tls = tls_files.clone();
        Self {
            pd_endpoints,
            etcd_endpoints,
            tls_files,
            store_opener: Arc::new(move |endpoints, keyspace| {
                open_target_store_with_tls(endpoints, keyspace, store_tls.as_ref())
            }),
            etcd_provider: None,
            pending_etcd: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clients(mut self, store: Arc<StoreOpener>, etcd: Arc<EtcdProvider>) -> Self {
        self.store_opener = store;
        self.etcd_provider = Some(etcd);
        self
    }

    fn target_real_etcd(
        &self,
        keyspace: &str,
    ) -> Result<Arc<serverinfo::RealEtcdClient>, ManagerError> {
        let security = self
            .tls_files
            .as_ref()
            .map(|(ca, cert, key)| NetworkSecurity {
                ca_path: ca.clone(),
                cert_path: cert.clone(),
                key_path: key.clone(),
            });
        let pd = NetworkPdKeyspaceClient::connect(
            &self.pd_endpoints,
            security.as_ref(),
            Duration::from_secs(5),
            "astersql-crossks",
        )
        .map_err(|error| ManagerError(format!("connect target PD: {error}")))?;
        let id = pd.load_keyspace(keyspace).map_err(|error| {
            ManagerError(format!("resolve target keyspace {keyspace}: {error}"))
        })?;
        let tls = self
            .tls_files
            .as_ref()
            .map(|(ca, cert, key)| (ca.as_str(), cert.as_str(), key.as_str()));
        let etcd = serverinfo::RealEtcdClient::connect(self.etcd_endpoints.clone(), tls)
            .map_err(|error| ManagerError(format!("connect target etcd: {error}")))?
            .with_namespace(format!("/keyspaces/tidb/{id}"));
        Ok(Arc::new(etcd))
    }

    fn target_etcd(&self, keyspace: &str) -> Result<Arc<dyn EtcdClient>, ManagerError> {
        if let Some(provider) = &self.etcd_provider {
            return provider(keyspace);
        }
        let client = self.target_real_etcd(keyspace)?;
        self.pending_etcd
            .lock()
            .expect("target etcd cache poisoned")
            .insert(keyspace.to_owned(), Arc::clone(&client));
        Ok(client)
    }

    /// Install a lazily created target runtime manager on the serving Domain.
    pub fn install_on_domain(self: Arc<Self>, domain: &Arc<Domain>, current_keyspace: String) {
        let provider = Arc::clone(&self);
        let manager = crossks::new_manager_with_server_info_provider(
            false,
            current_keyspace,
            self,
            Arc::new(move |keyspace| provider.target_etcd(keyspace).map(Some)),
            Arc::new(serverinfo::NoopMinStartTSReporter),
        );
        domain.install_cross_ks_manager(manager);
    }
}

struct TargetInfoCache(Arc<Domain>);
impl crossks::InfoCache for TargetInfoCache {}

struct TargetDomainLifecycle(Arc<Domain>);
impl Lifecycle for TargetDomainLifecycle {
    fn close(&self) -> Result<(), ManagerError> {
        self.0.close();
        Ok(())
    }
}

struct CrossKSMinIdLoop {
    cancellation: systable::Cancellation,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CrossKSMinIdLoop {
    fn start(refresher: Arc<systable::MinJobIdRefresher>) -> Result<Arc<Self>, ManagerError> {
        let cancellation = systable::Cancellation::default();
        let worker_cancel = cancellation.clone();
        let thread = thread::Builder::new()
            .name("crossks-min-ddl-job-id".into())
            .spawn(move || refresher.start(&systable::Context::default(), &worker_cancel))
            .map_err(|error| {
                ManagerError(format!("start cross-keyspace min job ID refresh: {error}"))
            })?;
        Ok(Arc::new(Self {
            cancellation,
            thread: Mutex::new(Some(thread)),
        }))
    }
}

impl Lifecycle for CrossKSMinIdLoop {
    fn close(&self) -> Result<(), ManagerError> {
        self.cancellation.cancel();
        if let Some(thread) = self
            .thread
            .lock()
            .expect("min job ID thread lock poisoned")
            .take()
        {
            let _ = thread.join();
        }
        Ok(())
    }
}

pub(crate) struct CrossKSProductionDdlBackend {
    domain: Arc<Domain>,
    pool: Arc<CrossKSSessionPool>,
    submitter: CrossKSJobSubmitter,
    owner: Arc<CrossKSDdlOwner>,
    state: Arc<CrossKSStateSyncer>,
    etcd: Arc<dyn EtcdClient>,
}

impl CrossKSProductionDdlBackend {
    pub(crate) fn new(
        domain: Arc<Domain>,
        pool: Arc<CrossKSSessionPool>,
        submitter: CrossKSJobSubmitter,
        owner: Arc<CrossKSDdlOwner>,
        state: Arc<CrossKSStateSyncer>,
        etcd: Arc<dyn EtcdClient>,
    ) -> Self {
        Self {
            domain,
            pool,
            submitter,
            owner,
            state,
            etcd,
        }
    }
}

impl DdlBackend for CrossKSProductionDdlBackend {
    fn resolve_database(&self, schema_id: i64) -> Result<Option<String>, crossks::Error> {
        Ok(self
            .domain
            .info_schema()
            .SchemaByID(schema_id)
            .map(|schema| schema.name.lower.clone()))
    }

    fn resolve_table(
        &self,
        schema_id: i64,
        table_id: i64,
    ) -> Result<Option<(String, crossks::TableMode)>, crossks::Error> {
        let Some(schema) = self.domain.info_schema().SchemaByID(schema_id) else {
            return Ok(None);
        };
        let Some(table) = self.domain.info_schema().TableByID(table_id) else {
            return Ok(None);
        };
        let info = table.Meta();
        let found = self
            .domain
            .table_by_name(&schema.name.lower, &info.name.lower)
            .map_err(|error| crossks::Error(error.to_string()))?;
        if found.ID != table_id {
            return Ok(None);
        }
        let mode = match found.Mode {
            astersql_meta_model::TableMode::TableModeNormal => crossks::TableMode::Normal,
            astersql_meta_model::TableMode::TableModeImport => crossks::TableMode::Import,
            astersql_meta_model::TableMode::TableModeRestore => crossks::TableMode::Restore,
        };
        Ok(Some((info.name.lower.clone(), mode)))
    }

    fn session_variables(&self) -> Result<crossks::SessionVariables, crossks::Error> {
        let lease = self.pool.acquire().map_err(crossks::Error)?;
        let rows = lease.query("SELECT @@sql_mode").map_err(crossks::Error)?;
        let sql_mode = rows
            .first()
            .and_then(|row| row.first())
            .map(String::as_str)
            .unwrap_or("");
        let mode = astersql_parser_mysql::r#const::GetSQLMode(sql_mode)
            .map_err(|error| crossks::Error(error.to_string()))?;
        Ok(crossks::SessionVariables {
            cdc_write_source: 0,
            sql_mode: mode.0 as u64,
        })
    }

    fn refresh_server_state(&self) -> Result<(), crossks::Error> {
        self.state.refresh().map_err(crossks::Error)
    }

    fn submit(&self, job: &mut crossks::AlterTableModeJob) -> Result<(), crossks::Error> {
        self.submitter
            .submit_table_mode(job)
            .map_err(|error| crossks::Error(error.to_string()))
    }

    fn notify_owner(&self) -> Result<(), crossks::Error> {
        self.etcd
            .Put(
                &Context::Background(),
                "/tidb/ddl/add_ddl_job_general",
                b"0".to_vec(),
                None,
            )
            .map_err(|error| crossks::Error(error.to_string()))?;
        self.owner.notify();
        Ok(())
    }

    fn history_job(&self, job_id: i64) -> Result<Option<crossks::HistoryJobState>, crossks::Error> {
        self.owner.history_job(job_id).map_err(crossks::Error)
    }
}

impl CrossKSProductionRuntimeFactory {
    fn create_runtime(
        &self,
        keyspace: &str,
        server_info_id: &str,
    ) -> Result<Arc<crossks::SessionManager>, ManagerError> {
        let prepared_etcd = self
            .pending_etcd
            .lock()
            .expect("target etcd cache poisoned")
            .remove(keyspace);
        let store = (self.store_opener)(&self.pd_endpoints, keyspace)?;
        let factory = match CanonicalSessionFactory::from_crossks_tikv_store(store.inner().clone())
        {
            Ok(factory) => factory,
            Err(error) => {
                let _ = crossks::Store::close(store.as_ref());
                return Err(ManagerError(error.to_string()));
            }
        };
        let domain = Arc::clone(factory.domain());
        let pool = match CrossKSSessionPool::try_new(Arc::clone(&domain)) {
            Ok(pool) => pool,
            Err(error) => {
                domain.close();
                let _ = crossks::Store::close(store.as_ref());
                return Err(ManagerError(error));
            }
        };
        let mut owner: Option<Arc<CrossKSDdlOwner>> = None;
        let mut schema: Option<Arc<CrossKSSchemaSyncer>> = None;
        let mut min_loop: Option<Arc<CrossKSMinIdLoop>> = None;
        let result = (|| {
            let (etcd, election) = if self.etcd_provider.is_some() {
                (self.target_etcd(keyspace)?, None)
            } else {
                let real = match prepared_etcd {
                    Some(client) => client,
                    None => self.target_real_etcd(keyspace)?,
                };
                let election = astersql_owner::NewOwnerManager(
                    astersql_owner::Context::new(),
                    real.raw_client(),
                    "ddl",
                    server_info_id.to_owned(),
                    format!("{}{}", real.namespace(), astersql_ddl_util::DDLOwnerKey),
                );
                let etcd: Arc<dyn EtcdClient> = real;
                (etcd, Some(election))
            };
            let id = server_info_id.to_owned();
            let syncer =
                CrossKSSchemaSyncer::new(Arc::clone(&domain), Arc::clone(&etcd), id.clone());
            syncer.start().map_err(ManagerError)?;
            schema = Some(Arc::clone(&syncer));
            let state = CrossKSStateSyncer::new(Arc::clone(&etcd));
            state.refresh().map_err(ManagerError)?;
            let table_pool: Arc<dyn systable::SessionPool> =
                Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
            let table_manager = systable::new_manager(table_pool);
            let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&table_manager)));
            let refresher = Arc::new(systable::new_min_job_id_refresher(table_manager));
            refresher.refresh(&systable::Context::default());
            let min_id = Arc::new(CrossKSMinJobId::new(Arc::clone(&refresher)));
            let refresh_loop = CrossKSMinIdLoop::start(refresher)?;
            min_loop = Some(Arc::clone(&refresh_loop));
            let state_for_submit: Arc<dyn jobsubmit::ServerState> = state.clone();
            let submitter =
                CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, Some(state_for_submit));
            let ddl_owner = match election {
                Some(election) => CrossKSDdlOwner::new_with_election(
                    Arc::clone(&domain),
                    Arc::clone(&pool),
                    Arc::clone(&etcd),
                    id,
                    election,
                ),
                None => CrossKSDdlOwner::new(
                    Arc::clone(&domain),
                    Arc::clone(&pool),
                    Arc::clone(&etcd),
                    id,
                ),
            };
            ddl_owner.install_schema_syncer(Arc::clone(&syncer));
            ddl_owner.start().map_err(ManagerError)?;
            owner = Some(Arc::clone(&ddl_owner));
            let backend = Arc::new(CrossKSProductionDdlBackend::new(
                Arc::clone(&domain),
                Arc::clone(&pool),
                submitter,
                Arc::clone(&ddl_owner),
                state,
                etcd,
            ));
            let cache: Arc<dyn crossks::InfoCache> = Arc::new(TargetInfoCache(Arc::clone(&domain)));
            let target_store: Arc<dyn crossks::Store> = store.clone();
            let session_pool: Arc<dyn crossks::SessionPool> = pool.clone();
            let lifecycles: Vec<Arc<dyn Lifecycle>> = vec![
                Arc::new(TargetDomainLifecycle(Arc::clone(&domain))),
                syncer,
                refresh_loop,
                ddl_owner,
            ];
            Ok(Arc::new(crossks::SessionManager::new(
                target_store,
                cache,
                session_pool,
                Arc::new(crossks::new_schema_coordinator()),
                Arc::new(crossks::DdlClient::new(backend)),
                lifecycles,
            )))
        })();
        if result.is_err() {
            if let Some(owner) = owner {
                owner.close();
            }
            if let Some(min_loop) = min_loop {
                let _ = min_loop.close();
            }
            if let Some(schema) = schema {
                schema.close();
            }
            pool.close();
            domain.close();
            let _ = crossks::Store::close(store.as_ref());
        }
        result
    }
}

impl RuntimeFactory for CrossKSProductionRuntimeFactory {
    fn create(&self, keyspace: &str) -> Result<Arc<crossks::SessionManager>, ManagerError> {
        self.create_runtime(
            keyspace,
            &format!("crossks-{}-{keyspace}", std::process::id()),
        )
    }

    fn create_with_server_info(
        &self,
        keyspace: &str,
        server_info_id: &str,
    ) -> Result<Arc<crossks::SessionManager>, ManagerError> {
        self.create_runtime(keyspace, server_info_id)
    }

    fn registration_failed(&self, keyspace: &str) {
        self.pending_etcd
            .lock()
            .expect("target etcd cache poisoned")
            .remove(keyspace);
    }
}
