// Copyright 2026 AsterSQL.

use astersql_ddl_jobsubmit::Session as JobSession;
use astersql_session::runtime::{CreateAnalyzeSession, system_session::SystemSessionPool};
use std::sync::Arc;

#[test]
fn crossks_align_system_session_return_rolls_back() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(Arc::clone(&domain));
    let session = pool.acquire().unwrap();
    session
        .query("CREATE TABLE system_pool_rollback (id INT PRIMARY KEY)")
        .unwrap();
    session.query("BEGIN PESSIMISTIC").unwrap();
    session
        .query("INSERT INTO system_pool_rollback VALUES (42)")
        .unwrap();
    let id = session.session_id();
    drop(session);
    let session = pool.acquire().unwrap();
    assert_eq!(
        session.session_id(),
        id,
        "the common pool reuses the same concrete session"
    );
    assert_eq!(
        session
            .query("SELECT id FROM system_pool_rollback")
            .unwrap(),
        Vec::<Vec<String>>::new(),
        "returning a lease must rollback its transaction before reuse"
    );
    drop(session);
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_system_session_transaction_sql_and_go_meta() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let mut seed = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let role_key = astersql_kv::Key(astersql_util_codec::EncodeUint(
        astersql_util_codec::EncodeBytes(vec![b'm'], b"BDRRole"),
        u64::from(b's'),
    ));
    seed.Set(role_key, b"primary".to_vec()).unwrap();
    seed.Commit(&astersql_kv::Context::default()).unwrap();
    let pool = SystemSessionPool::new(Arc::clone(&domain));
    let mut session = pool.acquire().unwrap();
    session
        .query("CREATE TABLE system_pool_affinity (id INT PRIMARY KEY)")
        .unwrap();
    session.begin().unwrap();
    let (role, start_ts) = session.read_bdr_role_and_start_ts().unwrap();
    assert_eq!(role, "primary");
    assert!(start_ts > 0);
    assert_eq!(session.transaction_start_ts().unwrap(), start_ts);
    let version = session.current_version().unwrap();
    session.lock_global_id_key(version).unwrap();
    session.set_snapshot_ts(version);
    let ids = session.generate_global_ids(2).unwrap();
    assert_eq!(ids[1], ids[0] + 1);
    session
        .query("INSERT INTO system_pool_affinity VALUES (42)")
        .unwrap();
    session.commit().unwrap();
    session.begin().unwrap();
    session
        .query("INSERT INTO system_pool_affinity VALUES (43)")
        .unwrap();
    session.generate_global_ids(1).unwrap();
    session.rollback();
    assert_eq!(
        session
            .query("SELECT id FROM system_pool_affinity")
            .unwrap(),
        vec![vec!["42".to_owned()]]
    );
    session.begin().unwrap();
    let next = session.generate_global_ids(1).unwrap();
    assert_eq!(
        next[0],
        ids[1] + 1,
        "rollback must restore the Go global ID meta value"
    );
    session.commit().unwrap();
    drop(session);
    pool.close();
    assert!(pool.acquire().is_err());
    domain.close();
}

#[test]
fn crossks_align_system_session_register_cancel_and_close() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(Arc::clone(&domain));
    let cancelled = astersql_session_syssession::CancellationToken::default();
    cancelled.cancel();
    assert!(pool.acquire_with_cancellation(&cancelled).is_err());
    // Go's capacity is an idle cache: a sixth borrower must not wait for a slot.
    let sessions = (0..6).map(|_| pool.acquire().unwrap()).collect::<Vec<_>>();
    let ids = sessions
        .iter()
        .map(|session| session.session_id())
        .collect::<Vec<_>>();
    for id in &ids {
        assert!(astersql_ddl_session::internal_session_ids().contains(id));
    }
    pool.close();
    for session in &sessions {
        assert!(session.query("SELECT 1").is_err());
    }
    drop(sessions);
    for id in &ids {
        assert!(!astersql_ddl_session::internal_session_ids().contains(id));
    }
    assert!(pool.acquire().is_err());
    domain.close();
}

#[test]
fn crossks_align_system_session_common_job_and_table_pool() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pool = SystemSessionPool::new(Arc::clone(&domain));
    let mut session = astersql_ddl_jobsubmit::SessionPool::get(pool.as_ref()).unwrap();
    session.begin().unwrap();
    // Seed a nonempty, Go-encoded metadata value before validating persistence.
    let first = session.generate_global_ids(2).unwrap();
    session.commit().unwrap();
    astersql_ddl_jobsubmit::SessionPool::put(pool.as_ref(), session);
    let mut session = astersql_ddl_jobsubmit::SessionPool::get(pool.as_ref()).unwrap();
    session.begin().unwrap();
    let next = session.generate_global_ids(1).unwrap();
    assert_eq!(next[0], first[1] + 1);
    session.rollback();
    astersql_ddl_jobsubmit::SessionPool::put(pool.as_ref(), session);
    let mut session = astersql_ddl_systable::SessionPool::get(pool.as_ref()).unwrap();
    assert_eq!(
        session
            .execute(
                &astersql_ddl_systable::Context::default(),
                "SELECT 42",
                "system"
            )
            .unwrap(),
        vec![astersql_ddl_systable::Row(vec![
            astersql_ddl_systable::Value::Int(42)
        ])]
    );
    astersql_ddl_systable::SessionPool::put(pool.as_ref(), session);
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_system_session_callbacks_and_metadata_error() {
    use astersql_session::runtime::system_session::SystemSessionCallbacks;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let (events, received) = std::sync::mpsc::channel();
    let borrowed = events.clone();
    let returned = events.clone();
    let pool = SystemSessionPool::new_with_callbacks(
        Arc::clone(&domain),
        SystemSessionCallbacks {
            borrowed: Arc::new(move |context| {
                borrowed.send(("get", context.session_id())).unwrap();
            }),
            returned: Arc::new(move |id| {
                returned.send(("put", id)).unwrap();
            }),
            destroyed: Arc::new(move |id| {
                events.send(("destroy", id)).unwrap();
            }),
        },
    );
    let session = pool.acquire().unwrap();
    let id = session.session_id();
    assert_eq!(received.recv().unwrap(), ("get", id));
    session.query("SELECT 42").unwrap();
    drop(session);
    assert_eq!(received.recv().unwrap(), ("put", id));
    let mut session = pool.acquire().unwrap();
    assert_eq!(received.recv().unwrap(), ("get", id));
    session.set_snapshot_ts(42); // infallible ABI, but no active transaction
    assert!(session.query("SELECT 42").is_err());
    assert!(session.commit().is_err());
    drop(session);
    assert_eq!(received.recv().unwrap(), ("put", id));
    let session = pool.acquire().unwrap();
    assert_ne!(
        session.session_id(),
        id,
        "an errored metadata adapter cannot be reused"
    );
    let new_id = session.session_id();
    assert_eq!(received.recv().unwrap(), ("get", new_id));
    pool.close();
    assert_eq!(received.recv().unwrap(), ("destroy", new_id));
    drop(session);
    assert_eq!(received.recv().unwrap(), ("put", new_id));
    domain.close();
}
