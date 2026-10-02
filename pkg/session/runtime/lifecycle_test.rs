// Copyright 2026 AsterSQL.

use super::session_factory::{KeyspaceSessionFactory, TargetTransport};
use astersql_domain_serverinfo::{EtcdClient, MemoryEtcdClient};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn crossks_align_lifecycle_opens_store_before_virtual_registration() {
    let (domain, _) = super::CreateAnalyzeSession().unwrap();
    let etcd = Arc::new(MemoryEtcdClient::default());
    let seen = Arc::new(AtomicBool::new(false));
    let store_etcd = etcd.clone();
    let store_seen = seen.clone();
    let client = etcd.clone();
    let factory = Arc::new(KeyspaceSessionFactory::with_openers(
        Arc::new(move |_| {
            store_seen.store(!store_etcd.Snapshot().is_empty(), Ordering::Release);
            Err(astersql_domain_crossks::ManagerError(
                "Store open failed".into(),
            ))
        }),
        Arc::new(move |_| {
            Ok(TargetTransport {
                server: client.clone() as Arc<dyn EtcdClient>,
                schema: Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default()),
            })
        }),
    ));
    factory.install_on_domain(&domain, "SYSTEM".into()).unwrap();
    assert!(
        domain
            .cross_ks_manager()
            .unwrap()
            .get_or_create("tenant-lifecycle")
            .is_err()
    );
    domain.close();
    assert!(etcd.Snapshot().is_empty());
    assert!(
        !seen.load(Ordering::Acquire),
        "Go opens Store before publishing the virtual server"
    );
}

struct Fixture {
    domain: Arc<astersql_domain::Domain>,
    kv: astersql_store_mockstore_mockstorage::KVStore,
    server: Arc<MemoryEtcdClient>,
    protocol: Arc<astersql_ddl_schemaver::MemoryEtcdClient>,
}
impl Fixture {
    fn new() -> Self {
        let kv = astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO();
        let storage = Arc::try_unwrap(
            astersql_store_mockstore_mockstorage::NewMockStorage(kv.clone(), None).unwrap(),
        )
        .ok()
        .unwrap();
        let domain = Arc::new(astersql_domain::Domain::new(
            storage,
            Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
            Default::default(),
        ));
        domain.init().unwrap();
        domain
            .ddl_create_database_with_id(
                "mysql",
                true,
                Some(astersql_meta_metadef::SystemDatabaseID),
            )
            .unwrap();
        let session = super::BootstrapCanonicalDomain(domain.clone()).unwrap();
        session
            .execute("CREATE TABLE test.lifecycle_target (id INT PRIMARY KEY)")
            .unwrap();
        Self {
            domain,
            kv,
            server: Arc::new(MemoryEtcdClient::default()),
            protocol: Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default()),
        }
    }
    fn factory(&self, owned: bool) -> Arc<KeyspaceSessionFactory> {
        let storage = self.domain.storage_handle();
        let server = self.server.clone();
        let protocol = self.protocol.clone();
        Arc::new(KeyspaceSessionFactory::with_openers(
            Arc::new(move |keyspace| {
                Ok(super::session_factory::TargetSessionStore::new(
                    storage.clone(),
                    keyspace.to_owned(),
                    owned,
                ))
            }),
            Arc::new(move |_| {
                Ok(TargetTransport {
                    server: server.clone(),
                    schema: protocol.clone(),
                })
            }),
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.domain.close();
        let _ = self.kv.Close();
    }
}

#[test]
fn crossks_align_lifecycle_public_components_load_only_system_schema_and_close_once() {
    let f = Fixture::new();
    let factory = f.factory(true);
    factory
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let manager = f.domain.cross_ks_manager().unwrap();
    let runtime = manager.get_or_create("tenant-lifecycle").unwrap();
    let schema = runtime.info_cache().schema().unwrap().CompleteInfoSchema();
    assert!(
        schema
            .SchemaByName(&astersql_infoschema::CiString::new("mysql"))
            .is_some()
    );
    assert!(
        schema
            .SchemaByName(&astersql_infoschema::CiString::new("test"))
            .is_none()
    );
    assert!(runtime.server_info_id().is_some());
    assert_eq!(f.kv.CloseCount(), 0);
    runtime.close();
    runtime.close();
    manager.close();
    assert_eq!(f.kv.CloseCount(), 1);
    assert!(f.server.Snapshot().is_empty());
    use astersql_ddl_schemaver::EtcdClient;
    assert!(
        f.protocol
            .Get(
                &astersql_ddl_schemaver::Context::Background(),
                "/tidb/ddl/all_schema_versions/",
                true
            )
            .unwrap()
            .Kvs
            .is_empty()
    );
}

#[test]
fn min_job_id_refresher_can_be_skipped_for_session_count() {
    let f = Fixture::new();
    let hook_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&hook_called);
    let factory = Arc::new(
        KeyspaceSessionFactory::with_openers(
            {
                let storage = f.domain.storage_handle();
                Arc::new(move |keyspace| {
                    Ok(super::session_factory::TargetSessionStore::new(
                        storage.clone(),
                        keyspace.to_owned(),
                        false,
                    ))
                })
            },
            {
                let server = Arc::clone(&f.server);
                let protocol = Arc::clone(&f.protocol);
                Arc::new(move |_| {
                    Ok(TargetTransport {
                        server: server.clone(),
                        schema: protocol.clone(),
                    })
                })
            },
        )
        .with_min_job_id_refresher_hook(Arc::new(move |should_run| {
            called.store(true, Ordering::Release);
            assert!(
                *should_run,
                "the production default must start the refresher"
            );
            *should_run = false;
        })),
    );
    factory
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let manager = f.domain.cross_ks_manager().unwrap();
    let runtime = manager.get_or_create("tenant-session-count").unwrap();

    assert!(hook_called.load(Ordering::Acquire));
    assert_eq!(runtime.coordinator().internal_session_count(), 0);

    manager.close();
}

