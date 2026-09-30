// Copyright 2026 AsterSQL.

//! Target-keyspace schema-version publication and global server state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use astersql_ddl_jobsubmit::ServerState;
use astersql_domain::Domain;
use astersql_domain_crossks::{Lifecycle, ManagerError};
use astersql_domain_serverinfo::{Context, EtcdClient};

const GLOBAL_VERSION_KEY: &str = "/tidb/ddl/global_schema_version";
const ALL_VERSIONS_KEY: &str = "/tidb/ddl/all_schema_versions";
const GLOBAL_STATE_KEY: &str = "/tidb/server/global_state";

/// Publishes the target Domain's loaded schema version under its own leased
/// server ID and reloads when another DDL owner advances the global version.
pub struct CrossKSSchemaSyncer {
    domain: Arc<Domain>,
    etcd: Arc<dyn EtcdClient>,
    id: String,
    lease: Mutex<Option<i64>>,
    stopped: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CrossKSSchemaSyncer {
    pub fn new(domain: Arc<Domain>, etcd: Arc<dyn EtcdClient>, id: String) -> Arc<Self> {
        Arc::new(Self {
            domain,
            etcd,
            id,
            lease: Mutex::new(None),
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let context = Context::Background();
        let lease = self
            .etcd
            .GrantLease(&context, 90)
            .map_err(|error| error.to_string())?;
        *self
            .lease
            .lock()
            .expect("schema syncer lease lock poisoned") = Some(lease);
        let version = self.domain.info_schema().SchemaMetaVersion();
        if let Err(error) = self.publish_self(version) {
            self.close();
            return Err(error);
        }
        let syncer = Arc::clone(self);
        let thread = thread::Builder::new()
            .name(format!("crossks-schema-{}", self.id))
            .spawn(move || syncer.run())
            .map_err(|error| {
                self.close();
                format!("start cross-keyspace schema syncer: {error}")
            })?;
        *self.thread.lock().expect("schema syncer lock poisoned") = Some(thread);
        Ok(())
    }

    fn self_key(&self) -> String {
        format!("{ALL_VERSIONS_KEY}/{}", self.id)
    }

    fn publish_self(&self, version: i64) -> Result<(), String> {
        let lease = self
            .lease
            .lock()
            .expect("schema syncer lease lock poisoned")
            .ok_or_else(|| "schema version lease is absent".to_owned())?;
        self.etcd
            .Put(
                &Context::Background(),
                &self.self_key(),
                version.to_string().into_bytes(),
                Some(lease),
            )
            .map_err(|error| error.to_string())
    }

    /// Publish a completed DDL version for all target-keyspace instances.
    pub fn publish_global(&self) -> Result<(), String> {
        let version = self.domain.info_schema().SchemaMetaVersion();
        self.etcd
            .Put(
                &Context::Background(),
                GLOBAL_VERSION_KEY,
                version.to_string().into_bytes(),
                None,
            )
            .map_err(|error| error.to_string())?;
        self.publish_self(version)
    }

    /// Wait until every registered target-keyspace server reports the new
    /// schema version, matching the DDL owner's post-commit sync barrier.
    pub fn wait_all_versions(&self, timeout: Duration) -> Result<(), String> {
        self.wait_all_versions_with_cancel(timeout, &self.stopped)
    }

    pub fn wait_all_versions_with_cancel(
        &self,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let target = self.domain.info_schema().SchemaMetaVersion();
        let deadline = Instant::now() + timeout;
        loop {
            if self.stopped.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                return Err("schema syncer closed while waiting for versions".into());
            }
            let context = Context::Background();
            let servers = self
                .etcd
                .Get(&context, "/tidb/server/info/", true)
                .map_err(|error| error.to_string())?;
            let versions = self
                .etcd
                .Get(&context, ALL_VERSIONS_KEY, true)
                .map_err(|error| error.to_string())?;
            let synced = servers.iter().all(|server| {
                let Some(id) = server.key.rsplit('/').next() else {
                    return false;
                };
                versions.iter().any(|entry| {
                    entry.key == format!("{ALL_VERSIONS_KEY}/{id}")
                        && String::from_utf8_lossy(&entry.value)
                            .parse::<i64>()
                            .ok()
                            .is_some_and(|version| version >= target)
                })
            });
            if synced {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "target schema version {target} was not synchronized before timeout"
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn reload(&self) -> Result<(), String> {
        self.domain.reload().map_err(|error| error.to_string())?;
        self.publish_self(self.domain.info_schema().SchemaMetaVersion())
    }

    fn run(&self) {
        while !self.stopped.load(Ordering::Acquire) {
            if let Ok(values) = self
                .etcd
                .Get(&Context::Background(), GLOBAL_VERSION_KEY, false)
            {
                if let Some(value) = values.first() {
                    if let Ok(global) = String::from_utf8_lossy(&value.value).parse::<i64>() {
                        if global > self.domain.info_schema().SchemaMetaVersion() {
                            let _ = self.reload();
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn close(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(thread) = self
            .thread
            .lock()
            .expect("schema syncer lock poisoned")
            .take()
        {
            let _ = thread.join();
        }
        if let Some(lease) = self
            .lease
            .lock()
            .expect("schema syncer lease lock poisoned")
            .take()
        {
            let _ = self.etcd.RevokeLease(&Context::Background(), lease);
        }
    }
}

impl Lifecycle for CrossKSSchemaSyncer {
    fn close(&self) -> Result<(), ManagerError> {
        self.close();
        Ok(())
    }
}

/// Reads the Go-compatible global server state from the target keyspace's
/// etcd namespace and exposes its upgrading flag to durable job submission.
pub struct CrossKSStateSyncer {
    etcd: Arc<dyn EtcdClient>,
    upgrading: AtomicBool,
}

impl CrossKSStateSyncer {
    pub fn new(etcd: Arc<dyn EtcdClient>) -> Arc<Self> {
        Arc::new(Self {
            etcd,
            upgrading: AtomicBool::new(false),
        })
    }

    pub fn refresh(&self) -> Result<(), String> {
        let values = self
            .etcd
            .Get(&Context::Background(), GLOBAL_STATE_KEY, false)
            .map_err(|error| error.to_string())?;
        let upgrading = match values.first() {
            Some(value) => {
                let json: serde_json::Value =
                    serde_json::from_slice(&value.value).map_err(|error| error.to_string())?;
                json.get("state").and_then(serde_json::Value::as_str) == Some("upgrading")
            }
            None => false,
        };
        self.upgrading.store(upgrading, Ordering::Release);
        Ok(())
    }
}

impl ServerState for CrossKSStateSyncer {
    fn is_upgrading(&self) -> bool {
        self.upgrading.load(Ordering::Acquire)
    }
}
