// Copyright 2026 AsterSQL.

//! Target-keyspace TiKV store ownership for cross-keyspace runtimes.

use std::sync::Arc;

use astersql_domain_crossks::{ManagerError, Store};
use astersql_store_driver::{Security, TiKVDriver, TikvStore, WithSecurity};

/// Owns the real target-keyspace client opened through the TiKV driver.
pub struct CrossKSStore {
    keyspace: String,
    inner: TikvStore,
}

impl CrossKSStore {
    pub fn inner(&self) -> &TikvStore {
        &self.inner
    }
}

impl Store for CrossKSStore {
    fn keyspace(&self) -> &str {
        &self.keyspace
    }

    fn close(&self) -> Result<(), ManagerError> {
        self.inner
            .Close()
            .map_err(|error| ManagerError(format!("close keyspace {}: {error}", self.keyspace)))
    }

    fn close_on_runtime_shutdown(&self) -> bool {
        true
    }
}

/// Open a separate TiKV client for a target keyspace using the serving PD set.
pub fn open_target_store(
    pd_endpoints: &[String],
    keyspace: &str,
) -> Result<Arc<CrossKSStore>, ManagerError> {
    open_target_store_with_tls(pd_endpoints, keyspace, None)
}

/// Open with the serving Store's TLS configuration even if global defaults
/// have changed since the serving Store was created.
pub fn open_target_store_with_tls(
    pd_endpoints: &[String],
    keyspace: &str,
    tls_files: Option<&(String, String, String)>,
) -> Result<Arc<CrossKSStore>, ManagerError> {
    open_target_store_with_driver_and_tls(
        &mut TiKVDriver::default(),
        pd_endpoints,
        keyspace,
        tls_files,
    )
}

pub(crate) fn open_target_store_with_driver(
    driver: &mut TiKVDriver,
    pd_endpoints: &[String],
    keyspace: &str,
) -> Result<Arc<CrossKSStore>, ManagerError> {
    open_target_store_with_driver_and_tls(driver, pd_endpoints, keyspace, None)
}

fn open_target_store_with_driver_and_tls(
    driver: &mut TiKVDriver,
    pd_endpoints: &[String],
    keyspace: &str,
    tls_files: Option<&(String, String, String)>,
) -> Result<Arc<CrossKSStore>, ManagerError> {
    if pd_endpoints.is_empty() || keyspace.is_empty() {
        return Err(ManagerError(
            "target keyspace and PD endpoints are required".into(),
        ));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("keyspaceName", keyspace)
        .finish();
    let path = format!("tikv://{}?{query}", pd_endpoints.join(","));
    let options = tls_files.map_or_else(Vec::new, |(ca, cert, key)| {
        vec![WithSecurity(Security {
            cluster_ssl_ca: ca.clone(),
            cluster_ssl_cert: cert.clone(),
            cluster_ssl_key: key.clone(),
        })]
    });
    let store = driver
        .OpenWithOptions(&path, options)
        .map_err(|error| ManagerError(format!("open keyspace {keyspace}: {error}")))?;
    Ok(Arc::new(CrossKSStore {
        keyspace: keyspace.to_owned(),
        inner: store,
    }))
}
