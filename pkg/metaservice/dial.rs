// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    Context, GROUP_ID_KEY, KeyspaceMeta, MetaServiceError, PdClient, PdMember, get_info,
    get_pd_addrs,
};
use etcd_client::{Client, ConnectOptions, GetOptions, PutOptions, TlsOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

/// Full metadata needed by the transaction codec and meta-service routing.
#[derive(Clone, Debug)]
pub struct DialKeyspaceMeta {
    pub id: u32,
    pub name: String,
    pub config: HashMap<String, String>,
}

/// PD RPC boundary: discovery is V1 and keyspace metadata is loaded explicitly once.
pub trait MetadataPdClient: PdClient {
    fn load_keyspace(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Option<DialKeyspaceMeta>, MetaServiceError>;
    fn close(&self);
}

#[derive(Clone, Debug, Default)]
pub struct PdSecurity {
    pub ca: String,
    pub cert: String,
    pub key: String,
}

impl PdSecurity {
    /// Load the same CA and client identity used by the PD connection into etcd.
    pub fn etcd_tls(&self) -> Result<Option<TlsOptions>, MetaServiceError> {
        if self.ca.is_empty() && self.cert.is_empty() && self.key.is_empty() {
            return Ok(None);
        }
        if self.cert.is_empty() != self.key.is_empty() {
            return Err(MetaServiceError::ServiceUrl(
                "certificate and private key must be configured together".into(),
            ));
        }
        let read = |path: &str| {
            std::fs::read(path).map_err(|error| {
                MetaServiceError::ServiceUrl(format!("read TLS file {path:?}: {error}"))
            })
        };
        let mut tls = TlsOptions::new();
        if !self.ca.is_empty() {
            tls = tls.ca_certificate(etcd_client::Certificate::from_pem(read(&self.ca)?));
        }
        if !self.cert.is_empty() {
            tls = tls.identity(etcd_client::Identity::from_pem(
                read(&self.cert)?,
                read(&self.key)?,
            ));
        }
        Ok(Some(tls))
    }
}

pub type PdClientFactory = Arc<
    dyn Fn(&Context, &[String], &PdSecurity) -> Result<Arc<dyn MetadataPdClient>, MetaServiceError>
        + Send
        + Sync,
>;

// An owned reactor keeps synchronous BR/Lightning callers out of nested Tokio runtimes.
// Its final owner joins the thread only after queued RPCs have completed.
type Work = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send>;
enum WorkerMessage {
    Call(Work),
    Stop,
}
struct RpcWorker {
    sender: mpsc::Sender<WorkerMessage>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}
impl RpcWorker {
    fn new() -> Result<Arc<Self>, MetaServiceError> {
        let (sender, receiver) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let _ = ready.send(Err("create metadata reactor".to_owned()));
                return;
            };
            let _ = ready.send(Ok(()));
            while let Ok(WorkerMessage::Call(work)) = receiver.recv() {
                work(&runtime);
            }
        });
        started
            .recv()
            .map_err(|error| MetaServiceError::Pd(error.to_string()))?
            .map_err(MetaServiceError::Pd)?;
        Ok(Arc::new(Self {
            sender,
            thread: Mutex::new(Some(thread)),
        }))
    }
    fn call<T: Send + 'static>(
        &self,
        call: impl FnOnce(&tokio::runtime::Runtime) -> Result<T, MetaServiceError> + Send + 'static,
    ) -> Result<T, MetaServiceError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(WorkerMessage::Call(Box::new(move |runtime| {
                let _ = sender.send(call(runtime));
            })))
            .map_err(|error| MetaServiceError::Pd(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| MetaServiceError::Pd(error.to_string()))?
    }
}
impl Drop for RpcWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerMessage::Stop);
        if let Some(thread) = self.thread.get_mut().unwrap().take() {
            let _ = thread.join();
        }
    }
}
async fn checked<T>(
    context: &Context,
    future: impl std::future::Future<Output = Result<T, MetaServiceError>>,
) -> Result<T, MetaServiceError> {
    if context.is_cancelled() {
        return Err(MetaServiceError::Cancelled);
    }
    tokio::select! {
        result = future => result,
        _ = async { while !context.is_cancelled() { tokio::time::sleep(Duration::from_millis(10)).await; } } => Err(MetaServiceError::Cancelled),
    }
}

