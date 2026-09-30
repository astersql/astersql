// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_domain_crossks::{AlterTableModeTarget, Cancellation, DdlClient, TableMode};
use astersql_domain_serverinfo::{EtcdClient, MemoryEtcdClient};

use super::{
    CanonicalSessionFactory, CreateAnalyzeSession,
    crossks_job_submit::CrossKSJobSubmitter,
    crossks_owner::CrossKSDdlOwner,
    crossks_runtime::{CrossKSProductionDdlBackend, CrossKSProductionRuntimeFactory},
    crossks_schema::{CrossKSSchemaSyncer, CrossKSStateSyncer},
    crossks_session_pool::{
        CrossKSFlashbackGuard, CrossKSMinJobId, CrossKSSessionPool, CrossKSSystemTablePool,
    },
};

#[test]
#[ignore = "requires an explicitly supplied PD/TiKV keyspace and embedded etcd"]
fn go_merge_43_real_tikv_crossks_runtime_submits_and_consumes_table_mode() {
    let pd = std::env::var("ASTERSQL_GO_MERGE_43_PD").expect("PD endpoint");
    let keyspace = std::env::var("ASTERSQL_GO_MERGE_43_KEYSPACE").expect("test keyspace");
    let endpoints = vec![pd.clone()];
    let target_store = super::crossks_store::open_target_store(&endpoints, &keyspace).unwrap();
    let target = CanonicalSessionFactory::from_tikv_store(target_store.inner().clone()).unwrap();
    let session = target.create_session();
    session
        .execute("CREATE DATABASE IF NOT EXISTS go_merge_43_real")
        .unwrap();
    session
        .execute("DROP TABLE IF EXISTS go_merge_43_real.mode_target")
        .unwrap();
    session
        .execute("CREATE TABLE go_merge_43_real.mode_target (id INT PRIMARY KEY)")
        .unwrap();
    let schema = target
        .domain()
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "go_merge_43_real")
        .unwrap();
    let table = target
        .domain()
        .table_by_name("go_merge_43_real", "mode_target")
        .unwrap();
    let (serving, _) = CreateAnalyzeSession().unwrap();
    Arc::new(CrossKSProductionRuntimeFactory::new(
        endpoints.clone(),
        endpoints,
        None,
    ))
    .install_on_domain(&serving, "SYSTEM".into());
    let manager = serving.cross_ks_manager().unwrap();
    let handle = manager.acquire(&keyspace, "real-tikv-test").unwrap();
    assert!(manager.get(&keyspace).unwrap().server_info_id().is_some());
    let cancellation = astersql_domain_crossks::Cancellation::default();
    handle
        .alter_table_mode(
            &cancellation,
            AlterTableModeTarget {
                schema_id: schema.id,
                schema_name: "go_merge_43_real".into(),
                table_id: table.ID,
                table_name: "mode_target".into(),
                current_mode: TableMode::Normal,
                target_mode: TableMode::Import,
            },
        )
        .unwrap();
    target.domain().reload().unwrap();
    assert_eq!(
        target
            .domain()
            .table_by_name("go_merge_43_real", "mode_target")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
    handle.release();
    manager.close();
    target.domain().close();
    serving.close();
    let _ = astersql_domain_crossks::Store::close(target_store.as_ref());
}

#[test]
fn go_merge_43_crossks_production_backend_submits_notifies_and_waits_for_history() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("CREATE TABLE test.crossks_runtime_test (id INT PRIMARY KEY)")
        .unwrap();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "test")
        .unwrap();
    let table = domain
        .table_by_name("test", "crossks_runtime_test")
        .unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn astersql_ddl_systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = astersql_ddl_systable::new_manager(table_pool);
    let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&manager)));
    let refresher = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(manager));
    let min_id = Arc::new(CrossKSMinJobId::new(refresher));
    let etcd = Arc::new(MemoryEtcdClient::default());
    let schema_syncer =
        CrossKSSchemaSyncer::new(Arc::clone(&domain), etcd.clone(), "virtual-runtime".into());
    schema_syncer.start().unwrap();
    let state = CrossKSStateSyncer::new(etcd.clone());
    let state_for_submit: Arc<dyn astersql_ddl_jobsubmit::ServerState> = state.clone();
    let submitter =
        CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, Some(state_for_submit));
    let owner = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd.clone(),
        "ddl-runtime".into(),
    );
    owner.install_schema_syncer(Arc::clone(&schema_syncer));
    owner.start().unwrap();
    let backend = Arc::new(CrossKSProductionDdlBackend::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        submitter,
        Arc::clone(&owner),
        state,
        etcd.clone(),
    ));
    let client = DdlClient::new(backend);
    let cancellation = Arc::new(Cancellation::default());
    let deadline = Arc::clone(&cancellation);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(15));
        deadline.cancel();
    });
    client
        .alter_table_mode(
            &cancellation,
            AlterTableModeTarget {
                schema_id: schema.id,
                schema_name: "test".into(),
                table_id: table.ID,
                table_name: "crossks_runtime_test".into(),
                current_mode: TableMode::Normal,
                target_mode: TableMode::Import,
            },
        )
        .unwrap();
    assert_eq!(
        domain
            .table_by_name("test", "crossks_runtime_test")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
    assert!(
        etcd.Snapshot()
            .contains_key("/tidb/ddl/add_ddl_job_general")
    );
    assert!(
        etcd.Snapshot()
            .contains_key("/tidb/ddl/global_schema_version")
    );
    owner.close();
    schema_syncer.close();
    astersql_domain_crossks::SessionPool::close(pool.as_ref());
    domain.close();
}

#[test]
fn go_merge_43_production_runtime_factory_cleans_virtual_registration_on_store_error() {
    let (serving_domain, _) = CreateAnalyzeSession().unwrap();
    let etcd = Arc::new(MemoryEtcdClient::default());
    let client = Arc::clone(&etcd);
    let factory = Arc::new(
        CrossKSProductionRuntimeFactory::new(
            vec!["pd:2379".into()],
            vec!["etcd:2379".into()],
            None,
        )
        .with_clients(
            Arc::new(move |_, _| {
                Err(astersql_domain_crossks::ManagerError(
                    "target Store failed".into(),
                ))
            }),
            Arc::new(move |_| Ok(client.clone() as Arc<dyn EtcdClient>)),
        ),
    );
    factory.install_on_domain(&serving_domain, "SYSTEM".into());
    let manager = serving_domain.cross_ks_manager().unwrap();
    assert!(manager.get_or_create("tenant-a").is_err());
    assert!(etcd.Snapshot().is_empty());
    serving_domain.close();
    assert!(etcd.Snapshot().is_empty());
}