#[test]
fn crossks_align_lifecycle_shared_system_store_survives_idle_eviction() {
    let f = Fixture::new();
    f.factory(false)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let manager = f.domain.cross_ks_manager().unwrap();
    let handle = manager.acquire("SYSTEM", "lifecycle-holder").unwrap();
    manager.sweep_idle_runtimes(std::time::Duration::ZERO);
    assert!(manager.get("SYSTEM").is_some());
    handle.release();
    manager.set_last_release_at(
        "SYSTEM",
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    manager.sweep_idle_runtimes(std::time::Duration::ZERO);
    assert!(manager.get("SYSTEM").is_none());
    assert_eq!(f.kv.CloseCount(), 0);
    super::ConcreteSession::new(f.domain.clone())
        .execute("SELECT * FROM test.lifecycle_target")
        .unwrap();
    assert!(f.server.Snapshot().is_empty());
    manager.close();
    assert_eq!(f.kv.CloseCount(), 0);
}

#[test]
fn crossks_align_lifecycle_transport_failure_closes_owned_store_before_registration() {
    let f = Fixture::new();
    let storage = f.domain.storage_handle();
    let factory = Arc::new(KeyspaceSessionFactory::with_openers(
        Arc::new(move |keyspace| {
            Ok(super::session_factory::TargetSessionStore::new(
                storage.clone(),
                keyspace.into(),
                true,
            ))
        }),
        Arc::new(|_| {
            Err(astersql_domain_crossks::ManagerError(
                "transport unavailable".into(),
            ))
        }),
    ));
    factory
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let error = f
        .domain
        .cross_ks_manager()
        .unwrap()
        .get_or_create("tenant-lifecycle")
        .err()
        .unwrap();
    assert!(error.0.contains("transport unavailable"));
    assert_eq!(f.kv.CloseCount(), 1);
    assert!(f.server.Snapshot().is_empty());
}

#[test]
fn crossks_align_lifecycle_schema_init_failure_removes_registration_and_store() {
    let f = Fixture::new();
    f.protocol.FailPuts(1);
    f.factory(true)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    assert!(
        f.domain
            .cross_ks_manager()
            .unwrap()
            .get_or_create("tenant-lifecycle")
            .is_err()
    );
    assert_eq!(f.kv.CloseCount(), 1);
    assert!(f.server.Snapshot().is_empty());
}

#[test]
fn crossks_align_lifecycle_invalid_server_state_removes_registration_and_store() {
    let f = Fixture::new();
    use astersql_ddl_schemaver::EtcdClient;
    f.protocol
        .Put(
            &astersql_ddl_schemaver::Context::Background(),
            astersql_ddl_util::ServerGlobalState,
            "invalid",
            None,
        )
        .unwrap();
    f.factory(true)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let error = f
        .domain
        .cross_ks_manager()
        .unwrap()
        .get_or_create("tenant-lifecycle")
        .err()
        .unwrap();
    assert!(error.0.contains("state"));
    assert_eq!(f.kv.CloseCount(), 1);
    assert!(f.server.Snapshot().is_empty());
}

#[test]
fn crossks_align_lifecycle_factory_submits_durable_job_without_consuming_it() {
    let f = Fixture::new();
    f.factory(false)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let runtime = f
        .domain
        .cross_ks_manager()
        .unwrap()
        .get_or_create("tenant-lifecycle")
        .unwrap();
    let table = f.domain.table_by_name("test", "lifecycle_target").unwrap();
    let db = f
        .domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|db| db.name.lower == "test")
        .unwrap();
    let cancelled = Arc::new(astersql_domain_crossks::Cancellation::default());
    let timer = cancelled.clone();
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        timer.cancel();
    });
    let result = runtime.alter_table_mode(
        &cancelled,
        astersql_domain_crossks::AlterTableModeTarget {
            schema_id: db.id,
            schema_name: "test".into(),
            table_id: table.ID,
            table_name: "lifecycle_target".into(),
            current_mode: astersql_domain_crossks::TableMode::Normal,
            target_mode: astersql_domain_crossks::TableMode::Import,
        },
    );
    cancel.join().unwrap();
    assert!(result.is_err());
    let pool = super::system_session::SystemSessionPool::new(f.domain.clone());
    let lease = pool.acquire().unwrap();
    let rows = lease
        .query("SELECT job_meta FROM mysql.tidb_ddl_job")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let job = astersql_meta::decode_go_history_job(rows[0][0].as_bytes()).unwrap();
    assert_eq!(job.tp, 75);
    assert_eq!(job.table_id, table.ID);
    assert_eq!(job.state, astersql_meta_model::group_3::JobState::Queueing);
    assert!(
        lease
            .query("SELECT job_meta FROM mysql.tidb_ddl_history")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.domain
            .table_by_name("test", "lifecycle_target")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
    assert!(
        f.server
            .Snapshot()
            .contains_key("/tidb/ddl/add_ddl_job_general")
    );
    assert_eq!(runtime.coordinator().internal_session_count(), 0);
    drop(lease);
    pool.close();
    runtime.close();
}

