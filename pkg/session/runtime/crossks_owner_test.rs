// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_domain_crossks::{AlterTableModeJob, SessionPool, TableMode};
use astersql_domain_serverinfo::MemoryEtcdClient;
use astersql_meta_model::group_3::{Job as ModelJob, JobState};

use super::{
    CreateAnalyzeSession,
    crossks_job_submit::CrossKSJobSubmitter,
    crossks_owner::CrossKSDdlOwner,
    crossks_session_pool::{
        CrossKSFlashbackGuard, CrossKSMinJobId, CrossKSSessionPool, CrossKSSystemTablePool,
    },
};

fn wait_for_history(
    owner: &CrossKSDdlOwner,
    job_id: i64,
) -> astersql_domain_crossks::HistoryJobState {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(state) = owner.history_job(job_id).unwrap() {
            return state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DDL job did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn go_merge_43_crossks_owner_completes_persisted_table_mode_job() {
    let (domain, session) = CreateAnalyzeSession().expect("canonical SQL session");
    session
        .execute("CREATE TABLE test.crossks_owner_test (id INT PRIMARY KEY)")
        .unwrap();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "test")
        .unwrap();
    let table = domain.table_by_name("test", "crossks_owner_test").unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn astersql_ddl_systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = astersql_ddl_systable::new_manager(table_pool);
    let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&manager)));
    let refresher = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(manager));
    let min_id = Arc::new(CrossKSMinJobId::new(refresher));
    let submitter = CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, None);
    let mut job = AlterTableModeJob {
        id: 0,
        schema_id: schema.id,
        table_id: table.ID,
        schema_name: "test".into(),
        table_name: "crossks_owner_test".into(),
        target_mode: TableMode::Import,
        query: "skip".into(),
        cdc_write_source: 0,
        sql_mode: 0,
    };
    submitter.submit_table_mode(&mut job).unwrap();
    let etcd = Arc::new(MemoryEtcdClient::default());
    let owner = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd,
        "owner-a".into(),
    );
    owner.start().unwrap();
    assert!(matches!(
        wait_for_history(&owner, job.id),
        astersql_domain_crossks::HistoryJobState::Synced
    ));
    assert_eq!(
        domain
            .table_by_name("test", "crossks_owner_test")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
    let lease = pool.acquire().unwrap();
    assert!(
        lease
            .query(format!(
                "SELECT job_id FROM mysql.tidb_ddl_job WHERE job_id = {}",
                job.id
            ))
            .unwrap()
            .is_empty()
    );
    drop(lease);
    owner.close();
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_owner_uses_shared_election_and_hands_off() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let etcd = Arc::new(MemoryEtcdClient::default());
    let key = "/go-merge-43/ddl-owner-handoff";
    let first_manager =
        astersql_owner::NewMockManager(astersql_owner::Context::new(), "owner-first", None, key);
    let second_manager =
        astersql_owner::NewMockManager(astersql_owner::Context::new(), "owner-second", None, key);
    let first = CrossKSDdlOwner::new_with_election(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd.clone(),
        "owner-first".into(),
        first_manager,
    );
    let second = CrossKSDdlOwner::new_with_election(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd,
        "owner-second".into(),
        second_manager,
    );
    first.start().unwrap();
    second.start().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !first.acquire_ownership().unwrap() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(first.acquire_ownership().unwrap());
    assert!(!second.acquire_ownership().unwrap());
    first.close();
    while !second.acquire_ownership().unwrap() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(second.acquire_ownership().unwrap());
    second.close();
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_owner_fake_election_releases_on_close() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let etcd = Arc::new(MemoryEtcdClient::default());
    let first = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd.clone(),
        "fake-first".into(),
    );
    let second = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd,
        "fake-second".into(),
    );
    first.start().unwrap();
    second.start().unwrap();
    assert!(first.acquire_ownership().unwrap());
    assert!(!second.acquire_ownership().unwrap());
    first.close();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !second.acquire_ownership().unwrap() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(second.acquire_ownership().unwrap());
    second.close();
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_owner_records_failed_job_in_history() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn astersql_ddl_systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = astersql_ddl_systable::new_manager(table_pool);
    let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&manager)));
    let min_id = Arc::new(CrossKSMinJobId::new(Arc::new(
        astersql_ddl_systable::new_min_job_id_refresher(manager),
    )));
    let submitter = CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, None);
    let mut job = AlterTableModeJob {
        id: 0,
        schema_id: 999999,
        table_id: 999998,
        schema_name: "missing".into(),
        table_name: "missing".into(),
        target_mode: TableMode::Import,
        query: "skip".into(),
        cdc_write_source: 0,
        sql_mode: 0,
    };
    submitter.submit_table_mode(&mut job).unwrap();
    let owner = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        Arc::new(MemoryEtcdClient::default()),
        "failed-owner".into(),
    );
    owner.start().unwrap();
    assert!(matches!(wait_for_history(&owner, job.id),
        astersql_domain_crossks::HistoryJobState::Failed(message) if message.contains("schema")));
    owner.close();
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_owner_go_semantics_cancelling_job_does_not_change_table() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("CREATE TABLE test.crossks_cancelling_test (id INT PRIMARY KEY)")
        .unwrap();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "test")
        .unwrap();
    let table = domain
        .table_by_name("test", "crossks_cancelling_test")
        .unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn astersql_ddl_systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = astersql_ddl_systable::new_manager(table_pool);
    let guard = Arc::new(CrossKSFlashbackGuard::new(Arc::clone(&manager)));
    let min_id = Arc::new(CrossKSMinJobId::new(Arc::new(
        astersql_ddl_systable::new_min_job_id_refresher(manager),
    )));
    let submitter = CrossKSJobSubmitter::new(Arc::clone(&pool), guard, min_id, None);
    let mut job = AlterTableModeJob {
        id: 0,
        schema_id: schema.id,
        table_id: table.ID,
        schema_name: "test".into(),
        table_name: "crossks_cancelling_test".into(),
        target_mode: TableMode::Import,
        query: "skip".into(),
        cdc_write_source: 0,
        sql_mode: 0,
    };
    submitter.submit_table_mode(&mut job).unwrap();
    let lease = pool.acquire().unwrap();
    let rows = lease
        .query(format!(
            "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id = {}",
            job.id
        ))
        .unwrap();
    let mut stored = ModelJob::decode(rows[0][0].as_bytes()).unwrap();
    stored.state = JobState::Cancelling;
    let encoded = stored.encode(false).unwrap();
    let encoded_hex = encoded
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    lease
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta = x'{}' WHERE job_id = {}",
            encoded_hex, job.id
        ))
        .unwrap();
    drop(lease);
    let owner = CrossKSDdlOwner::new(
        Arc::clone(&domain),
        Arc::clone(&pool),
        Arc::new(MemoryEtcdClient::default()),
        "cancelling-owner".into(),
    );
    owner.start().unwrap();
    assert!(matches!(
        wait_for_history(&owner, job.id),
        astersql_domain_crossks::HistoryJobState::Failed(_)
    ));
    assert_eq!(
        domain
            .table_by_name("test", "crossks_cancelling_test")
            .unwrap()
            .Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
    owner.close();
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_owner_relinquishes_election_for_other_ddl_types() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let etcd = Arc::new(MemoryEtcdClient::default());
    let key = "/go-merge-43/ddl-owner-foreign-job";
    let manager = astersql_owner::NewMockManager(
        astersql_owner::Context::new(),
        "crossks-foreign",
        None,
        key,
    );
    let owner = CrossKSDdlOwner::new_with_election(
        Arc::clone(&domain),
        Arc::clone(&pool),
        etcd,
        "crossks-foreign".into(),
        Arc::clone(&manager),
    );
    owner.start().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !manager.IsOwner() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(manager.IsOwner());
    session
        .execute("INSERT INTO mysql.tidb_ddl_job(job_id, type, processing) VALUES (999999, 3, 0)")
        .unwrap();
    let successor =
        astersql_owner::NewMockManager(astersql_owner::Context::new(), "general-ddl", None, key);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(successor.CampaignOwner(&[])).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !successor.IsOwner() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(successor.IsOwner());
    assert!(!manager.IsOwner());
    runtime.block_on(successor.Close());
    owner.close();
    pool.close();
    domain.close();
}