struct NetworkPd {
    worker: Arc<RpcWorker>,
    client: Mutex<Option<Arc<tikv_client::MetadataClient>>>,
}
impl NetworkPd {
    fn client(&self) -> Result<Arc<tikv_client::MetadataClient>, MetaServiceError> {
        self.client
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| MetaServiceError::Pd("PD client is closed".into()))
    }
}
impl PdClient for NetworkPd {
    fn get_all_members(&self, context: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        let client = self.client()?;
        let context = context.clone();
        self.worker.call(move |runtime| {
            runtime.block_on(checked(&context, async {
                let response = client
                    .get_members()
                    .await
                    .map_err(|error| MetaServiceError::Pd(error.to_string()))?;
                Ok(response
                    .members
                    .into_iter()
                    .map(|member| PdMember {
                        client_urls: member.client_urls,
                    })
                    .collect())
            }))
        })
    }
}
impl MetadataPdClient for NetworkPd {
    fn load_keyspace(
        &self,
        context: &Context,
        name: &str,
    ) -> Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        let client = self.client()?;
        let context = context.clone();
        let name = name.to_owned();
        self.worker.call(move |runtime| {
            runtime.block_on(checked(&context, async {
                match client.load_keyspace(&name).await {
                    Ok(meta) => Ok(Some(DialKeyspaceMeta {
                        id: meta.id,
                        name: meta.name,
                        config: meta.config,
                    })),
                    Err(tikv_client::Error::KeyspaceNotFound(_)) => Ok(None),
                    Err(error) => Err(MetaServiceError::Pd(error.to_string())),
                }
            }))
        })
    }
    fn close(&self) {
        self.client.lock().unwrap().take();
    }
}

pub fn ConnectMetadataPD(
    context: &Context,
    endpoints: &[String],
    security: &PdSecurity,
) -> Result<Arc<dyn MetadataPdClient>, MetaServiceError> {
    let worker = RpcWorker::new()?;
    let context = context.clone();
    let endpoints = endpoints.to_vec();
    let security = security.clone();
    let client = worker.call(move |runtime| {
        runtime.block_on(checked(&context, async {
            let security =
                if security.ca.is_empty() && security.cert.is_empty() && security.key.is_empty() {
                    tikv_client::SecurityManager::default()
                } else {
                    tikv_client::SecurityManager::load(security.ca, security.cert, security.key)
                        .map_err(|error| MetaServiceError::Pd(error.to_string()))?
                };
            tikv_client::MetadataClient::connect(
                &endpoints,
                Arc::new(security),
                Duration::from_secs(5),
            )
            .await
            .map(Arc::new)
            .map_err(|error| MetaServiceError::Pd(error.to_string()))
        }))
    })?;
    Ok(Arc::new(NetworkPd {
        worker,
        client: Mutex::new(Some(client)),
    }))
}

#[derive(Clone)]
pub struct EtcdDialConfig {
    pub tls: Option<TlsOptions>,
    pub dial_timeout: Duration,
    pub keepalive_time: Duration,
    pub keepalive_timeout: Duration,
    pub permit_without_stream: bool,
}
impl Default for EtcdDialConfig {
    fn default() -> Self {
        Self {
            tls: None,
            dial_timeout: Duration::from_secs(5),
            keepalive_time: Duration::from_secs(10),
            keepalive_timeout: Duration::from_secs(3),
            permit_without_stream: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EtcdDialInfo {
    pub endpoints: Vec<String>,
    pub namespace: String,
}
pub fn ResolveEtcdDialInfo(
    context: &Context,
    pd: &dyn MetadataPdClient,
    meta: Option<&DialKeyspaceMeta>,
    caller_endpoints: &[String],
) -> Result<EtcdDialInfo, MetaServiceError> {
    let mut endpoints: Vec<_> = caller_endpoints
        .iter()
        .filter(|address| !address.is_empty())
        .cloned()
        .collect();
    if endpoints.is_empty() && meta.is_none_or(|meta| !meta.config.contains_key(GROUP_ID_KEY)) {
        endpoints = get_pd_addrs(context, pd, false)?;
    }
    let group_meta = meta.map(|meta| KeyspaceMeta {
        name: meta.name.clone(),
        config: meta.config.clone(),
    });
    let info = get_info(group_meta.as_ref(), &endpoints)?;
    let namespace = match meta {
        Some(meta) if meta.id > 0x00ff_ffff => {
            return Err(MetaServiceError::Pd(format!(
                "invalid keyspace id {}",
                meta.id
            )));
        }
        Some(meta) => format!("/keyspaces/tidb/{}", meta.id),
        None => String::new(),
    };
    Ok(EtcdDialInfo {
        endpoints: info.group_addrs().to_vec(),
        namespace,
    })
}

struct EtcdSession {
    worker: Arc<RpcWorker>,
    client: Arc<Mutex<Option<Client>>>,
}
#[derive(Clone)]
pub struct NamespacedEtcdClient {
    session: Arc<EtcdSession>,
    endpoints: Arc<Vec<String>>,
    namespace: Arc<str>,
    context: Context,
}
impl std::fmt::Debug for NamespacedEtcdClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamespacedEtcdClient")
            .field("endpoints", &self.endpoints)
            .field("namespace", &self.namespace)
            .field("closed", &self.is_closed())
            .finish()
    }
}
#[derive(Debug, Clone)]
pub struct EtcdEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub lease: i64,
}

