// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Store and common system-session assembly for target keyspaces. Execution
//! belongs to the target's normal DDL service, never to this factory.

use super::kv;
use super::system_session::{JobSubmitServerState, SystemSessionCallbacks, SystemSessionPool};
use astersql_ddl_schemaver as version;
use astersql_domain::{Domain, DomainConfig, InfoSchemaLoader, LoadedInfoSchema, StorageHandle};
use astersql_domain_crossks::{self as crossks, Lifecycle, ManagerError, RuntimeFactory};
use astersql_domain_serverinfo as serverinfo;
use astersql_infoschema_issyncer as schema;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct TargetSessionStore {
    pub(crate) storage: Arc<StorageHandle>,
    keyspace: String,
    owned: bool,
    closed: AtomicBool,
}
impl TargetSessionStore {
    pub(crate) fn new(storage: Arc<StorageHandle>, keyspace: String, owned: bool) -> Arc<Self> {
        Arc::new(Self {
            storage,
            keyspace,
            owned,
            closed: AtomicBool::new(false),
        })
    }
}
impl crossks::Store for TargetSessionStore {
    fn keyspace(&self) -> &str {
        &self.keyspace
    }
    fn close_on_runtime_shutdown(&self) -> bool {
        self.owned
    }
    fn close(&self) -> Result<(), ManagerError> {
        if !self.owned || self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.storage
            .close()
            .map_err(|e| ManagerError(e.to_string()))
    }
}

struct SystemSchemaLoader {
    store: Arc<dyn schema::SchemaStore>,
    keyspace: String,
}
impl SystemSchemaLoader {
    fn load(&self, ts: u64) -> Result<LoadedInfoSchema, kv::Error> {
        let loader = schema::NewLoaderForCrossKS(self.store.clone(), None);
        let (info, _, _, _) = loader
            .LoadWithTS(ts, true)
            .map_err(|e| kv::errors::New(e.to_string()))?;
        Ok(LoadedInfoSchema::new(info.CompleteInfoSchema(), ts))
    }
}
impl InfoSchemaLoader for SystemSchemaLoader {
    fn load_info_schema(
        &self,
        store: &dyn kv::Storage,
        _: &str,
    ) -> Result<LoadedInfoSchema, kv::Error> {
        self.load(store.CurrentVersion("global")?.Ver)
    }
    fn load_snapshot_info_schema(
        &self,
        _: &dyn kv::Storage,
        _: &str,
        ts: u64,
    ) -> Result<LoadedInfoSchema, kv::Error> {
        self.load(ts)
    }
    fn keyspace_exists(&self, _: &dyn kv::Storage, keyspace: &str) -> Result<bool, kv::Error> {
        Ok(keyspace == self.keyspace)
    }
}

pub(crate) struct TargetTransport {
    pub server: Arc<dyn serverinfo::EtcdClient>,
    pub schema: Arc<dyn version::EtcdClient>,
}
type StoreOpener = dyn Fn(&str) -> Result<Arc<TargetSessionStore>, ManagerError> + Send + Sync;
type TransportOpener = dyn Fn(&str) -> Result<TargetTransport, ManagerError> + Send + Sync;

struct PreparedTarget {
    store: Arc<TargetSessionStore>,
    domain: Arc<Domain>,
    pool: Arc<SystemSessionPool>,
    coordinator: Arc<crossks::SchemaCoordinator>,
    validator: Arc<astersql_infoschema_isvalidator::Validator>,
    transferred: bool,
}
impl Drop for PreparedTarget {
    fn drop(&mut self) {
        if !self.transferred {
            self.pool.close();
            self.domain.close();
            let _ = crossks::Store::close(self.store.as_ref());
        }
    }
}

