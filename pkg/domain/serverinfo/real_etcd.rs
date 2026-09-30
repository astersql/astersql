// Copyright 2026 AsterSQL.

//! Synchronous server-info operations over the production etcd client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use etcd_client::{
    Certificate, Client, Compare, CompareOp, ConnectOptions, DeleteOptions, GetOptions, Identity,
    PutOptions, TlsOptions, Txn, TxnOp,
};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

use crate::{Context, EtcdClient, KeyValue, SyncError};

/// Owns a connected etcd client and keeps granted server-info leases alive.
pub struct RealEtcdClient {
    runtime: Arc<Runtime>,
    client: Mutex<Client>,
    keepalive: Mutex<HashMap<i64, JoinHandle<()>>>,
    namespace: String,
}

impl RealEtcdClient {
    /// Connect to the PD embedded etcd endpoints used by the serving Domain.
    pub fn connect(
        endpoints: Vec<String>,
        tls_files: Option<(&str, &str, &str)>,
    ) -> Result<Self, SyncError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| SyncError(format!("start etcd runtime: {error}")))?;
        let mut options = ConnectOptions::new()
            .with_connect_timeout(Duration::from_secs(5))
            .with_keep_alive(Duration::from_secs(10), Duration::from_secs(3))
            .with_keep_alive_while_idle(true);
        if let Some((ca_path, cert_path, key_path)) = tls_files {
            let ca = std::fs::read(ca_path)
                .map_err(|error| SyncError(format!("read etcd CA: {error}")))?;
            let cert = std::fs::read(cert_path)
                .map_err(|error| SyncError(format!("read etcd certificate: {error}")))?;
            let key = std::fs::read(key_path)
                .map_err(|error| SyncError(format!("read etcd private key: {error}")))?;
            options = options.with_tls(
                TlsOptions::new()
                    .ca_certificate(Certificate::from_pem(ca))
                    .identity(Identity::from_pem(cert, key)),
            );
        }
        let client = runtime
            .block_on(Client::connect(endpoints, Some(options)))
            .map_err(|error| SyncError(format!("connect etcd: {error}")))?;
        Ok(Self {
            runtime: Arc::new(runtime),
            client: Mutex::new(client),
            keepalive: Mutex::new(HashMap::new()),
            namespace: String::new(),
        })
    }

    /// Wrap a connected client. The caller supplies TLS and namespace policy.
    pub fn new(client: Client) -> Result<Self, SyncError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| SyncError(format!("start etcd runtime: {error}")))?;
        Ok(Self {
            runtime: Arc::new(runtime),
            client: Mutex::new(client),
            keepalive: Mutex::new(HashMap::new()),
            namespace: String::new(),
        })
    }

    /// Apply the numeric PD keyspace namespace to every etcd key.
    pub fn with_namespace(mut self, namespace: String) -> Self {
        self.namespace = namespace;
        self
    }

    fn key(&self, key: &str) -> String {
        format!("{}{key}", self.namespace)
    }

    fn client(&self) -> Client {
        self.client
            .lock()
            .expect("etcd client lock poisoned")
            .clone()
    }

    /// Share the connected client with the Go-compatible owner election
    /// manager. Callers must prefix owner keys with this client's namespace.
    pub fn raw_client(&self) -> Client {
        self.client()
    }

    /// Numeric PD keyspace namespace applied to server-info keys.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

