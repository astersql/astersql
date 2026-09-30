// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_domain_crossks::Store;
use astersql_store_driver::{InMemoryBackend, TiKVDriver};

use super::crossks_store::open_target_store_with_driver;

#[test]
fn go_merge_43_target_keyspace_store_uses_distinct_driver_identity_and_closes() {
    let mut driver = TiKVDriver::with_backend(Arc::new(InMemoryBackend::default()));
    let endpoints = vec!["pd:2379".to_owned()];
    let tenant = open_target_store_with_driver(&mut driver, &endpoints, "tenant/a").unwrap();
    let system = open_target_store_with_driver(&mut driver, &endpoints, "SYSTEM").unwrap();
    assert_eq!(tenant.keyspace(), "tenant/a");
    assert_eq!(tenant.inner().GetKeyspace(), "tenant/a");
    assert_eq!(system.inner().GetKeyspace(), "SYSTEM");
    assert!(system.close_on_runtime_shutdown());
    assert_ne!(tenant.inner().GetKeyspace(), system.inner().GetKeyspace());
    tenant.close().unwrap();
    assert!(tenant.inner().is_closed());
    assert!(!system.inner().is_closed());
    system.close().unwrap();
}
