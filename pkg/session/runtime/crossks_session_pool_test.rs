// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_ddl_jobsubmit::{Session as JobSession, SessionPool as JobSessionPool};
use astersql_ddl_systable as systable;
use astersql_domain_crossks::SessionPool;

use super::{
    CreateAnalyzeSession,
    crossks_session_pool::{
        CrossKSFlashbackGuard, CrossKSJobSessionPool, CrossKSMinJobId, CrossKSSessionPool,
        CrossKSSystemTablePool,
    },
};

#[test]
fn go_merge_43_crossks_system_pool_preserves_session_and_closes_workers() {
    let (domain, _) = CreateAnalyzeSession().expect("canonical SQL session");
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let lease = pool.acquire().expect("borrow session");
    lease
        .query("CREATE TABLE crossks_pool_test (id INT PRIMARY KEY)")
        .unwrap();
    lease
        .query("INSERT INTO crossks_pool_test VALUES (42)")
        .unwrap();
    assert_eq!(
        lease.query("SELECT id FROM crossks_pool_test").unwrap(),
        vec![vec!["42".to_owned()]],
    );
    drop(lease);
    let second = pool.acquire().expect("borrow returned session");
    assert_eq!(
        second.query("SELECT id FROM crossks_pool_test").unwrap(),
        vec![vec!["42".to_owned()]],
    );
    pool.close();
    assert!(second.query("SELECT id FROM crossks_pool_test").is_err());
    assert!(pool.acquire().is_err());
    domain.close();
}

#[test]
fn go_merge_43_crossks_jobsubmit_session_locks_and_allocates_global_ids() {
    let (domain, _) = CreateAnalyzeSession().expect("canonical SQL session");
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let jobs = CrossKSJobSessionPool::new(Arc::clone(&pool));
    let mut session = jobs.get().expect("borrow job session");
    session.begin().expect("begin pessimistic transaction");
    let (role, start_ts) = session.read_bdr_role_and_start_ts().unwrap();
    assert_eq!(role, "none");
    assert!(start_ts > 0);
    let current_version = session.current_version().unwrap();
    session.lock_global_id_key(current_version).unwrap();
    session.set_snapshot_ts(current_version);
    let ids = session.generate_global_ids(2).unwrap();
    assert_eq!(ids[1], ids[0] + 1);
    session.commit().unwrap();
    jobs.put(session);
    pool.close();
    domain.close();
}

#[test]
fn go_merge_43_crossks_system_table_manager_reads_target_job_table() {
    let (domain, _) = CreateAnalyzeSession().expect("canonical SQL session");
    let pool = CrossKSSessionPool::new(Arc::clone(&domain));
    let table_pool: Arc<dyn systable::SessionPool> =
        Arc::new(CrossKSSystemTablePool::new(Arc::clone(&pool)));
    let manager = systable::new_manager(table_pool);
    let guard = CrossKSFlashbackGuard::new(Arc::clone(&manager));
    assert!(
        !astersql_ddl_jobsubmit::SystemTableManager::has_flashback_cluster_job(&guard, 0)
            .expect("check empty DDL job table")
    );
    let refresher = Arc::new(systable::new_min_job_id_refresher(manager));
    refresher.refresh(&systable::Context::default());
    let provider = CrossKSMinJobId::new(refresher);
    assert_eq!(
        astersql_ddl_jobsubmit::MinJobIdProvider::current_min_job_id(&provider),
        0
    );
    pool.close();
    domain.close();
}