impl NamespacedEtcdClient {
    pub fn with_context(&self, context: Context) -> Self {
        Self {
            context,
            ..self.clone()
        }
    }
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn is_closed(&self) -> bool {
        self.session.client.lock().unwrap().is_none()
    }
    fn key(&self, key: &str) -> String {
        format!("{}{key}", self.namespace)
    }
    fn call<T: Send + 'static>(
        &self,
        action: impl FnOnce(Client, Context, &tokio::runtime::Runtime) -> Result<T, MetaServiceError>
        + Send
        + 'static,
    ) -> Result<T, MetaServiceError> {
        let client = self
            .session
            .client
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| MetaServiceError::Pd("etcd client is closed".into()))?;
        let context = self.context.clone();
        self.session
            .worker
            .call(move |runtime| action(client, context, runtime))
    }
    pub fn put(
        &self,
        key: &str,
        value: Vec<u8>,
        lease: Option<i64>,
    ) -> Result<(), MetaServiceError> {
        let key = self.key(key);
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                client
                    .put(key, value, lease.map(|id| PutOptions::new().with_lease(id)))
                    .await
                    .map(|_| ())
                    .map_err(MetaServiceError::Etcd)
            }))
        })
    }
    pub fn get(
        &self,
        key: &str,
        prefix: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, MetaServiceError> {
        Ok(self
            .get_entries(key, prefix)?
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect())
    }
    pub fn get_entries(&self, key: &str, prefix: bool) -> Result<Vec<EtcdEntry>, MetaServiceError> {
        let key = self.key(key);
        let namespace = self.namespace.clone();
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                let response = client
                    .get(key, prefix.then(|| GetOptions::new().with_prefix()))
                    .await
                    .map_err(MetaServiceError::Etcd)?;
                Ok(response
                    .kvs()
                    .iter()
                    .map(|kv| EtcdEntry {
                        key: kv
                            .key()
                            .strip_prefix(namespace.as_bytes())
                            .unwrap_or(kv.key())
                            .to_vec(),
                        value: kv.value().to_vec(),
                        lease: kv.lease(),
                    })
                    .collect())
            }))
        })
    }
    pub fn time_to_live(&self, lease: i64) -> Result<i64, MetaServiceError> {
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                client
                    .lease_time_to_live(lease, None)
                    .await
                    .map(|response| response.ttl())
                    .map_err(MetaServiceError::Etcd)
            }))
        })
    }
    pub fn delete(&self, key: &str) -> Result<(), MetaServiceError> {
        let key = self.key(key);
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                client
                    .delete(key, None)
                    .await
                    .map(|_| ())
                    .map_err(MetaServiceError::Etcd)
            }))
        })
    }
    pub fn grant(&self, ttl: i64) -> Result<i64, MetaServiceError> {
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                client
                    .lease_grant(ttl, None)
                    .await
                    .map(|response| response.id())
                    .map_err(MetaServiceError::Etcd)
            }))
        })
    }
    pub fn keepalive(&self, id: i64) -> Result<(), MetaServiceError> {
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                let (mut keeper, mut stream) = client
                    .lease_keep_alive(id)
                    .await
                    .map_err(MetaServiceError::Etcd)?;
                keeper.keep_alive().await.map_err(MetaServiceError::Etcd)?;
                let response = stream
                    .message()
                    .await
                    .map_err(MetaServiceError::Etcd)?
                    .ok_or_else(|| {
                        MetaServiceError::Pd("etcd lease keepalive stream closed".into())
                    })?;
                if response.ttl() <= 0 {
                    return Err(MetaServiceError::Pd("etcd lease expired".into()));
                }
                Ok(())
            }))
        })
    }
    pub fn revoke(&self, id: i64) -> Result<(), MetaServiceError> {
        self.call(move |mut client, context, runtime| {
            runtime.block_on(checked(&context, async {
                client
                    .lease_revoke(id)
                    .await
                    .map(|_| ())
                    .map_err(MetaServiceError::Etcd)
            }))
        })
    }
    pub fn close(&self) -> Result<(), MetaServiceError> {
        let session = self.session.client.clone();
        self.session.worker.call(move |_| {
            session.lock().unwrap().take();
            Ok(())
        })
    }
}