struct FailingRegistration {
    inner: Arc<MemoryEtcdClient>,
    after_put: bool,
}
impl astersql_domain_serverinfo::EtcdClient for FailingRegistration {
    fn GrantLease(
        &self,
        context: &astersql_domain_serverinfo::Context,
        ttl: i32,
    ) -> Result<i64, astersql_domain_serverinfo::SyncError> {
        if !self.after_put {
            return Err(astersql_domain_serverinfo::SyncError(
                "injected lease grant".into(),
            ));
        }
        self.inner.GrantLease(context, ttl)
    }
    fn Get(
        &self,
        context: &astersql_domain_serverinfo::Context,
        key: &str,
        prefix: bool,
    ) -> Result<Vec<astersql_domain_serverinfo::KeyValue>, astersql_domain_serverinfo::SyncError>
    {
        self.inner.Get(context, key, prefix)
    }
    fn Put(
        &self,
        context: &astersql_domain_serverinfo::Context,
        key: &str,
        value: Vec<u8>,
        lease: Option<i64>,
    ) -> Result<(), astersql_domain_serverinfo::SyncError> {
        self.inner.Put(context, key, value, lease)?;
        Err(astersql_domain_serverinfo::SyncError(
            "injected registration response loss".into(),
        ))
    }
    fn Delete(
        &self,
        context: &astersql_domain_serverinfo::Context,
        key: &str,
    ) -> Result<(), astersql_domain_serverinfo::SyncError> {
        self.inner.Delete(context, key)
    }
    fn DeletePrefix(
        &self,
        context: &astersql_domain_serverinfo::Context,
        key: &str,
    ) -> Result<(), astersql_domain_serverinfo::SyncError> {
        self.inner.DeletePrefix(context, key)
    }
    fn RevokeLease(
        &self,
        context: &astersql_domain_serverinfo::Context,
        lease: i64,
    ) -> Result<(), astersql_domain_serverinfo::SyncError> {
        self.inner.RevokeLease(context, lease)
    }
}

