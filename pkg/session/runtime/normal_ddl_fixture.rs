// Copyright 2026 AsterSQL.

use super::{BootstrapCanonicalDomain, ConcreteSession, system_session::SystemSessionPool};
use astersql_meta_model::group_3::{Job, JobState};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

fn bootstrap_store() -> &'static astersql_store_mockstore_mockstorage::KVStore {
    static STORE: OnceLock<astersql_store_mockstore_mockstorage::KVStore> = OnceLock::new();
    STORE.get_or_init(|| {
        let kv = astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO();
        let storage = Arc::try_unwrap(
            astersql_store_mockstore_mockstorage::NewMockStorage(kv.clone(), None).unwrap(),
        )
        .ok()
        .unwrap();
        let mut config = astersql_domain::DomainConfig::default();
        config.schema_lease = Duration::ZERO;
        config.stats_lease = Duration::ZERO;
        let domain = Arc::new(astersql_domain::Domain::new(
            storage,
            Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
            config,
        ));
        domain.init().unwrap();
        let session = BootstrapCanonicalDomain(domain.clone()).unwrap();
        domain.set_global_system_variable("tidb_cdc_write_source", "9");
        drop(session);
        domain.close();
        kv
    })
}

pub(super) fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn hash(h: &[u8], f: &[u8]) -> astersql_kv::Key {
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    astersql_kv::Key(EncodeBytes(
        EncodeUint(EncodeBytes(vec![b'm'], h), b'h' as u64),
        f,
    ))
}
pub(super) struct Fixture {
    pub(super) domain: Arc<astersql_domain::Domain>,
    pub(super) pool: Arc<SystemSessionPool>,
    pub(super) db: i64,
    pub(super) table: i64,
    pub(super) kv: astersql_store_mockstore_mockstorage::KVStore,
    pub(super) clean: astersql_store_mockstore_mockstorage::KVStore,
}
impl Fixture {
    pub(super) fn new() -> Self {
        Self::new_with_stats(true)
    }

    pub(super) fn new_without_stats() -> Self {
        Self::new_with_stats(false)
    }

    fn new_with_stats(initialize_stats: bool) -> Self {
        let kv = if initialize_stats {
            bootstrap_store().ForkForTest()
        } else {
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO()
        };
        let storage = Arc::try_unwrap(
            astersql_store_mockstore_mockstorage::NewMockStorage(kv.clone(), None).unwrap(),
        )
        .ok()
        .unwrap();
        let mut config = astersql_domain::DomainConfig::default();
        config.schema_lease = Duration::ZERO;
        config.stats_lease = Duration::ZERO;
        let domain = Arc::new(astersql_domain::Domain::new(
            storage,
            Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
            config,
        ));
        domain.init().unwrap();
        if initialize_stats {
            domain.initialize_stats().unwrap();
        } else {
            // The focused service regression only exercises durable DDL queue
            // tables. Avoid installing every bootstrap table in its timed path.
            let session = ConcreteSession::new(domain.clone());
            for database in ["mysql", "sys", "test"] {
                session
                    .execute(&format!("CREATE DATABASE IF NOT EXISTS {database}"))
                    .unwrap();
            }
            session
                .execute(astersql_meta_metadef::CreateTiDBDDLJobTable)
                .unwrap();
        }
        domain.set_global_system_variable("tidb_cdc_write_source", "9");
        let pool = SystemSessionPool::new(domain.clone());
        pool.acquire()
            .unwrap()
            .query("CREATE TABLE test.normal_ddl_target (id int primary key, payload varchar(40))")
            .unwrap();
        let db = domain
            .info_schema()
            .AllSchemas()
            .into_iter()
            .find(|s| s.name.lower == "test")
            .unwrap()
            .id;
        let table = domain
            .table_by_name("test", "normal_ddl_target")
            .unwrap()
            .ID;
        // Seed non-empty, complete Go table metadata in the actual MVCC Store.
        let mut txn = domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        let info = domain.table_by_name("test", "normal_ddl_target").unwrap();
        txn.Set(
            hash(
                format!("DB:{db}").as_bytes(),
                format!("Table:{table}").as_bytes(),
            ),
            astersql_meta_model::EncodeTableInfo(&info).unwrap(),
        )
        .unwrap();
        let dbinfo = astersql_meta_model::DBInfo {
            ID: db,
            Name: astersql_meta_model::ast::NewCIStr("test"),
            State: astersql_meta_model::SchemaState::Public,
            ..Default::default()
        };
        txn.Set(
            hash(b"DBs", format!("DB:{db}").as_bytes()),
            astersql_meta_model::EncodeDBInfo(&dbinfo).unwrap(),
        )
        .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        let clean = kv.ForkForTest();
        Self {
            domain,
            pool,
            db,
            table,
            kv,
            clean,
        }
    }
    pub(super) fn reset(&self) {
        self.kv.RestoreFromForTest(&self.clean);
    }
    pub(super) fn insert(&self, id: i64, state: JobState) {
        use astersql_ddl_jobsubmit::{
            AlterTableModeTarget, SessionVariables, TableMode, build_alter_table_mode_job,
            table_mode_args,
        };
        let (job, args, noop) = build_alter_table_mode_job(
            SessionVariables {
                cdc_write_source: 41,
                sql_mode: 7,
            },
            AlterTableModeTarget {
                current_mode: TableMode::Normal,
                target_mode: TableMode::Import,
                schema_id: self.db,
                table_id: self.table,
                schema_name: "test".into(),
                table_name: "normal_ddl_target".into(),
            },
        )
        .unwrap();
        assert!(!noop);
        let mut built = job.unwrap();
        built.id = id;
        let mut job = Job::decode(&built.encode(&table_mode_args(args.unwrap()))).unwrap();
        job.state = state;
        self.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id, reorg, schema_ids, table_ids, job_meta, type, processing) VALUES ({id},0,'{}','{}',X'{}',75,0)",self.db,self.table,hex(&job.encode(false).unwrap()))).unwrap();
    }
    pub(super) fn queue(&self, id: i64) -> Option<Job> {
        let rows = self
            .pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id={id}"
            ))
            .unwrap();
        rows.first()
            .map(|r| astersql_meta::decode_go_history_job(r[0].as_bytes()).unwrap())
    }
    pub(super) fn reader(&self) -> astersql_meta::SnapshotReader {
        let snapshot = self
            .domain
            .storage_handle()
            .with_storage(|s| {
                let v = s.CurrentVersion("global")?;
                Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
            })
            .unwrap();
        astersql_meta::SnapshotReader::new(snapshot)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.pool.close();
        self.domain.close();
    }
}
