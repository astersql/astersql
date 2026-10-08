// Copyright 2026 AsterSQL.

use astersql_ddl_jobsubmit::Session as JobSession;
use astersql_session::runtime::{CreateAnalyzeSession, system_session::SystemSessionPool};
use std::sync::Arc;
use std::sync::atomic::Ordering;

struct TxnTotalSizeLimitGuard(u64);

impl TxnTotalSizeLimitGuard {
    fn set(limit: u64) -> Self {
        Self(astersql_kv::TxnTotalSizeLimit.swap(limit, Ordering::SeqCst))
    }
}

impl Drop for TxnTotalSizeLimitGuard {
    fn drop(&mut self) {
        astersql_kv::TxnTotalSizeLimit.store(self.0, Ordering::SeqCst);
    }
}

#[test]
fn crossks_align_system_session_return_rolls_back() {
    // Go persists databases and tables as separate metadata keys. Keep the
    // limit below the legacy monolithic Rust catalog value while leaving enough
    // room for every individual system-table definition.
    let _txn_limit = TxnTotalSizeLimitGuard::set(256 * 1024);
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

static CROSSKS_MDL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct RestoreCrossKsMDL(bool, bool);
impl Drop for RestoreCrossKsMDL {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::SetEnableMDL(self.0);
        astersql_ddl_schemaver::SetMDLEnabled(self.1);
    }
}
fn enable_crossks_mdl() -> RestoreCrossKsMDL {
    let restore = RestoreCrossKsMDL(
        astersql_sessionctx_vardef::IsMDLEnabled(),
        astersql_ddl_schemaver::IsMDLEnabled(),
    );
    astersql_sessionctx_vardef::SetEnableMDL(true);
    astersql_ddl_schemaver::SetMDLEnabled(true);
    restore
}

#[test]
fn crossks_align_infoschema_real_system_table_old_transaction() {
    let _lock = CROSSKS_MDL_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    use astersql_session_sessmgr::mdldef::JobMDL;
    use std::collections::{HashMap, HashSet};
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let _restore = enable_crossks_mdl();
    let (_, table) = domain.stats_table("mysql", "tidb").unwrap();
    session.execute("begin").unwrap();
    let mut sets = session.execute("select * from mysql.tidb limit 1").unwrap();
    while sets[0].next_row().unwrap().is_some() {}
    let mut jobs = HashMap::from([(
        73,
        Arc::new(JobMDL {
            ver: domain.info_schema().SchemaMetaVersion() + 1,
            table_ids: HashSet::from([table.ID]),
        }),
    )]);
    session.transaction_mdl().check_jobs(&mut jobs);
    assert!(
        jobs.is_empty(),
        "real system-table SELECT must hold the old schema version"
    );
}

