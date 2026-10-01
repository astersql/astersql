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

struct SubmitFixture {
    domain: Arc<astersql_domain::Domain>,
    pool: Arc<super::system_session::SystemSessionPool>,
    backend: Arc<dyn astersql_domain_crossks::DdlBackend>,
    notification: Arc<MemoryEtcdClient>,
    state_client: Arc<astersql_ddl_schemaver::MemoryEtcdClient>,
    manager: Arc<dyn astersql_ddl_systable::Manager>,
    min_id: Arc<astersql_ddl_systable::MinJobIdRefresher>,
}
fn submit_fixture(cdc_source: u64) -> SubmitFixture {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("CREATE TABLE test.submit_only_target (id INT PRIMARY KEY)")
        .unwrap();
    domain.set_global_system_variable("tidb_cdc_write_source", &cdc_source.to_string());
    domain.set_global_system_variable("sql_mode", "STRICT_TRANS_TABLES");
    let pool = super::system_session::SystemSessionPool::new(Arc::clone(&domain));
    let manager = astersql_ddl_systable::new_manager(pool.clone());
    let min_id = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
        manager.clone(),
    ));
    min_id.refresh(&astersql_ddl_systable::Context::default());
    let state_client = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    let state: Arc<dyn astersql_ddl_serverstate::Syncer> =
        Arc::new(astersql_ddl_serverstate::EtcdSyncer::with_client(
            state_client.clone(),
            "/tidb/server/global_state",
        ));
    let options = pool.table_mode_submit_options(
        manager.clone(),
        min_id.clone(),
        Some(Arc::new(super::system_session::JobSubmitServerState(
            state.clone(),
        ))),
    );
    let storage = domain.storage_handle();
    let variables_pool = pool.clone();
    let notification = Arc::new(MemoryEtcdClient::default());
    let backend = astersql_domain_crossks::SubmitOnlyBackend::new(
        options,
        Arc::new(move || {
            storage
                .with_storage(|store| {
                    let ts = store.CurrentVersion("global")?;
                    Ok(store.GetSnapshot(ts))
                })
                .map_err(|e: super::kv::Error| astersql_domain_crossks::Error(e.to_string()))
        }),
        Arc::new(move || {
            variables_pool
                .acquire()
                .map_err(astersql_domain_crossks::Error)?
                .ddl_session_variables()
                .map_err(astersql_domain_crossks::Error)
        }),
        Arc::new(move || {
            state
                .get_global_state(&astersql_ddl_serverstate::SyncContext::new())
                .map(|_| ())
                .map_err(|e| astersql_domain_crossks::Error(e.to_string()))
        }),
        Some(Arc::new(astersql_domain_crossks::EtcdOwnerNotifier(
            notification.clone(),
        ))),
    );
    SubmitFixture {
        domain,
        pool,
        backend: Arc::new(backend),
        notification,
        state_client,
        manager,
        min_id,
    }
}
fn submit_only_fixture() -> (
    Arc<astersql_domain::Domain>,
    Arc<dyn astersql_domain_crossks::DdlBackend>,
) {
    let f = submit_fixture(0);
    (f.domain, f.backend)
}
fn target_request(f: &SubmitFixture) -> AlterTableModeTarget {
    let schema = f
        .domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "test")
        .unwrap();
    let table = f
        .domain
        .table_by_name("test", "submit_only_target")
        .unwrap();
    AlterTableModeTarget {
        schema_id: schema.id,
        schema_name: "TEST".into(),
        table_id: table.ID,
        table_name: "SUBMIT_ONLY_TARGET".into(),
        current_mode: TableMode::Restore,
        target_mode: TableMode::Import,
    }
}
fn seed_string(f: &SubmitFixture, key: &[u8], value: &[u8]) {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        super::kv::Key(EncodeUint(EncodeBytes(vec![b'm'], key), u64::from(b's'))),
        value.to_vec(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
}

fn go_meta_hash_key(hash: &[u8], field: &[u8]) -> super::kv::Key {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    super::kv::Key(EncodeBytes(
        EncodeUint(EncodeBytes(vec![b'm'], hash), u64::from(b'h')),
        field,
    ))
}

#[test]
fn crossks_align_submit_only_reads_go_meta_history_without_owner() {
    use astersql_domain_crossks::HistoryJobState;
    use astersql_meta_model::group_3::{Job, JobState};
    let (domain, backend) = submit_only_fixture();
    let mut job = Job {
        id: 9876,
        state: JobState::Synced,
        ..Default::default()
    };
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(b"DDLJobHistory", &job.id.to_be_bytes()),
        job.encode(false).unwrap(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    assert!(
        matches!(
            backend.history_job(job.id).unwrap(),
            Some(HistoryJobState::Synced)
        ),
        "Go history must be read from the actual target KV, without a dedicated owner"
    );
    domain.close();
}

#[test]
fn crossks_align_submit_only_resolves_actual_meta_instead_of_cached_user_schema() {
    let (domain, backend) = submit_only_fixture();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "test")
        .unwrap();
    let mut table = (*domain.table_by_name("test", "submit_only_target").unwrap()).clone();
    table.Mode = astersql_meta_model::TableMode::TableModeImport;
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(
            format!("DB:{}", schema.id).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    assert_eq!(
        backend
            .resolve_table(schema.id, table.ID)
            .unwrap()
            .unwrap()
            .1,
        TableMode::Import,
        "resolution must use Go meta even when the local user cache is stale"
    );
    domain.close();
}

#[test]
fn crossks_align_submit_only_persists_and_notifies_without_changing_schema() {
    use astersql_meta_model::group_3::{ACTION_ALTER_TABLE_MODE, Job, JobState, JobVersion};
    let f = submit_fixture(7);
    seed_string(&f, b"BDRRole", b"primary");
    let schema_version = f.domain.info_schema().SchemaMetaVersion();
    let request = target_request(&f);
    let table_id = request.table_id;
    let backend = f.backend.clone();
    let token = Arc::new(Cancellation::default());
    let worker_token = token.clone();
    let worker = std::thread::spawn(move || {
        DdlClient::new(backend).alter_table_mode(&worker_token, request)
    });
    let lease = f.pool.acquire().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let rows = loop {
        let rows = lease
            .query("SELECT job_id, job_meta, type, processing FROM mysql.tidb_ddl_job")
            .unwrap();
        if !rows.is_empty() || std::time::Instant::now() >= deadline {
            break rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    token.cancel();
    assert!(worker.join().unwrap().unwrap_err().0.contains("cancel"));
    assert_eq!(rows.len(), 1);
    let id: i64 = rows[0][0].parse().unwrap();
    let job = Job::decode(rows[0][1].as_bytes()).unwrap();
    assert_eq!(job.id, id);
    let global_id = f.domain.storage_handle().with_storage(|store| {
        use astersql_util_codec::{EncodeBytes, EncodeUint};
        store
            .GetSnapshot(store.CurrentVersion("global").unwrap())
            .Get(
                &super::kv::Context::default(),
                super::kv::Key(EncodeUint(
                    EncodeBytes(vec![b'm'], b"NextGlobalID"),
                    u64::from(b's'),
                )),
                &[],
            )
            .unwrap()
            .Value
    });
    assert_eq!(String::from_utf8(global_id).unwrap(), id.to_string());
    assert_eq!(
        f.backend
            .resolve_table(job.schema_id, table_id)
            .unwrap()
            .unwrap()
            .1,
        TableMode::Normal,
        "the persisted target table mode must remain Normal without an owner"
    );
    assert_eq!(job.tp, ACTION_ALTER_TABLE_MODE);
    assert_eq!(job.state, JobState::Queueing);
    assert_eq!(job.version, JobVersion::V2);
    assert_eq!(job.table_id, table_id);
    assert_eq!(job.query, "skip");
    assert_eq!(job.schema_name, "test");
    assert_eq!(job.table_name, "submit_only_target");
    assert_eq!(job.bdr_role, "primary");
    assert_eq!(job.cdc_write_source, 7);
    assert!(
        job.start_ts > 0
            && job.sql_mode > 0
            && job.trace_info.is_some()
            && job.binlog_info.is_some()
    );
    let args: serde_json::Value = serde_json::from_slice(&job.raw_args).unwrap();
    assert_eq!(
        args,
        serde_json::json!({"schema_id": job.schema_id, "table_id": table_id, "table_mode": 1})
    );
    assert_eq!(&rows[0][2..], &["75", "0"]);
    f.min_id.refresh(&astersql_ddl_systable::Context::default());
    assert_eq!(f.min_id.current_min_job_id(), id);
    assert_eq!(
        f.manager
            .get_job_by_id(&Default::default(), id)
            .unwrap()
            .job
            .id,
        id
    );
    assert_eq!(
        f.domain
            .table_by_name("test", "submit_only_target")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
    assert_eq!(f.domain.info_schema().SchemaMetaVersion(), schema_version);
    assert!(f.backend.history_job(id).unwrap().is_none());
    let values = f.notification.Snapshot();
    assert_eq!(
        values.len(),
        1,
        "the only etcd side effect is the advisory notification; no election"
    );
    assert_eq!(values["/tidb/ddl/add_ddl_job_general"].value, b"0");
    drop(lease);
    f.pool.close();
    f.domain.close();
}

#[test]
fn crossks_align_submit_only_refreshes_upgrade_state_and_rejects_bdr_and_flashback() {
    use astersql_ddl_schemaver::EtcdClient as _;
    use astersql_meta_model::group_3::{AdminCommandOperator, Job, JobState};
    let f = submit_fixture(0);
    seed_string(&f, b"BDRRole", b"secondary");
    let client = DdlClient::new(f.backend.clone());
    let request = target_request(&f);
    let target = client
        .resolve_alter_table_mode_target(request.clone())
        .unwrap();
    let mut job = client.build_alter_table_mode_job(&target).unwrap().unwrap();
    assert!(f.backend.submit(&mut job).unwrap_err().0.contains("BDR"));
    assert_eq!(job.id, 0);
    let lease = f.pool.acquire().unwrap();
    assert!(
        lease
            .query("SELECT job_id FROM mysql.tidb_ddl_job")
            .unwrap()
            .is_empty()
    );
    seed_string(&f, b"BDRRole", b"none");
    f.state_client
        .Put(
            &astersql_ddl_schemaver::Context::Background(),
            "/tidb/server/global_state",
            r#"{"state":"upgrading"}"#,
            None,
        )
        .unwrap();
    f.backend.refresh_server_state().unwrap();
    f.backend.submit(&mut job).unwrap();
    let rows = lease
        .query(format!(
            "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id = {}",
            job.id
        ))
        .unwrap();
    let persistent = Job::decode(rows[0][0].as_bytes()).unwrap();
    assert_eq!(persistent.state, JobState::Pausing);
    assert_eq!(persistent.admin_operator, AdminCommandOperator::System);
    lease
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET type = {} WHERE job_id = {}",
            astersql_meta_model::group_3::ACTION_FLASHBACK_CLUSTER,
            job.id
        ))
        .unwrap();
    let mut blocked = client.build_alter_table_mode_job(&target).unwrap().unwrap();
    assert!(
        f.backend
            .submit(&mut blocked)
            .unwrap_err()
            .0
            .contains("flashback")
    );
    assert_eq!(blocked.id, 0);
    drop(lease);
    f.pool.close();
    f.domain.close();
}

#[test]
fn crossks_align_submit_only_waits_for_go_history_errors_and_reports_unexpected_states() {
    use astersql_domain_crossks::HistoryJobState;
    use astersql_meta_model::group_3::{Job, JobState};
    let (domain, backend) = submit_only_fixture();
    assert!(backend.history_job(123).unwrap().is_none());
    for (state, error) in [
        (JobState::Synced, Some("ignored synced error")),
        (JobState::Running, Some("worker failed")),
        (JobState::Cancelled, None),
    ] {
        let mut job = Job {
            id: 123,
            state,
            error: error.map(str::to_owned),
            ..Default::default()
        };
        let mut txn = domain
            .storage_handle()
            .with_storage(|store| store.Begin(&[]))
            .unwrap();
        txn.Set(
            go_meta_hash_key(b"DDLJobHistory", &123_i64.to_be_bytes()),
            job.encode(false).unwrap(),
        )
        .unwrap();
        txn.Commit(&super::kv::Context::default()).unwrap();
        let history = backend.history_job(123).unwrap().unwrap();
        let client = DdlClient::new(backend.clone());
        let result = client.wait_ddl_finished(&Cancellation::default(), 123);
        match state {
            JobState::Synced => {
                assert!(matches!(history, HistoryJobState::Synced));
                assert!(result.is_ok());
            }
            JobState::Running => {
                assert!(matches!(history, HistoryJobState::Failed(ref e) if e == "worker failed"));
                assert_eq!(result.unwrap_err().0, "worker failed");
            }
            _ => {
                assert!(matches!(history, HistoryJobState::Unexpected(_)));
                assert!(result.unwrap_err().0.contains("unexpected"));
            }
        }
    }
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(b"DDLJobHistory", &123_i64.to_be_bytes()),
        b"broken job JSON".to_vec(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    assert!(backend.history_job(123).is_err());
    let token = Arc::new(Cancellation::default());
    let waiter_token = token.clone();
    let waiter =
        std::thread::spawn(move || DdlClient::new(backend).wait_ddl_finished(&waiter_token, 123));
    std::thread::sleep(std::time::Duration::from_millis(220));
    token.cancel();
    assert!(
        waiter.join().unwrap().unwrap_err().0.contains("cancel"),
        "history decode errors must retry until cancellation"
    );
    domain.close();
}

#[test]
fn crossks_align_submit_only_preserves_retryable_kv_conflict_for_jobsubmit() {
    use astersql_ddl_jobsubmit::Session as _;
    let f = submit_fixture(0);
    let mut first = f.pool.acquire().unwrap();
    let mut second = f.pool.acquire().unwrap();
    // Real optimistic MVCC conflicts exercise the error conversion boundary.
    // This local KV's pessimistic LockKeys is a no-op, so racing pessimistic
    // transactions would not be evidence of production lock/retry semantics.
    first.query("BEGIN OPTIMISTIC").unwrap();
    second.query("BEGIN OPTIMISTIC").unwrap();
    first.generate_global_ids(1).unwrap();
    second.generate_global_ids(1).unwrap();
    second.commit().unwrap();
    let error = first.commit().unwrap_err();
    assert_eq!(
        error.kind,
        astersql_ddl_jobsubmit::ErrorKind::Retryable,
        "jobsubmit must receive the retryable KV error rather than permanent Storage"
    );
    first.rollback();
    drop(first);
    drop(second);
    f.pool.close();
    f.domain.close();
}

#[test]
fn crossks_align_submit_only_noop_and_name_mismatch_do_not_enqueue() {
    let f = submit_fixture(0);
    let client = DdlClient::new(f.backend.clone());
    let mut request = target_request(&f);
    request.target_mode = TableMode::Normal;
    client
        .alter_table_mode(&Cancellation::default(), request.clone())
        .unwrap();
    request.schema_name = "wrong_schema".into();
    assert!(
        client
            .alter_table_mode(&Cancellation::default(), request.clone())
            .unwrap_err()
            .0
            .contains("expected schema name")
    );
    request.schema_name = "test".into();
    request.table_name = "wrong_table".into();
    assert!(
        client
            .alter_table_mode(&Cancellation::default(), request)
            .unwrap_err()
            .0
            .contains("expected table name")
    );
    let lease = f.pool.acquire().unwrap();
    assert!(
        lease
            .query("SELECT job_id FROM mysql.tidb_ddl_job")
            .unwrap()
            .is_empty()
    );
    assert!(f.notification.Snapshot().is_empty());
    drop(lease);
    f.pool.close();
    f.domain.close();
}

#[test]
fn crossks_align_submit_only_meta_reader_uses_one_immutable_snapshot() {
    use astersql_meta_model::group_3::{Job, JobState};
    let f = submit_fixture(0);
    let request = target_request(&f);
    let old_snapshot = f
        .domain
        .storage_handle()
        .with_storage(|store| store.GetSnapshot(store.CurrentVersion("global").unwrap()));
    let old = astersql_meta::SnapshotReader::new(old_snapshot);
    assert_eq!(
        old.get_database(request.schema_id).unwrap().unwrap().Name.L,
        "test"
    );
    let mut table = old
        .get_table(request.schema_id, request.table_id)
        .unwrap()
        .unwrap();
    assert_eq!(table.Mode, astersql_meta_model::TableMode::TableModeNormal);
    table.Mode = astersql_meta_model::TableMode::TableModeImport;
    let mut job = Job {
        id: 444,
        state: JobState::Synced,
        ..Default::default()
    };
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(
            format!("DB:{}", request.schema_id).as_bytes(),
            format!("Table:{}", request.table_id).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Set(
        go_meta_hash_key(b"DDLJobHistory", &444_i64.to_be_bytes()),
        job.encode(false).unwrap(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    assert!(old.get_history_ddl_job(444).unwrap().is_none());
    assert_eq!(
        old.get_table(request.schema_id, request.table_id)
            .unwrap()
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
    assert_eq!(
        f.backend
            .resolve_table(request.schema_id, request.table_id)
            .unwrap()
            .unwrap()
            .1,
        TableMode::Import
    );
    assert!(matches!(
        f.backend.history_job(444).unwrap(),
        Some(astersql_domain_crossks::HistoryJobState::Synced)
    ));
    f.pool.close();
    f.domain.close();
}

#[test]
fn crossks_align_submit_only_reads_go_structured_history_error() {
    use astersql_meta_model::group_3::{Job, JobState};
    let (domain, backend) = submit_only_fixture();
    let mut job = Job {
        id: 789,
        state: JobState::Cancelled,
        ..Default::default()
    };
    let mut value: serde_json::Value = serde_json::from_slice(&job.encode(false).unwrap()).unwrap();
    // github.com/pingcap/errors.MarshalJSON, as pinned in this repository's go.mod.
    value["err"] = serde_json::json!({"class": 2, "code": 8214, "message": "Cancelled DDL job", "rfccode": "ddl:8214"});
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(b"DDLJobHistory", &789_i64.to_be_bytes()),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    assert!(
        matches!(backend.history_job(789).unwrap(), Some(astersql_domain_crossks::HistoryJobState::Failed(ref message)) if message == "[ddl:8214]Cancelled DDL job")
    );
    let result = DdlClient::new(backend).wait_ddl_finished(&Cancellation::default(), 789);
    assert_eq!(result.unwrap_err().0, "[ddl:8214]Cancelled DDL job");
    // Older Go histories may carry only the class/code fallback.
    value["err"].as_object_mut().unwrap().remove("rfccode");
    value["warning"] =
        serde_json::json!({"class": 2, "code": 100, "message": "warning", "rfccode": "ddl:100"});
    let mut txn = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Set(
        go_meta_hash_key(b"DDLJobHistory", &789_i64.to_be_bytes()),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    txn.Commit(&super::kv::Context::default()).unwrap();
    let snapshot = domain
        .storage_handle()
        .with_storage(|store| store.GetSnapshot(store.CurrentVersion("global").unwrap()));
    let reader = astersql_meta::SnapshotReader::new(snapshot);
    let read = reader.get_history_ddl_job(789).unwrap().unwrap();
    assert_eq!(read.error.as_deref(), Some("[ddl:8214]Cancelled DDL job"));
    assert_eq!(read.warning.as_deref(), Some("[ddl:100]warning"));

    domain.close();
}