impl EtcdClient for RealEtcdClient {
    fn GrantLease(&self, context: &Context, ttl: i32) -> Result<i64, SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        let lease = self
            .runtime
            .block_on(async { client.lease_grant(i64::from(ttl), None).await })
            .map_err(|error| SyncError(format!("grant etcd lease: {error}")))?
            .id();
        let mut client = self.client();
        let interval = Duration::from_secs((ttl.max(1) as u64 / 3).max(1));
        let task = self.runtime.spawn(async move {
            loop {
                let Ok((mut keeper, mut stream)) = client.lease_keep_alive(lease).await else {
                    tokio::time::sleep(interval).await;
                    continue;
                };
                loop {
                    tokio::time::sleep(interval).await;
                    if keeper.keep_alive().await.is_err()
                        || !matches!(stream.message().await, Ok(Some(_)))
                    {
                        break;
                    }
                }
            }
        });
        self.keepalive
            .lock()
            .expect("etcd keepalive lock poisoned")
            .insert(lease, task);
        Ok(lease)
    }

    fn Get(&self, context: &Context, key: &str, prefix: bool) -> Result<Vec<KeyValue>, SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        let physical_key = self.key(key);
        let options = prefix.then(|| GetOptions::new().with_prefix());
        let response = self
            .runtime
            .block_on(async { client.get(physical_key, options).await })
            .map_err(|error| SyncError(format!("get etcd key {key}: {error}")))?;
        Ok(response
            .kvs()
            .iter()
            .map(|entry| KeyValue {
                key: String::from_utf8_lossy(entry.key())
                    .strip_prefix(&self.namespace)
                    .unwrap_or_default()
                    .to_owned(),
                value: entry.value().to_vec(),
                lease: (entry.lease() != 0).then_some(entry.lease()),
            })
            .collect())
    }

    fn Put(
        &self,
        context: &Context,
        key: &str,
        value: Vec<u8>,
        lease: Option<i64>,
    ) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        let physical_key = self.key(key);
        let options = lease.map(|lease| PutOptions::new().with_lease(lease));
        self.runtime
            .block_on(async { client.put(physical_key, value, options).await })
            .map_err(|error| SyncError(format!("put etcd key {key}: {error}")))?;
        Ok(())
    }

    fn Delete(&self, context: &Context, key: &str) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        let physical_key = self.key(key);
        self.runtime
            .block_on(async { client.delete(physical_key, None).await })
            .map_err(|error| SyncError(format!("delete etcd key {key}: {error}")))?;
        Ok(())
    }

    fn DeletePrefix(&self, context: &Context, key: &str) -> Result<(), SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        let physical_key = self.key(key);
        self.runtime
            .block_on(async {
                client
                    .delete(physical_key, Some(DeleteOptions::new().with_prefix()))
                    .await
            })
            .map_err(|error| SyncError(format!("delete etcd prefix {key}: {error}")))?;
        Ok(())
    }

    fn RevokeLease(&self, context: &Context, lease: i64) -> Result<(), SyncError> {
        if let Some(task) = self
            .keepalive
            .lock()
            .expect("etcd keepalive lock poisoned")
            .remove(&lease)
        {
            task.abort();
        }
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let mut client = self.client();
        self.runtime
            .block_on(async { client.lease_revoke(lease).await })
            .map_err(|error| SyncError(format!("revoke etcd lease {lease}: {error}")))?;
        Ok(())
    }

    fn CompareAndPut(
        &self,
        context: &Context,
        key: &str,
        expected: Option<(&[u8], Option<i64>)>,
        value: Vec<u8>,
        lease: i64,
    ) -> Result<bool, SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let physical_key = self.key(key);
        let compare = match expected {
            None => vec![Compare::create_revision(
                physical_key.clone(),
                CompareOp::Equal,
                0,
            )],
            Some((old_value, old_lease)) => vec![
                Compare::value(physical_key.clone(), CompareOp::Equal, old_value),
                Compare::lease(
                    physical_key.clone(),
                    CompareOp::Equal,
                    old_lease.unwrap_or_default(),
                ),
            ],
        };
        let txn = Txn::new().when(compare).and_then([TxnOp::put(
            physical_key,
            value,
            Some(PutOptions::new().with_lease(lease)),
        )]);
        let mut client = self.client();
        self.runtime
            .block_on(async { client.txn(txn).await })
            .map(|response| response.succeeded())
            .map_err(|error| SyncError(format!("compare-and-put etcd key {key}: {error}")))
    }

    fn CompareAndDelete(
        &self,
        context: &Context,
        key: &str,
        expected: (&[u8], i64),
    ) -> Result<bool, SyncError> {
        if context.Done() {
            return Err(SyncError("context cancelled".into()));
        }
        let physical_key = self.key(key);
        let txn = Txn::new()
            .when([
                Compare::value(physical_key.clone(), CompareOp::Equal, expected.0),
                Compare::lease(physical_key.clone(), CompareOp::Equal, expected.1),
            ])
            .and_then([TxnOp::delete(physical_key, None)]);
        let mut client = self.client();
        self.runtime
            .block_on(async { client.txn(txn).await })
            .map(|response| response.succeeded())
            .map_err(|error| SyncError(format!("compare-and-delete etcd key {key}: {error}")))
    }
}

impl Drop for RealEtcdClient {
    fn drop(&mut self) {
        for (_, task) in self
            .keepalive
            .lock()
            .expect("etcd keepalive lock poisoned")
            .drain()
        {
            task.abort();
        }
    }
}