/// Opens the target Store before registration and assembles the Go common
/// components. A pending target is owned until registration/bootstrap succeeds.
pub struct KeyspaceSessionFactory {
    store: Arc<StoreOpener>,
    transport: Arc<TransportOpener>,
    prepared: Mutex<HashMap<String, PreparedTarget>>,
    transports: Mutex<HashMap<String, TargetTransport>>,
}
impl KeyspaceSessionFactory {
    pub fn new(
        pd: Vec<String>,
        endpoints: Vec<String>,
        tls: Option<(String, String, String)>,
    ) -> Self {
        let store_pd = pd.clone();
        let store_tls = tls.clone();
        Self::with_openers(
            Arc::new(move |keyspace| {
                let (store, owned) = if keyspace == crossks::SYSTEM_KEYSPACE {
                    let shared = astersql_store::GetSystemStorage()
                        .ok_or_else(|| ManagerError("SYSTEM Store is not initialized".into()))?;
                    (
                        shared
                            .CanonicalTiKVStore()
                            .map_err(|e| ManagerError(e.to_string()))?,
                        false,
                    )
                } else {
                    let query = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("keyspaceName", keyspace)
                        .finish();
                    let options = store_tls.as_ref().map_or_else(Vec::new, |(ca, cert, key)| {
                        vec![astersql_store_driver::WithSecurity(
                            astersql_store_driver::Security {
                                cluster_ssl_ca: ca.clone(),
                                cluster_ssl_cert: cert.clone(),
                                cluster_ssl_key: key.clone(),
                            },
                        )]
                    });
                    let store = astersql_store_driver::TiKVDriver::default()
                        .OpenWithOptions(&format!("tikv://{}?{query}", store_pd.join(",")), options)
                        .map_err(|e| ManagerError(e.to_string()))?;
                    (store, true)
                };
                Ok(TargetSessionStore::new(
                    Arc::new(StorageHandle::new(store)),
                    keyspace.to_owned(),
                    owned,
                ))
            }),
            Arc::new(move |keyspace| {
                let security =
                    tls.as_ref()
                        .map(|(ca, cert, key)| astersql_store_copr::NetworkSecurity {
                            ca_path: ca.clone(),
                            cert_path: cert.clone(),
                            key_path: key.clone(),
                        });
                let pd = astersql_store_copr::NetworkPdKeyspaceClient::connect(
                    &pd,
                    security.as_ref(),
                    Duration::from_secs(5),
                    "astersql-crossks",
                )
                .map_err(|e| ManagerError(e.to_string()))?;
                let id = pd
                    .load_keyspace(keyspace)
                    .map_err(|e| ManagerError(e.to_string()))?;
                let files = tls
                    .as_ref()
                    .map(|(ca, cert, key)| (ca.as_str(), cert.as_str(), key.as_str()));
                let real = Arc::new(
                    serverinfo::RealEtcdClient::connect(endpoints.clone(), files)
                        .map_err(|e| ManagerError(e.to_string()))?
                        .with_namespace(format!("/keyspaces/tidb/{id}")),
                );
                Ok(TargetTransport {
                    schema: Arc::new(
                        version::RealEtcdClient::new(real.clone())
                            .map_err(|e| ManagerError(e.to_string()))?,
                    ),
                    server: real,
                })
            }),
        )
    }
    pub(crate) fn with_openers(store: Arc<StoreOpener>, transport: Arc<TransportOpener>) -> Self {
        Self {
            store,
            transport,
            prepared: Mutex::new(HashMap::new()),
            transports: Mutex::new(HashMap::new()),
        }
    }
    pub fn install_on_domain(
        self: Arc<Self>,
        domain: &Arc<Domain>,
        current: String,
    ) -> Result<(), ManagerError> {
        let provider = self.clone();
        let manager = crossks::new_manager_with_server_info_provider(
            false,
            current,
            self,
            Arc::new(move |keyspace| {
                let transport = (provider.transport)(keyspace)?;
                let server = transport.server.clone();
                provider
                    .transports
                    .lock()
                    .unwrap()
                    .insert(keyspace.to_owned(), transport);
                Ok(Some(server))
            }),
            Arc::new(serverinfo::NoopMinStartTSReporter),
        );
        manager.start_idle_gc()?;
        domain.install_cross_ks_manager(manager);
        Ok(())
    }
}