pub fn NewEtcdClientFromPDClient(
    context: &Context,
    pd: &dyn MetadataPdClient,
    meta: Option<&DialKeyspaceMeta>,
    caller_endpoints: &[String],
    config: EtcdDialConfig,
) -> Result<NamespacedEtcdClient, MetaServiceError> {
    let info = ResolveEtcdDialInfo(context, pd, meta, caller_endpoints)?;
    let worker = RpcWorker::new()?;
    let endpoints = info.endpoints.clone();
    let ctx = context.clone();
    let client = worker.call(move |runtime| {
        runtime.block_on(checked(&ctx, async {
            let mut options = ConnectOptions::new().with_connect_timeout(config.dial_timeout);
            // gRPC treats a zero interval as disabled and a zero timeout as its default.
            if !config.keepalive_time.is_zero() {
                let timeout = if config.keepalive_timeout.is_zero() {
                    Duration::from_secs(20)
                } else {
                    config.keepalive_timeout
                };
                options = options
                    .with_keep_alive(config.keepalive_time, timeout)
                    .with_keep_alive_while_idle(config.permit_without_stream);
            }
            if let Some(tls) = config.tls {
                options = options.with_tls(tls);
            }
            Client::connect(endpoints, Some(options))
                .await
                .map_err(MetaServiceError::Etcd)
        }))
    })?;
    Ok(NamespacedEtcdClient {
        session: Arc::new(EtcdSession {
            worker,
            client: Arc::new(Mutex::new(Some(client))),
        }),
        endpoints: Arc::new(info.endpoints),
        namespace: Arc::from(info.namespace),
        context: context.clone(),
    })
}

pub fn DialEtcdClient(
    context: &Context,
    keyspace_name: &str,
    endpoints: &[String],
    security: &PdSecurity,
    factory: Option<&PdClientFactory>,
    config: EtcdDialConfig,
) -> Result<NamespacedEtcdClient, MetaServiceError> {
    let pd = match factory {
        Some(factory) => factory(context, endpoints, security)?,
        None => ConnectMetadataPD(context, endpoints, security)?,
    };
    let result = (|| {
        let meta = if keyspace_name.is_empty() {
            None
        } else {
            Some(
                pd.load_keyspace(context, keyspace_name)?
                    .ok_or_else(|| MetaServiceError::MissingKeyspaceMeta(keyspace_name.into()))?,
            )
        };
        NewEtcdClientFromPDClient(context, pd.as_ref(), meta.as_ref(), endpoints, config)
    })();
    pd.close();
    result
}

/// A storage-owned PD client and its resolved codec metadata. The existing-client
/// path borrows these resources and never closes the storage's PD client.
pub trait EtcdMetadataStore: Send + Sync {
    fn pd_client(&self) -> Result<Arc<dyn MetadataPdClient>, MetaServiceError>;
    fn keyspace_meta(&self) -> Result<Option<DialKeyspaceMeta>, MetaServiceError>;
}

/// Borrow the storage-owned PD client and full codec metadata without closing PD.
pub fn NewEtcdClientFromStore(
    context: &Context,
    store: &dyn EtcdMetadataStore,
    caller_endpoints: &[String],
    config: EtcdDialConfig,
) -> Result<NamespacedEtcdClient, MetaServiceError> {
    let pd = store.pd_client()?;
    let meta = store.keyspace_meta()?;
    NewEtcdClientFromPDClient(
        context,
        pd.as_ref(),
        meta.as_ref(),
        caller_endpoints,
        config,
    )
}