#[test]
fn crossks_align_infoschema_sql_mdl_barrier_and_release() {
    let _lock = CROSSKS_MDL_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    use astersql_ddl_schemaver::{Context, EtcdClient, MemoryEtcdClient, NewEtcdSyncer};
    use astersql_infoschema_issyncer as issyncer;
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let _restore = enable_crossks_mdl();
    let (_, table) = domain.stats_table("mysql", "tidb").unwrap();
    session.execute("begin").unwrap();
    let mut rows = session.execute("select * from mysql.tidb limit 1").unwrap();
    while rows[0].next_row().unwrap().is_some() {}
    let ddl = astersql_session::runtime::ConcreteSession::new(domain.clone());
    ddl.execute("create table mdl_version_advance (id int)")
        .unwrap();
    let version = domain.info_schema().SchemaMetaVersion();
    ddl.execute(&format!(
        "insert into mysql.tidb_mdl_info (job_id, version, table_ids) values (73, {version}, '{}'), (74, {version}, '{}')",
        table.ID, table.ID
    ))
    .unwrap();
    let pool = SystemSessionPool::new(domain.clone());
    let min = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
        astersql_ddl_systable::new_manager(pool.clone()),
    ));
    let coordinator = Arc::new(astersql_domain_crossks::new_schema_coordinator());
    coordinator.store_internal_session(Arc::new(astersql_domain_crossks::RegisteredMDLSession {
        id: 1,
        mdl: session.transaction_mdl(),
    }));
    let etcd = Arc::new(MemoryEtcdClient::default());
    let protocol = NewEtcdSyncer(etcd.clone(), "virtual-sql");
    protocol.Init(Context::Background()).unwrap();
    let getter = coordinator.clone();
    let mut syncer = issyncer::New(
        Some(Arc::new(issyncer::KvSchemaStore::from_source(
            domain.storage_handle(),
        ))),
        Some(Arc::new(issyncer::InfoCache::from_shared(
            domain.info_cache(),
        ))),
        1000,
        Some(pool.clone()),
        None,
        None,
    );
    syncer.InitRequiredFields(Arc::new(move || Some(getter.clone())), protocol.clone());
    syncer.SetMinJobIDRefresher(min);
    syncer.Reload().unwrap();
    syncer.RefreshMDLFromSQL().unwrap();
    assert!(syncer.mdlCheckContains(table.ID));
    let path = format!(
        "{}/73/virtual-sql",
        astersql_ddl_schemaver::DDLAllSchemaVersionsByJob
    );
    syncer.CheckMDL().unwrap();
    assert!(
        EtcdClient::Get(etcd.as_ref(), &Context::Background(), &path, false)
            .unwrap()
            .Kvs
            .is_empty()
    );
    session.execute("commit").unwrap();
    etcd.FailPuts(astersql_ddl_schemaver::keyOpDefaultRetryCnt as usize);
    assert!(syncer.CheckMDL().is_err());
    let prefix = astersql_ddl_schemaver::DDLAllSchemaVersionsByJob;
    assert_eq!(
        EtcdClient::Get(etcd.as_ref(), &Context::Background(), prefix, true)
            .unwrap()
            .Kvs
            .len(),
        1,
        "one failed job must not prevent publishing the other"
    );
    let syncer = Arc::new(syncer);
    let context = Context::Background().WithTimeout(std::time::Duration::from_secs(2));
    let run = syncer.clone();
    let ctx = context.clone();
    let worker = std::thread::spawn(move || run.MDLCheckLoop(ctx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let published = loop {
        let value = EtcdClient::Get(etcd.as_ref(), &Context::Background(), prefix, true).unwrap();
        if value.Kvs.len() == 2 {
            assert!(
                value
                    .Kvs
                    .iter()
                    .all(|entry| entry.Value == version.to_string().as_bytes())
            );
            break value.Kvs[0].Value.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "MDL loop did not publish after commit"
        );
        std::thread::yield_now();
    };
    assert_eq!(published, version.to_string().as_bytes());
    context.Cancel();
    worker.join().unwrap().unwrap();
    protocol.Close();
    pool.close();
    domain.close();
}

#[test]
fn crossks_align_infoschema_prepared_read_and_transaction_cleanup() {
    use astersql_session_sessmgr::mdldef::JobMDL;
    use std::collections::{HashMap, HashSet};
    let _lock = CROSSKS_MDL_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let _restore = enable_crossks_mdl();
    session
        .execute("create table mdl_planned (id int primary key, value int)")
        .unwrap();
    session
        .execute("insert into mdl_planned values (1, 9)")
        .unwrap();
    let (_, table) = domain.stats_table("test", "mdl_planned").unwrap();
    let jobs = || {
        HashMap::from([(
            74,
            Arc::new(JobMDL {
                ver: domain.info_schema().SchemaMetaVersion() + 1,
                table_ids: HashSet::from([table.ID]),
            }),
        )])
    };
    let mut unblocked = jobs();
    session.transaction_mdl().check_jobs(&mut unblocked);
    assert_eq!(
        unblocked.len(),
        1,
        "autocommit writes release MDL after completion"
    );
    let prepared = session
        .PreparePlannedKVSelect(
            "select value from mdl_planned where id = ?",
            domain.info_schema(),
        )
        .unwrap();
    session.execute("begin").unwrap();
    let snapshot = domain.storage_handle().with_storage(|store| {
        let version = store.CurrentVersion("global").unwrap();
        store.GetSnapshot(version)
    });
    let result = session
        .ExecutePreparedPlannedKVSelect(
            prepared,
            &[astersql_types::datum::NewIntDatum(1)],
            snapshot.as_ref(),
        )
        .unwrap();
    assert_eq!(result.Rows.len(), 1);
    let mut blocked = jobs();
    session.transaction_mdl().check_jobs(&mut blocked);
    assert!(
        blocked.is_empty(),
        "typed prepared execution holds table MDL"
    );
    session.execute("rollback").unwrap();
    let mut unblocked = jobs();
    session.transaction_mdl().check_jobs(&mut unblocked);
    assert_eq!(unblocked.len(), 1);
    session.execute("begin").unwrap();
    let mut sets = session.execute("select * from mdl_planned").unwrap();
    while sets[0].next_row().unwrap().is_some() {}
    let mdl = session.transaction_mdl();
    drop(sets);
    drop(session);
    let mut unblocked = jobs();
    mdl.check_jobs(&mut unblocked);
    assert_eq!(unblocked.len(), 1, "session close releases transaction MDL");
    domain.close();
}

#[test]
fn crossks_align_infoschema_public_column_revision_fences_old_transaction() {
    let _lock = CROSSKS_MDL_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let _restore = enable_crossks_mdl();
    session
        .execute("create table mdl_revision (id int primary key, value int)")
        .unwrap();
    session
        .execute("insert into mdl_revision values (1, 9)")
        .unwrap();
    session.execute("begin").unwrap();
    let (_, mut table) = domain.stats_table("test", "mdl_revision").unwrap();
    table.Revision += 1;
    let column = table
        .Columns
        .iter_mut()
        .find(|column| column.Name.L == "value")
        .unwrap();
    column.ID += 100;
    let version = domain.info_schema().SchemaMetaVersion() + 1;
    let string = |name: &[u8]| {
        astersql_kv::Key(astersql_util_codec::EncodeUint(
            astersql_util_codec::EncodeBytes(vec![b'm'], name),
            b's' as u64,
        ))
    };
    let hash = |name: &[u8], field: &[u8]| {
        astersql_kv::Key(astersql_util_codec::EncodeBytes(
            astersql_util_codec::EncodeUint(
                astersql_util_codec::EncodeBytes(vec![b'm'], name),
                b'h' as u64,
            ),
            field,
        ))
    };
    let mut transaction = domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    transaction
        .Set(
            hash(
                format!("DB:{}", table.DBID).as_bytes(),
                format!("Table:{}", table.ID).as_bytes(),
            ),
            astersql_meta_model::EncodeTableInfo(&table).unwrap(),
        )
        .unwrap();
    transaction
        .Set(
            string(b"SchemaVersionKey"),
            version.to_string().into_bytes(),
        )
        .unwrap();
    transaction
        .Set(
            string(format!("Diff:{version}").as_bytes()),
            format!(
                "{{\"version\":{version},\"type\":12,\"schema_id\":{},\"table_id\":{}}}",
                table.DBID, table.ID
            )
            .into_bytes(),
        )
        .unwrap();
    transaction.Commit(&astersql_kv::Context::new()).unwrap();
    domain.reload().unwrap();
    let error = match session.execute("select value from mdl_revision") {
        Ok(_) => panic!("an old transaction must reject a changed public column ID"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("public column value has changed")
    );
    session.execute("rollback").unwrap();
    domain.close();
}