#[test]
fn crossks_align_lifecycle_registration_lease_and_response_failure_cleanup() {
    for after_put in [false, true] {
        let f = Fixture::new();
        let storage = f.domain.storage_handle();
        let server = Arc::new(FailingRegistration {
            inner: f.server.clone(),
            after_put,
        });
        let protocol = f.protocol.clone();
        let factory = Arc::new(KeyspaceSessionFactory::with_openers(
            Arc::new(move |ks| {
                Ok(super::session_factory::TargetSessionStore::new(
                    storage.clone(),
                    ks.into(),
                    true,
                ))
            }),
            Arc::new(move |_| {
                Ok(TargetTransport {
                    server: server.clone(),
                    schema: protocol.clone(),
                })
            }),
        ));
        factory
            .install_on_domain(&f.domain, "source".into())
            .unwrap();
        assert!(
            f.domain
                .cross_ks_manager()
                .unwrap()
                .get_or_create("tenant-lifecycle")
                .is_err()
        );
        assert_eq!(f.kv.CloseCount(), 1);
        assert!(f.server.Snapshot().is_empty());
    }
}

#[test]
fn crossks_align_lifecycle_schema_reload_failure_cleans_registered_resources() {
    let f = Fixture::new();
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    let hash = EncodeBytes(
        EncodeUint(EncodeBytes(vec![b'm'], b"DBs"), b'h' as u64),
        format!("DB:{}", astersql_meta_metadef::SystemDatabaseID).as_bytes(),
    );
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Delete(super::kv::Key(hash)).unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    f.factory(true)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let error = f
        .domain
        .cross_ks_manager()
        .unwrap()
        .get_or_create("tenant-lifecycle")
        .err()
        .unwrap();
    assert!(error.0.contains("system database not found"));
    assert_eq!(f.kv.CloseCount(), 1);
    assert!(f.server.Snapshot().is_empty());
}

#[test]
fn crossks_align_lifecycle_session_commit_uses_shared_validator_and_recovers() {
    let f = Fixture::new();
    let validator: Arc<astersql_infoschema_isvalidator::Validator> = Arc::from(
        astersql_infoschema_isvalidator::new(std::time::Duration::from_secs(1)),
    );
    let version = f.domain.info_schema().SchemaMetaVersion();
    let ts = f
        .domain
        .storage_handle()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    validator.update(ts, version, version, None);
    let pool = super::system_session::SystemSessionPool::new_with_validator(
        f.domain.clone(),
        Default::default(),
        Some(validator.clone()),
    );
    let lease = pool.acquire().unwrap();
    lease.query("BEGIN").unwrap();
    lease
        .query("INSERT INTO test.lifecycle_target VALUES (1)")
        .unwrap();
    validator.stop();
    let commit = lease.query("COMMIT");
    assert!(
        commit.is_err(),
        "stopped shared validator must prevent SQL writes from committing"
    );
    assert!(
        lease
            .query("SELECT * FROM test.lifecycle_target")
            .unwrap()
            .is_empty()
    );
    validator.restart(version);
    let ts = f
        .domain
        .storage_handle()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    validator.update(ts, version, version, None);
    lease.query("BEGIN").unwrap();
    lease
        .query("INSERT INTO test.lifecycle_target VALUES (2)")
        .unwrap();
    lease.query("COMMIT").unwrap();
    assert_eq!(
        lease.query("SELECT * FROM test.lifecycle_target").unwrap(),
        vec![vec!["2".to_owned()]]
    );
    drop(lease);
    pool.close();
}