struct RuntimeLifetime {
    context: version::Context,
    min_cancel: astersql_ddl_systable::Cancellation,
    loops: Mutex<Vec<JoinHandle<()>>>,
    syncer: Arc<dyn version::Syncer>,
    domain: Arc<Domain>,
}
impl Lifecycle for RuntimeLifetime {
    fn close(&self) -> Result<(), ManagerError> {
        self.context.Cancel();
        self.min_cancel.cancel();
        for join in self.loops.lock().unwrap().drain(..) {
            let _ = join.join();
        }
        self.syncer.Close();
        self.domain.close();
        Ok(())
    }
}
impl Drop for RuntimeLifetime {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct SharedSchemaCache(Arc<schema::InfoCache>);
impl crossks::InfoCache for SharedSchemaCache {
    fn schema(&self) -> Option<schema::SchemaInfo> {
        self.0.latest()
    }
}

impl RuntimeFactory for KeyspaceSessionFactory {
    fn prepare(&self, keyspace: &str) -> Result<(), ManagerError> {
        let store = (self.store)(keyspace)?;
        let schema_store: Arc<dyn schema::SchemaStore> =
            Arc::new(schema::KvSchemaStore::from_source(store.storage.clone()));
        let mut config = DomainConfig::default();
        config.keyspace = keyspace.to_owned();
        let domain = Arc::new(Domain::new_with_storage_handle(
            store.storage.clone(),
            Arc::new(SystemSchemaLoader {
                store: schema_store,
                keyspace: keyspace.to_owned(),
            }),
            config,
        ));
        let coordinator = Arc::new(crossks::new_schema_coordinator());
        let borrowed = coordinator.clone();
        let returned = coordinator.clone();
        let destroyed = coordinator.clone();
        let validator: Arc<astersql_infoschema_isvalidator::Validator> = Arc::from(
            astersql_infoschema_isvalidator::new(astersql_sessionctx_vardef::GetSchemaLease()),
        );
        let pool = SystemSessionPool::new_with_validator(
            domain.clone(),
            SystemSessionCallbacks {
                borrowed: Arc::new(move |session| {
                    if let Some(mdl) = super::system_session::transaction_mdl(session.as_ref()) {
                        borrowed.store_internal_session(Arc::new(crossks::RegisteredMDLSession {
                            id: session.session_id(),
                            mdl,
                        }));
                    }
                }),
                returned: Arc::new(move |id| returned.delete_internal_session(id)),
                destroyed: Arc::new(move |id| destroyed.delete_internal_session(id)),
            },
            Some(validator.clone()),
        );
        self.prepared.lock().unwrap().insert(
            keyspace.to_owned(),
            PreparedTarget {
                store,
                domain,
                pool,
                coordinator,
                validator,
                transferred: false,
            },
        );
        Ok(())
    }
    fn create(&self, keyspace: &str) -> Result<Arc<crossks::SessionManager>, ManagerError> {
        self.prepare(keyspace)?;
        self.create_with_server_info(keyspace, &uuid::Uuid::new_v4().to_string())
    }
    fn create_with_server_info(
        &self,
        keyspace: &str,
        id: &str,
    ) -> Result<Arc<crossks::SessionManager>, ManagerError> {
        if !self.prepared.lock().unwrap().contains_key(keyspace) {
            self.prepare(keyspace)?;
        }
        let mut target = self.prepared.lock().unwrap().remove(keyspace).unwrap();
        let transport = match self.transports.lock().unwrap().remove(keyspace) {
            Some(t) => t,
            None => (self.transport)(keyspace)?,
        };
        let protocol = version::NewEtcdSyncer(transport.schema.clone(), id);
        let context = version::Context::Background();
        let lifetime = Arc::new(RuntimeLifetime {
            context: context.clone(),
            min_cancel: Default::default(),
            loops: Mutex::new(Vec::new()),
            syncer: protocol.clone(),
            domain: target.domain.clone(),
        });
        protocol
            .Init(context.clone())
            .map_err(|e| ManagerError(e.to_string()))?;
        let state: Arc<dyn astersql_ddl_serverstate::Syncer> =
            Arc::new(astersql_ddl_serverstate::EtcdSyncer::with_client(
                transport.schema,
                astersql_ddl_util::ServerGlobalState,
            ));
        state
            .get_global_state(&astersql_ddl_serverstate::SyncContext::new())
            .map_err(|e| ManagerError(e.to_string()))?;
        let cache = Arc::new(schema::InfoCache::from_shared(target.domain.info_cache()));
        let schema_store: Arc<dyn schema::SchemaStore> = Arc::new(
            schema::KvSchemaStore::from_source(target.store.storage.clone()),
        );
        let mut syncer = schema::NewCrossKSSyncer(
            Some(schema_store),
            Some(cache.clone()),
            astersql_sessionctx_vardef::GetSchemaLease().as_millis() as u64,
            Some(target.pool.clone()),
            Some(target.validator.clone()),
            keyspace,
        );
        let coordinator = target.coordinator.clone();
        syncer.InitRequiredFields(Arc::new(move || Some(coordinator.clone())), protocol);
        syncer.Reload().map_err(|e| ManagerError(e.to_string()))?;
        target
            .domain
            .init()
            .map_err(|e| ManagerError(e.to_string()))?;
        for table in ["tidb_ddl_job", "tidb_ddl_history"] {
            target.domain.table_by_name("mysql", table).map_err(|e| {
                ManagerError(format!("target is not bootstrapped: mysql.{table}: {e}"))
            })?;
        }
        let manager = astersql_ddl_systable::new_manager(target.pool.clone());
        let refresher = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
            manager.clone(),
        ));
        syncer.SetMinJobIDRefresher(refresher.clone());
        let syncer = Arc::new(syncer);
        let storage = target.store.storage.clone();
        let variables = target.pool.clone();
        let refresh_state = state.clone();
        let backend = crossks::SubmitOnlyBackend::new(
            target.pool.table_mode_submit_options(
                manager,
                refresher.clone(),
                Some(Arc::new(JobSubmitServerState(state))),
            ),
            Arc::new(move || {
                storage
                    .with_storage(|store| {
                        let ts = store.CurrentVersion("global")?;
                        Ok(store.GetSnapshot(ts))
                    })
                    .map_err(|e: kv::Error| crossks::Error(e.to_string()))
            }),
            Arc::new(move || {
                variables
                    .acquire()
                    .map_err(crossks::Error)?
                    .ddl_session_variables()
                    .map_err(crossks::Error)
            }),
            Arc::new(move || {
                refresh_state
                    .get_global_state(&astersql_ddl_serverstate::SyncContext::new())
                    .map(|_| ())
                    .map_err(|e| crossks::Error(e.to_string()))
            }),
            Some(Arc::new(crossks::EtcdOwnerNotifier(transport.server))),
        );
        let worker = syncer.clone();
        let ctx = context.clone();
        lifetime.loops.lock().unwrap().push(
            thread::Builder::new()
                .name("keyspace-schema-sync".into())
                .spawn(move || {
                    if let Err(e) = worker.SyncLoop(ctx) {
                        eprintln!("keyspace schema loop: {e}");
                    }
                })
                .map_err(|e| ManagerError(e.to_string()))?,
        );
        let worker = syncer;
        let ctx = context.clone();
        lifetime.loops.lock().unwrap().push(
            thread::Builder::new()
                .name("keyspace-mdl-check".into())
                .spawn(move || {
                    if let Err(e) = worker.MDLCheckLoop(ctx) {
                        eprintln!("keyspace MDL loop: {e}");
                    }
                })
                .map_err(|e| ManagerError(e.to_string()))?,
        );
        let cancellation = lifetime.min_cancel.clone();
        lifetime.loops.lock().unwrap().push(
            thread::Builder::new()
                .name("keyspace-min-job-id".into())
                .spawn(move || refresher.start(&Default::default(), &cancellation))
                .map_err(|e| ManagerError(e.to_string()))?,
        );
        let runtime = Arc::new(crossks::SessionManager::new(
            target.store.clone(),
            Arc::new(SharedSchemaCache(cache)),
            target.pool.clone(),
            target.coordinator.clone(),
            Arc::new(crossks::DdlClient::new(Arc::new(backend))),
            vec![lifetime],
        ));
        target.transferred = true;
        Ok(runtime)
    }
    fn registration_failed(&self, keyspace: &str) {
        self.transports.lock().unwrap().remove(keyspace);
        self.prepared.lock().unwrap().remove(keyspace);
    }
}