#[test]
fn crossks_align_lifecycle_automatic_gc_is_installed_and_stops_with_manager() {
    let f = Fixture::new();
    f.factory(false)
        .install_on_domain(&f.domain, "source".into())
        .unwrap();
    let manager = f.domain.cross_ks_manager().unwrap();
    let handle = manager.acquire("SYSTEM", "automatic-gc-holder").unwrap();
    handle.release();
    manager.set_last_release_at(
        "SYSTEM",
        std::time::Instant::now()
            - astersql_domain_crossks::CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT
            - std::time::Duration::from_secs(1),
    );
    let deadline = std::time::Instant::now()
        + astersql_domain_crossks::CROSS_KEYSPACE_RUNTIME_SWEEP_INTERVAL
        + std::time::Duration::from_secs(5);
    while manager.get("SYSTEM").is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let evicted = manager.get("SYSTEM").is_none();
    manager.close();
    manager.close();
    assert!(
        evicted,
        "factory installation must start the Go idle runtime sweep loop"
    );
    assert_eq!(f.kv.CloseCount(), 0);
    assert!(f.server.Snapshot().is_empty());
}

#[test]
fn crossks_align_lifecycle_shared_validator_checks_related_table_deltas() {
    for related in [false, true] {
        let f = Fixture::new();
        let validator: Arc<astersql_infoschema_isvalidator::Validator> = Arc::from(
            astersql_infoschema_isvalidator::new(std::time::Duration::from_secs(30)),
        );
        let version = f.domain.info_schema().SchemaMetaVersion();
        let table = f
            .domain
            .table_by_name("test", "lifecycle_target")
            .unwrap()
            .ID;
        let ts = f
            .domain
            .storage_handle()
            .with_storage(|s| s.CurrentVersion("global"))
            .unwrap()
            .Ver;
        validator.update(ts, version - 1, version, None);
        let pool = super::system_session::SystemSessionPool::new_with_validator(
            f.domain.clone(),
            Default::default(),
            Some(validator.clone()),
        );
        let lease = pool.acquire().unwrap();
        lease.query("BEGIN").unwrap();
        lease
            .query("INSERT INTO test.lifecycle_target VALUES (3)")
            .unwrap();
        let change = astersql_infoschema_isvalidator::RelatedSchemaChange {
            phy_tbl_ids: vec![if related { table } else { table + 10000 }],
            action_types: vec![75],
        };
        validator.update(ts, version, version + 1, Some(&change));
        let result = lease.query("COMMIT");
        if related {
            assert!(result.unwrap_err().to_string().contains("8028"));
            assert!(
                lease
                    .query("SELECT * FROM test.lifecycle_target")
                    .unwrap()
                    .is_empty()
            );
        } else {
            result.unwrap();
            assert_eq!(
                lease.query("SELECT * FROM test.lifecycle_target").unwrap(),
                vec![vec!["3".to_owned()]]
            );
        }
        drop(lease);
        pool.close();
    }
}

#[test]
fn crossks_align_lifecycle_expired_validator_rejects_writes_but_read_only_commit_succeeds() {
    let f = Fixture::new();
    let validator: Arc<astersql_infoschema_isvalidator::Validator> = Arc::from(
        astersql_infoschema_isvalidator::new(std::time::Duration::from_secs(1)),
    );
    let version = f.domain.info_schema().SchemaMetaVersion();
    let pool = super::system_session::SystemSessionPool::new_with_validator(
        f.domain.clone(),
        Default::default(),
        Some(validator.clone()),
    );
    let lease = pool.acquire().unwrap();
    validator.stop();
    lease.query("BEGIN").unwrap();
    lease.query("SELECT * FROM test.lifecycle_target").unwrap();
    lease.query("COMMIT").unwrap();
    validator.restart(version);
    let ts = f
        .domain
        .storage_handle()
        .with_storage(|s| s.CurrentVersion("global"))
        .unwrap()
        .Ver;
    validator.update(ts - (2000 << 18), version, version, None);
    lease.query("BEGIN").unwrap();
    lease
        .query("INSERT INTO test.lifecycle_target VALUES (4)")
        .unwrap();
    assert!(
        lease
            .query("COMMIT")
            .unwrap_err()
            .to_string()
            .contains("8027")
    );
    assert!(
        lease
            .query("SELECT * FROM test.lifecycle_target")
            .unwrap()
            .is_empty()
    );
    drop(lease);
    pool.close();
}