/// Assemble the ordinary schema runtime against the Domain's shared cache.
/// The returned pool and validator must also be used by the ordinary DDL service.
pub fn prepare_normal_schema_runtime(
    domain: &Arc<Domain>,
    protocol: Arc<dyn version::Syncer>,
    lease: Duration,
) -> Result<Arc<super::normal_ddl_service::NormalSchemaRuntime>, String> {
    use super::normal_ddl_service::{
        NormalSchemaCoordinator, NormalSchemaRuntime, SchemaLifecycle,
    };
    if lease.as_millis() == 0 {
        return Err("normal schema lease must be positive".into());
    }
    let lease_millis = lease
        .as_millis()
        .try_into()
        .map_err(|_| "normal schema lease overflow")?;
    let internal = Arc::new(crossks::new_schema_coordinator());
    let borrowed = internal.clone();
    let returned = internal.clone();
    let destroyed = internal.clone();
    let validator: Arc<astersql_infoschema_isvalidator::Validator> =
        Arc::from(astersql_infoschema_isvalidator::new(lease));
    let pool = SystemSessionPool::new_with_validator(
        domain.clone(),
        SystemSessionCallbacks {
            borrowed: Arc::new(move |session| {
                if let Some(mdl) = super::system_session::transaction_mdl(session.as_ref()) {
                    borrowed.store_internal_session(Arc::new(crossks::RegisteredMDLSession {
                        id: session.session_id(),
                        mdl,
                    }));
                }
            }),
            returned: Arc::new(move |id| returned.delete_internal_session(id)),
            destroyed: Arc::new(move |id| destroyed.delete_internal_session(id)),
        },
        Some(validator.clone()),
    );
    let refresher = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
        astersql_ddl_systable::new_manager(pool.clone()),
    ));
    let coordinator = Arc::new(NormalSchemaCoordinator {
        domain: Arc::downgrade(domain),
        internal,
    });
    let mut syncer = schema::New(
        Some(Arc::new(schema::KvSchemaStore::from_source(
            domain.storage_handle(),
        ))),
        Some(Arc::new(schema::InfoCache::from_shared(
            domain.info_cache(),
        ))),
        lease_millis,
        Some(pool.clone()),
        Some(validator.clone()),
        None,
    );
    syncer.InitRequiredFields(
        Arc::new(move || Some(coordinator.clone())),
        protocol.clone(),
    );
    syncer.SetMinJobIDRefresher(refresher.clone());
    let runtime = Arc::new(NormalSchemaRuntime {
        syncer: Arc::new(syncer),
        validator,
        pool,
        refresher,
        protocol,
        context: version::Context::Background(),
        min_cancel: Default::default(),
        lifecycle: Mutex::new(SchemaLifecycle {
            started: false,
            closed: false,
            loops: Vec::new(),
        }),
    });
    // RAII closes the protocol and pool if Init or the initial reload fails.
    runtime
        .protocol
        .Init(runtime.context.clone())
        .map_err(|e| e.to_string())?;
    runtime
        .syncer
        .ReloadWithContext(runtime.context.clone())
        .map_err(|e| e.to_string())?;
    Ok(runtime)
}
