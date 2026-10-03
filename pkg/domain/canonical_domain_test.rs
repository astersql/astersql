// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Canonical Domain 生命周期单测。
//
// Domain：TiDB/AsterSQL 中承载 InfoSchema、存储句柄与 keyspace 运行时的核心对象。
// 本测试用 Mock Storage / Loader 验证初始化、reload、快照缓存与关闭后不可再开事务。

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;

use super::domain::CrossKeyspaceCoordinator;
use super::{Domain, DomainConfig, InfoSchemaLoader, LoadedInfoSchema};

#[test]
fn go_merge_43_loader_reads_go_meta_without_private_catalog() {
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let database = astersql_meta_model::DBInfo {
        ID: 101,
        Name: astersql_parser_ast::NewCIStr("go_target"),
        ..Default::default()
    };
    let table = astersql_meta_model::TableInfo {
        ID: 102,
        DBID: database.ID,
        Name: astersql_parser_ast::NewCIStr("items"),
        ..Default::default()
    };
    let mut transaction = storage.Begin(&[]).unwrap();
    transaction.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"1".to_vec(),
    );
    transaction.Set(
        super::canonical_domain::tidb_string_key(b"NextGlobalID").0,
        b"102".to_vec(),
    );
    transaction.Set(
        super::canonical_domain::tidb_hash_key(b"DBs", b"DB:101").0,
        astersql_meta_model::EncodeDBInfo(&database).unwrap(),
    );
    transaction.Set(
        super::canonical_domain::tidb_hash_key(b"DB:101", b"Table:102").0,
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    );
    transaction.Set(
        super::canonical_domain::tidb_hash_key(b"DB:101", b"TID:102").0,
        b"4".to_vec(),
    );
    transaction.Commit().unwrap();
    let loader = super::canonical_domain::KvInfoSchemaLoader::new();
    let loaded = loader.load_info_schema(storage.as_ref(), "target").unwrap();
    assert_eq!(loaded.schema.SchemaMetaVersion(), 1);
    assert!(
        loaded
            .schema
            .TableByName(
                &infoschema::CiString::new("go_target"),
                &infoschema::CiString::new("items"),
            )
            .is_ok()
    );
    super::canonical_domain::DdlMetadataService::new()
        .set_table_mode(
            storage.as_ref(),
            "go_target",
            "items",
            astersql_meta_model::TableMode::TableModeImport,
        )
        .unwrap();
    let reloaded = loader.load_info_schema(storage.as_ref(), "target").unwrap();
    assert_eq!(reloaded.schema.SchemaMetaVersion(), 2);
    let table = reloaded
        .schema
        .TableByName(
            &infoschema::CiString::new("go_target"),
            &infoschema::CiString::new("items"),
        )
        .unwrap();
    assert_eq!(
        table.Meta().model_meta.as_ref().unwrap().Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
}

struct GoMerge43ExternalManager {
    role: String,
    updated: Arc<Mutex<Vec<bool>>>,
    ttl_events: Arc<Mutex<Vec<(String, i64, bool)>>>,
}

impl astersql_extworkload::Manager for GoMerge43ExternalManager {
    fn Close(&mut self) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn Role(&self) -> String {
        self.role.clone()
    }
    fn Meta(&self) -> Option<&astersql_extworkload::keyspacepb::KeyspaceMeta> {
        None
    }
    fn InitializeGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn AbortGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RegisterGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RecycleGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn UpdateGCLifeTime(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RegisterTTLTask(
        &mut self,
        _: &astersql_extworkload::context::Context,
        table_id: i64,
        enabled: bool,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.ttl_events
            .lock()
            .unwrap()
            .push(("register".into(), table_id, enabled));
        Ok(())
    }
    fn DeleteTTLTableInfo(
        &mut self,
        _: &astersql_extworkload::context::Context,
        table_id: i64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.ttl_events
            .lock()
            .unwrap()
            .push(("delete".into(), table_id, false));
        Ok(())
    }
    fn RecycleTTLTask(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn UpdateTTLJobEnable(
        &mut self,
        _: &astersql_extworkload::context::Context,
        enabled: bool,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.updated.lock().unwrap().push(enabled);
        Ok(())
    }
    fn RegisterAutoAnalyze(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RecycleAutoAnalyze(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
}

#[test]
fn go_merge_43_external_workload_role_gates_ttl_and_master_updates() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    let updated = Arc::new(Mutex::new(Vec::new()));
    let ttl_events = Arc::new(Mutex::new(Vec::new()));
    let context = astersql_extworkload::context::Background();

    domain.set_external_workload_manager(Some(Box::new(GoMerge43ExternalManager {
        role: astersql_config::RoleMaster.into(),
        updated: Arc::clone(&updated),
        ttl_events: Arc::clone(&ttl_events),
    })));
    assert!(!domain.should_start_ttl_job_manager());
    domain
        .update_external_workload_ttl_job_enable(&context, false)
        .unwrap();
    assert_eq!(*updated.lock().unwrap(), [false]);

    domain.set_external_workload_manager(Some(Box::new(GoMerge43ExternalManager {
        role: astersql_config::RoleTTLTaskWorker.into(),
        updated: Arc::clone(&updated),
        ttl_events: Arc::clone(&ttl_events),
    })));
    assert!(domain.should_start_ttl_job_manager());
    domain
        .update_external_workload_ttl_job_enable(&context, true)
        .unwrap();
    assert_eq!(*updated.lock().unwrap(), [false]);
}

#[test]
fn go_merge_43_ddl_registers_and_deletes_ttl_table_with_external_manager() {
    let storage = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            None,
        )
        .unwrap(),
    )
    .unwrap_or_else(|_| panic!("mock storage has another owner"));
    let domain = Domain::new(
        storage,
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    domain.init().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    domain.set_external_workload_manager(Some(Box::new(GoMerge43ExternalManager {
        role: astersql_config::RoleMaster.into(),
        updated: Arc::new(Mutex::new(Vec::new())),
        ttl_events: Arc::clone(&events),
    })));
    let table = astersql_meta_model::TableInfo {
        Name: astersql_parser_ast::NewCIStr("external_ttl"),
        TTLInfo: Some(astersql_meta_model::TTLInfo {
            ColumnName: astersql_parser_ast::NewCIStr("expire_at"),
            IntervalExprStr: "1".into(),
            IntervalTimeUnit: astersql_parser_ast::TimeUnitType::Day as i32,
            Enable: true,
            JobInterval: "24h".into(),
        }),
        ..Default::default()
    };
    let created = domain.ddl_create_table("test", table, false).unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![(
            "register".to_owned(),
            created.ID,
            astersql_sessionctx_vardef::EnableTTLJob.Load(),
        )]
    );
    domain
        .ddl_drop_tables(vec![("test".into(), "external_ttl".into())], false)
        .unwrap();
    assert_eq!(
        events.lock().unwrap()[1],
        ("delete".into(), created.ID, false)
    );
}

#[test]
fn go_merge_43_domain_passes_server_info_option_and_cleans_registration() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    domain.init().unwrap();
    let etcd = Arc::new(astersql_domain_serverinfo::MemoryEtcdClient::default());
    domain
        .install_server_info_syncer(
            "bootstrap".into(),
            etcd.clone(),
            &[astersql_domain_serverinfo::SyncerOption::WithoutStatusEndpointClaim],
        )
        .unwrap();
    let keys = etcd.Snapshot();
    assert!(keys.contains_key("/tidb/server/info/bootstrap"));
    assert!(keys.keys().any(|key| key.starts_with("/topology/tidb/")));
    assert!(
        !keys
            .keys()
            .any(|key| key.starts_with("/tidb/server/status_addr/"))
    );
    domain.start(super::domain::StartMode::Normal).unwrap();
    domain.close();
    let keys = etcd.Snapshot();
    assert!(!keys.contains_key("/tidb/server/info/bootstrap"));
    assert!(!keys.keys().any(|key| key.starts_with("/topology/tidb/")));
}

struct GoMerge43UnavailableCrossKSFactory;

impl astersql_domain_crossks::RuntimeFactory for GoMerge43UnavailableCrossKSFactory {
    fn create(
        &self,
        _: &str,
    ) -> Result<Arc<astersql_domain_crossks::SessionManager>, astersql_domain_crossks::ManagerError>
    {
        Err(astersql_domain_crossks::ManagerError(
            "runtime unavailable".into(),
        ))
    }
}

#[test]
fn go_merge_43_domain_closes_installed_cross_ks_manager() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    let manager = astersql_domain_crossks::new_manager(
        false,
        "SYSTEM",
        Arc::new(GoMerge43UnavailableCrossKSFactory),
    );
    domain.install_cross_ks_manager(manager.clone());
    assert!(domain.cross_ks_manager().is_some());
    domain.close();
    assert!(manager.get_or_create("tenant").is_err());
    assert!(domain.cross_ks_manager().is_none());
}

#[test]
fn go_merge_43_domain_owns_inference_provider_lifecycle() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    assert!(domain.get_embed_fn().is_none());
    domain.init().unwrap();
    domain.start(crate::domain::StartMode::Normal).unwrap();
    let embed_fn = domain
        .get_embed_fn()
        .expect("provider initialized on start");
    assert!(embed_fn.has_embedder("openai"));
    assert!(embed_fn.has_embedder("jina_ai"));
    assert!(embed_fn.has_embedder("cohere"));
    assert!(embed_fn.has_embedder("huggingface"));
    assert!(embed_fn.has_embedder("nvidia_nim"));
    assert!(embed_fn.has_embedder("gemini"));
    assert!(
        embed_fn
            .embed("openai/model", "text", &Default::default(), &|| false)
            .unwrap_err()
            .contains("API key is not configured")
    );
    domain.set_global_system_variable("tidb_exp_embed_openai_api_key", "test-key");
    domain.set_global_system_variable("tidb_exp_embed_openai_api_base", "invalid-url");
    assert!(
        embed_fn
            .embed("openai/model", "text", &Default::default(), &|| false)
            .unwrap_err()
            .contains("invalid OpenAI API base URL")
    );
    assert_eq!(
        embed_fn
            .embed("mock/json", "[1,2]", &Default::default(), &|| false)
            .unwrap(),
        [1.0, 2.0]
    );
    domain.close();
    assert!(domain.get_embed_fn().is_none());
    assert!(
        embed_fn
            .embed("mock/json", "[1]", &Default::default(), &|| false)
            .is_err()
    );
    domain.close();
    assert!(domain.get_embed_fn().is_none());
}

#[test]
fn go_merge_43_hosted_embedding_registration_requires_starter_and_enabled() {
    let mut config = astersql_config::new_config();
    assert!(!super::domain::hosted_embedding_enabled(&config, true));
    config.hosted_embedding.enabled = true;
    assert!(!super::domain::hosted_embedding_enabled(&config, false));
    assert!(super::domain::hosted_embedding_enabled(&config, true));
    config.auto_scaler_cluster_id = "tenant-1".into();
    assert_eq!(config.auto_scaler_cluster_id, "tenant-1");
}

#[test]
fn go_merge_43_ttl_does_not_start_with_config_but_no_controller() {
    struct Restore(Arc<astersql_config::Config>);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config((*self.0).clone());
        }
    }
    let _restore = Restore(astersql_config::get_global_config());
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    astersql_config::update_global(|config| {
        config.external_workload.Enable = false;
    });
    assert!(domain.should_start_ttl_job_manager());
    astersql_config::update_global(|config| {
        config.external_workload.Enable = true;
        config.external_workload.Role.clear();
    });
    assert_eq!(
        domain.ttl_external_workload_role(),
        (astersql_config::RoleMaster.into(), true)
    );
    assert!(!domain.should_start_ttl_job_manager());
    astersql_config::update_global(|config| {
        config.external_workload.Role = astersql_config::RoleTTLTaskWorker.into();
    });
    assert!(!domain.should_start_ttl_job_manager());
}

/// 可配置版本号并记录观测到的 Storage UUID 的 InfoSchemaLoader。
struct TestSchemaLoader {
    version: AtomicI64,
    observed_storage_ids: Mutex<Vec<String>>,
}

impl TestSchemaLoader {
    /// 以给定 schema 版本构造 Loader。
    fn new(version: i64) -> Self {
        Self {
            version: AtomicI64::new(version),
            observed_storage_ids: Mutex::new(Vec::new()),
        }
    }

    /// 更新后续 load_info_schema 返回的版本。
    fn set_version(&self, version: i64) {
        self.version.store(version, Ordering::Release);
    }

    /// 构造带指定版本与时间戳的 Mock InfoSchema。
    fn schema(version: i64, timestamp: u64) -> LoadedInfoSchema {
        let schema: SchemaRef =
            infoschema::infoschema::MockInfoSchemaWithSchemaVer(Vec::new(), version);
        LoadedInfoSchema::new(schema, timestamp)
    }

    /// 记录本次加载所使用的 Storage UUID。
    fn observe(&self, store: &dyn kv::Storage) {
        self.observed_storage_ids
            .lock()
            .expect("loader observation lock poisoned")
            .push(store.UUID());
    }
}

impl InfoSchemaLoader for TestSchemaLoader {
    fn load_info_schema(
        &self,
        store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.observe(store);
        let version = self.version.load(Ordering::Acquire);
        Ok(Self::schema(version, version as u64 * 10))
    }

    fn load_snapshot_info_schema(
        &self,
        store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.observe(store);
        Ok(Self::schema((timestamp / 10) as i64, timestamp))
    }

    fn keyspace_exists(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        self.observe(store);
        Ok(keyspace == "SYSTEM")
    }
}

/// 返回一个真实用户表，用于验证启动统计加载不会开启写事务。
struct TableSchemaLoader;

impl TableSchemaLoader {
    fn schema(version: i64) -> LoadedInfoSchema {
        let mut schema = infoschema::infoschema::infoSchema::new(version);
        let table = astersql_meta_model::TableInfo {
            ID: 42,
            DBID: 1,
            Name: astersql_parser_ast::NewCIStr("t"),
            State: astersql_meta_model::StatePublic,
            ..astersql_meta_model::TableInfo::default()
        };
        schema.add_schema(
            infoschema::DBInfo {
                id: 1,
                name: infoschema::CiString::new("test"),
                ..infoschema::DBInfo::default()
            },
            vec![infoschema::Table::from_model(table)],
        );
        LoadedInfoSchema::new(Arc::new(schema), version as u64 * 10)
    }
}

impl InfoSchemaLoader for TableSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(Self::schema(1))
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(Self::schema((timestamp / 10) as i64))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(keyspace == "SYSTEM")
    }
}

struct EmptySnapshot;

impl kv::Getter for EmptySnapshot {
    fn Get(
        &self,
        context: &kv::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        kv::EmptyRetriever.Get(context, key, options)
    }
}

impl kv::Retriever for EmptySnapshot {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::EmptyRetriever.Iter(key, upper_bound)
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::EmptyRetriever.IterReverse(key, lower_bound)
    }
}

impl kv::Snapshot for EmptySnapshot {
    fn BatchGet(
        &self,
        _context: &kv::Context,
        _keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<std::collections::HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        Ok(std::collections::HashMap::new())
    }

    fn SetOption(&mut self, _option: i32, _value: Option<Box<dyn Any>>) {}
}

#[derive(Default)]
struct TransactionCounters {
    begins: AtomicI64,
    commits: AtomicI64,
    writes: AtomicI64,
    allowed_on_almost_full: AtomicI64,
}

struct ReadOnlyTransaction {
    counters: Arc<TransactionCounters>,
    snapshot: EmptySnapshot,
    checkpoint: kv::tikv::MemDBCheckpoint,
    valid: bool,
}

impl kv::Getter for ReadOnlyTransaction {
    fn Get(
        &self,
        context: &kv::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        kv::EmptyRetriever.Get(context, key, options)
    }
}

impl kv::Retriever for ReadOnlyTransaction {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::EmptyRetriever.Iter(key, upper_bound)
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::EmptyRetriever.IterReverse(key, lower_bound)
    }
}

impl kv::Mutator for ReadOnlyTransaction {
    fn Set(&mut self, _key: kv::Key, _value: Vec<u8>) -> Result<(), kv::errors::SharedError> {
        self.counters.writes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn Delete(&mut self, _key: kv::Key) -> Result<(), kv::errors::SharedError> {
        self.counters.writes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

impl kv::RetrieverMutator for ReadOnlyTransaction {}

impl kv::FairLockingController for ReadOnlyTransaction {
    fn StartFairLocking(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    fn RetryFairLocking(&mut self, _context: &kv::Context) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    fn CancelFairLocking(&mut self, _context: &kv::Context) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    fn DoneFairLocking(&mut self, _context: &kv::Context) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    fn IsInFairLockingMode(&self) -> bool {
        false
    }
}

impl kv::Transaction for ReadOnlyTransaction {
    fn Size(&self) -> usize {
        0
    }

    fn Mem(&self) -> u64 {
        0
    }

    fn SetMemoryFootprintChangeHook(&mut self, _hook: Box<dyn Fn(u64)>) {}

    fn MemHookSet(&self) -> bool {
        false
    }

    fn Len(&self) -> usize {
        0
    }

    fn Commit(&mut self, _context: &kv::Context) -> Result<(), kv::errors::SharedError> {
        self.counters.commits.fetch_add(1, Ordering::AcqRel);
        self.valid = false;
        Ok(())
    }

    fn Rollback(&mut self) -> Result<(), kv::errors::SharedError> {
        self.valid = false;
        Ok(())
    }

    fn String(&self) -> String {
        "read-only-transaction".to_owned()
    }

    fn LockKeys(
        &mut self,
        _context: &kv::Context,
        _lock_context: &mut kv::LockCtx,
        _keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }

    fn LockKeysFunc(
        &mut self,
        _context: &kv::Context,
        _lock_context: &mut kv::LockCtx,
        callback: &mut dyn FnMut(),
        _keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        callback();
        Ok(())
    }

    fn SetOption(&mut self, _option: i32, _value: Option<Box<dyn Any>>) {}

    fn GetOption(&self, _option: i32) -> Option<&dyn Any> {
        None
    }

    fn IsReadOnly(&self) -> bool {
        self.counters.writes.load(Ordering::Acquire) == 0
    }

    fn StartTS(&self) -> u64 {
        1
    }

    fn CommitTS(&self) -> u64 {
        0
    }

    fn Valid(&self) -> bool {
        self.valid
    }

    fn GetMemBuffer(&self) -> &dyn kv::MemBuffer {
        panic!("memory buffer is not used by this test")
    }

    fn GetSnapshot(&self) -> &dyn kv::Snapshot {
        &self.snapshot
    }

    fn SetVars(&mut self, _variables: Box<dyn Any>) {}

    fn GetVars(&self) -> &dyn Any {
        &()
    }

    fn BatchGet(
        &self,
        _context: &kv::Context,
        _keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        Ok(HashMap::new())
    }

    fn IsPessimistic(&self) -> bool {
        false
    }

    fn CacheTableInfo(&mut self, _id: i64, _info: kv::model::TableInfo) {}

    fn GetTableInfo(&self, _id: i64) -> Option<&kv::model::TableInfo> {
        None
    }

    fn SetDiskFullOpt(&mut self, level: kv::kvrpcpb::DiskFullOpt) {
        if level == kv::kvrpcpb::DiskFullOpt::AllowedOnAlmostFull {
            self.counters
                .allowed_on_almost_full
                .fetch_add(1, Ordering::AcqRel);
        }
    }

    fn ClearDiskFullOpt(&mut self) {}

    fn GetMemDBCheckpoint(&self) -> &kv::tikv::MemDBCheckpoint {
        &self.checkpoint
    }

    fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &kv::tikv::MemDBCheckpoint) {}

    fn IsPipelined(&self) -> bool {
        false
    }

    fn MayFlush(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }
}

/// 仅实现关闭与版本查询的最小化 Storage Mock。
pub(super) struct TestStorage {
    closed: AtomicBool,
    transaction_counters: Option<Arc<TransactionCounters>>,
}

impl TestStorage {
    /// 构造未关闭的 TestStorage。
    pub(super) fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
            transaction_counters: None,
        }
    }

    fn with_transactions(counters: Arc<TransactionCounters>) -> Self {
        Self {
            closed: AtomicBool::new(false),
            transaction_counters: Some(counters),
        }
    }
}

impl kv::Storage for TestStorage {
    fn Begin(
        &self,
        _options: &[kv::tikv::TxnOption],
    ) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(kv::errors::New("storage is closed"));
        }
        let Some(counters) = &self.transaction_counters else {
            return Err(kv::errors::New("transactions are not used by this test"));
        };
        counters.begins.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(ReadOnlyTransaction {
            counters: Arc::clone(counters),
            snapshot: EmptySnapshot,
            checkpoint: kv::tikv::MemDBCheckpoint::default(),
            valid: true,
        }))
    }

    fn GetSnapshot(&self, _version: kv::Version) -> Box<dyn kv::Snapshot> {
        Box::new(EmptySnapshot)
    }

    fn GetClient(&self) -> &dyn kv::Client {
        panic!("client is not used by this test")
    }

    fn GetMPPClient(&self) -> &dyn kv::MPPClient {
        panic!("MPP client is not used by this test")
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }

    fn UUID(&self) -> String {
        "mock-storage".to_owned()
    }

    fn CurrentVersion(
        &self,
        _transaction_scope: &str,
    ) -> Result<kv::Version, kv::errors::SharedError> {
        Ok(kv::NewVersion(1))
    }

    fn GetOracle(&self) -> &dyn kv::oracle::Oracle {
        panic!("oracle is not used by this test")
    }

    fn SupportDeleteRange(&self) -> bool {
        false
    }

    fn Name(&self) -> String {
        "mock".to_owned()
    }

    fn Describe(&self) -> String {
        "canonical domain test storage".to_owned()
    }

    fn ShowStatus(
        &self,
        _context: &kv::context::Context,
        _key: &str,
    ) -> Result<Box<dyn Any>, kv::errors::SharedError> {
        Ok(Box::new(()))
    }

    fn GetMemCache(&self) -> &dyn kv::MemManager {
        panic!("memory cache is not used by this test")
    }

    fn GetMinSafeTS(&self, _transaction_scope: &str) -> u64 {
        0
    }

    fn GetLockWaits(&self) -> Result<Vec<kv::deadlockpb::WaitForEntry>, kv::errors::SharedError> {
        Ok(Vec::new())
    }

    fn GetCodec(&self) -> kv::tikv::Codec {
        kv::tikv::Codec
    }

    fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}

    fn GetOption(&self, _key: &dyn Any) -> Option<&dyn Any> {
        None
    }

    fn GetClusterID(&self) -> u64 {
        0
    }

    fn GetKeyspace(&self) -> String {
        "SYSTEM".to_owned()
    }
}

#[test]
/// 验证 Domain 使用规范 Storage，并完成 InfoSchema 加载 / 重载 / 快照 / 关闭链路。
fn domain_uses_canonical_storage_and_infoschema_lifecycle() {
    let loader = Arc::new(TestSchemaLoader::new(1));
    let domain = Domain::new(
        TestStorage::new(),
        loader.clone(),
        DomainConfig {
            info_cache_capacity: 4,
            ..DomainConfig::default()
        },
    );

    domain.init().expect("initialize domain");
    assert_eq!(domain.info_schema().SchemaMetaVersion(), 1);
    assert_eq!(
        domain
            .storage()
            .with_storage(|store| store.CurrentVersion("global"))
            .expect("canonical current version")
            .Ver,
        1
    );

    loader.set_version(2);
    assert_eq!(domain.reload().expect("reload infoschema"), 2);
    assert_eq!(domain.info_schema().SchemaMetaVersion(), 2);
    assert_eq!(
        domain
            .snapshot_info_schema(10)
            .expect("cached snapshot infoschema")
            .SchemaMetaVersion(),
        1
    );

    let runtime = domain
        .acquire_keyspace_runtime("SYSTEM", "canonical-test")
        .expect("acquire keyspace runtime");
    assert_eq!(runtime.info_schema().SchemaMetaVersion(), 2);
    drop(runtime);

    let storage = domain.storage();
    drop(domain);
    assert!(storage.with_storage(|store| store.Begin(&[])).is_err());
    assert!(
        loader
            .observed_storage_ids
            .lock()
            .expect("loader observation lock poisoned")
            .iter()
            .all(|id| id == "mock-storage")
    );
}

#[test]
/// 对齐 Go：Domain 先发布 schema，bootstrap 后统计通过只读内部事务初始化。
fn domain_initializes_statistics_after_schema_bootstrap_in_read_only_transaction() {
    let counters = Arc::new(TransactionCounters::default());
    let domain = Domain::new(
        TestStorage::with_transactions(Arc::clone(&counters)),
        Arc::new(TableSchemaLoader),
        DomainConfig::default(),
    );

    domain.init().expect("initialize schema catalog");
    assert_eq!(domain.info_schema().SchemaMetaVersion(), 1);
    assert_eq!(counters.begins.load(Ordering::Acquire), 0);

    domain
        .initialize_stats()
        .expect("initialize stats through internal transaction");
    assert_eq!(counters.begins.load(Ordering::Acquire), 1);
    assert_eq!(counters.commits.load(Ordering::Acquire), 1);
    assert_eq!(counters.writes.load(Ordering::Acquire), 0);
    assert!(
        domain.stats_context().physical_stats(42).is_none(),
        "missing persisted statistics must retain the pseudo-statistics fallback"
    );
    assert!(
        domain
            .stats_handle()
            .lock()
            .expect("stats handle lock")
            .init_stats_done
    );
}

#[test]
/// 对齐 Go meta/DDL：内部元数据事务在 AlmostFull 时仍允许写。
fn ddl_metadata_transaction_allows_almost_full_disk() {
    let counters = Arc::new(TransactionCounters::default());
    let domain = Domain::new(
        TestStorage::with_transactions(Arc::clone(&counters)),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    domain.init().expect("initialize domain");

    domain
        .ddl_create_database("app", true)
        .expect("create database metadata");
    assert!(counters.writes.load(Ordering::Acquire) > 0);
    assert!(
        counters.allowed_on_almost_full.load(Ordering::Acquire) > 0,
        "canonical metadata writes must propagate AllowedOnAlmostFull"
    );
}

#[test]
fn canonical_info_schema_keeps_empty_databases_visible() {
    let mut catalog = super::canonical_domain::MetadataCatalog::default();
    catalog.databases.insert(
        "empty_schema".to_owned(),
        astersql_meta_model::DBInfo {
            ID: 1,
            Name: astersql_parser_ast::NewCIStr("empty_schema"),
            State: astersql_meta_model::StatePublic,
            ..astersql_meta_model::DBInfo::default()
        },
    );
    let schema = super::canonical_domain::build_info_schema(&catalog);
    assert!(
        schema
            .AllSchemas()
            .iter()
            .any(|database| database.name.lower == "empty_schema"),
        "an empty database must remain visible through InfoSchema"
    );

    catalog.databases.remove("empty_schema");
    let schema = super::canonical_domain::build_info_schema(&catalog);
    assert!(
        schema
            .AllSchemas()
            .iter()
            .all(|database| database.name.lower != "empty_schema"),
        "dropped empty database must disappear from InfoSchema"
    );
}

#[test]
fn reserved_system_ids_do_not_advance_the_user_object_allocator() {
    let mut catalog = super::canonical_domain::MetadataCatalog::default();
    let mut system_table = astersql_meta_model::TableInfo {
        ID: astersql_meta_metadef::TiDBDDLJobTableID,
        Name: astersql_parser_ast::NewCIStr("tidb_ddl_job"),
        ..astersql_meta_model::TableInfo::default()
    };
    super::canonical_domain::assign_table_physical_ids(&mut catalog, &mut system_table);
    assert_eq!(system_table.ID, astersql_meta_metadef::TiDBDDLJobTableID);

    let mut user_table = astersql_meta_model::TableInfo::default();
    super::canonical_domain::assign_table_physical_ids(&mut catalog, &mut user_table);
    assert_eq!(user_table.ID, 1);
    assert!(!astersql_meta_metadef::IsReservedID(user_table.ID));
}

#[test]
/// 验证异常退出的 ADD INDEX owner 不会阻止替代 owner 接管持久化任务。
fn pending_add_index_owner_lease_recovers_after_unwind() {
    let domain = Arc::new(Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    ));
    let failed_owner = Arc::clone(&domain);

    let exit = std::thread::spawn(move || {
        let _owner = failed_owner.pending_add_index_owner_guard();
        panic!("simulate an ADD INDEX owner process exit");
    })
    .join();
    assert!(
        exit.is_err(),
        "the former owner must exit while holding its lease"
    );

    let replacement_owner = domain.pending_add_index_owner_guard();
    drop(replacement_owner);
}

#[test]
/// 验证共享协调器的 keyspace 可见性、schema sync 汇总与任务路由。
fn cross_keyspace_coordinator_routes_runtime_state_and_tasks() {
    let coordinator = Arc::new(CrossKeyspaceCoordinator::new());
    let new_domain = || {
        Domain::new(
            TestStorage::new(),
            Arc::new(TestSchemaLoader::new(1)),
            DomainConfig::default(),
        )
    };
    let system = new_domain();
    let user_one = new_domain();
    let user_two = new_domain();
    system.bind_cross_keyspace(Arc::clone(&coordinator), "SYSTEM", true);
    user_one.bind_cross_keyspace(Arc::clone(&coordinator), "keyspace1", false);
    user_two.bind_cross_keyspace(Arc::clone(&coordinator), "keyspace2", true);

    // User runtimes eagerly know SYSTEM, while SYSTEM only discovers a user
    // after that runtime publishes ordinary user-schema DDL.
    assert_eq!(user_one.cross_keyspaces_for_test(), ["SYSTEM"]);
    assert!(system.cross_keyspaces_for_test().is_empty());
    user_one.record_cross_keyspace_ddl("app", false);
    assert_eq!(system.cross_keyspaces_for_test(), ["keyspace1"]);
    assert_eq!(
        user_one.last_cross_sync_summary_for_test().unwrap(),
        super::domain::CrossKeyspaceSyncSummary {
            ServerCount: 1,
            AssumedServerCount: 0
        }
    );

    // mysql.* issued by an undiscovered user remains local. SYSTEM schema DDL
    // nevertheless targets every bound user runtime.
    user_two.record_cross_keyspace_ddl("mysql", false);
    assert_eq!(
        user_two.last_cross_sync_summary_for_test().unwrap(),
        super::domain::CrossKeyspaceSyncSummary {
            ServerCount: 1,
            AssumedServerCount: 0
        }
    );
    system.record_cross_keyspace_ddl("mysql", false);
    assert_eq!(
        system.last_cross_sync_summary_for_test().unwrap(),
        super::domain::CrossKeyspaceSyncSummary {
            ServerCount: 3,
            AssumedServerCount: 2
        }
    );

    user_one.record_cross_keyspace_ddl("mysql", true);
    assert_eq!(
        user_one.last_cross_sync_summary_for_test().unwrap(),
        super::domain::CrossKeyspaceSyncSummary {
            ServerCount: 2,
            AssumedServerCount: 1
        }
    );
    let job_id = user_one.last_ddl_job_id_for_test().unwrap();
    let task_key = user_one.last_ddl_task_key_for_test().unwrap();
    assert_eq!(task_key, format!("ddl/backfill/{job_id}"));
    assert_eq!(
        system.cross_task_count_for_key_for_test(&job_id.to_string()),
        0
    );
    assert_eq!(system.cross_task_count_for_test(), 1);
    assert_eq!(system.cross_task_count_for_key_for_test(&task_key), 1);
    assert_eq!(user_one.cross_task_count_for_test(), 0);
    assert_eq!(
        user_one.last_backfill_collation_for_test().unwrap(),
        super::domain::BackfillCollationResolution {
            DefaultUseNewCollation: true,
            UseNewCollation: false,
            ReorgUseNewCollation: false,
        }
    );
}

#[test]
fn system_runtime_lease_is_evicted_after_the_idle_timeout() {
    let coordinator = Arc::new(CrossKeyspaceCoordinator::new());
    let new_domain = || {
        Domain::new(
            TestStorage::new(),
            Arc::new(TestSchemaLoader::new(1)),
            DomainConfig::default(),
        )
    };
    let system = new_domain();
    let user = new_domain();
    system.bind_cross_keyspace(Arc::clone(&coordinator), "SYSTEM", true);
    user.bind_cross_keyspace(coordinator, "keyspace1", false);

    let first = system
        .acquire_cross_keyspace_runtime("keyspace1", Duration::from_millis(30))
        .expect("SYSTEM acquires the user runtime");
    let second = system
        .acquire_cross_keyspace_runtime("keyspace1", Duration::from_millis(30))
        .expect("a second lease shares the same runtime");
    assert_eq!(system.cross_keyspaces_for_test(), ["keyspace1"]);

    drop(first);
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(
        system.cross_keyspaces_for_test(),
        ["keyspace1"],
        "one live lease must prevent runtime GC"
    );
    drop(second);

    let deadline = Instant::now() + Duration::from_secs(1);
    while !system.cross_keyspaces_for_test().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(system.cross_keyspaces_for_test().is_empty());

    user.record_cross_keyspace_ddl("app", false);
    let permanent = system
        .acquire_cross_keyspace_runtime("keyspace1", Duration::from_millis(20))
        .expect("lease a permanently discovered runtime");
    drop(permanent);
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(
        system.cross_keyspaces_for_test(),
        ["keyspace1"],
        "idle lease GC must not erase permanent DDL discovery"
    );
}

#[test]
fn crossks_align_meta_loader_nonempty_go_snapshot() {
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let db = astersql_meta_model::DBInfo {
        ID: astersql_meta_metadef::SystemDatabaseID,
        Name: astersql_parser_ast::NewCIStr("mysql"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let table = astersql_meta_model::TableInfo {
        ID: astersql_meta_metadef::ReservedGlobalIDUpperBound,
        DBID: db.ID,
        Name: astersql_parser_ast::NewCIStr("tidb_ddl_job"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let user_db = astersql_meta_model::DBInfo {
        ID: 101,
        Name: astersql_parser_ast::NewCIStr("user_schema"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_hash_key(b"DBs", format!("DB:{}", db.ID).as_bytes()).0,
        astersql_meta_model::EncodeDBInfo(&db).unwrap(),
    );
    tx.Set(
        super::canonical_domain::tidb_hash_key(
            format!("DB:{}", db.ID).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        )
        .0,
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"1".to_vec(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"Diff:1").0,
        format!(
            "{{\"version\":1,\"type\":3,\"schema_id\":{},\"table_id\":{}}}",
            db.ID, table.ID
        )
        .into_bytes(),
    );
    tx.Set(
        super::canonical_domain::tidb_hash_key(b"DBs", b"DB:101").0,
        astersql_meta_model::EncodeDBInfo(&user_db).unwrap(),
    );
    // A malformed user-table payload must never be decoded by the crossKS loader.
    tx.Set(
        super::canonical_domain::tidb_hash_key(b"DB:101", b"Table:102").0,
        b"invalid user metadata".to_vec(),
    );
    tx.Commit().unwrap();
    let ts = storage.CurrentVersion("global").unwrap().Ver;
    // The storage adapter must expose committed Go meta through its public
    // Reader contract as well as through Loader's timestamp-bound snapshot.
    let adapter = super::domain::KvSchemaStore::new(storage.clone());
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::MaxDiffVersion(&adapter).unwrap(),
        1
    );
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::GetTable(&adapter, db.ID, table.ID)
            .unwrap()
            .unwrap()
            .ID,
        table.ID
    );
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::GetSchemaDiff(&adapter, 1)
            .unwrap()
            .unwrap()
            .Version,
        1
    );
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::GetDatabase(&adapter, db.ID)
            .unwrap()
            .unwrap()
            .ID,
        db.ID
    );
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::ListDatabases(&adapter)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        astersql_infoschema_issyncer::SchemaReader::ListTables(&adapter, db.ID)
            .unwrap()
            .len(),
        1
    );
    let loader = astersql_infoschema_issyncer::NewLoaderForCrossKS(
        Arc::new(super::domain::KvSchemaStore::new(storage.clone())),
        None,
    );
    let (schema, _, _, change) = loader.LoadWithTS(ts, false).unwrap();
    assert_eq!(schema.Version, 1);
    assert_eq!(schema.Tables.len(), 1);
    assert_eq!(schema.Databases.len(), 1);
    assert_eq!(schema.Tables[0].ID, table.ID);
    assert!(change.is_none());
    assert!(schema.Tables[0].Model.is_some());
    let mut updated = table.clone();
    updated.Columns.push(astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("job_id"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    });
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_hash_key(
            format!("DB:{}", db.ID).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        )
        .0,
        astersql_meta_model::EncodeTableInfo(&updated).unwrap(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"2".to_vec(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"Diff:2").0,
        format!(
            "{{\"version\":2,\"type\":5,\"schema_id\":{},\"table_id\":{}}}",
            db.ID, table.ID
        )
        .into_bytes(),
    );
    tx.Commit().unwrap();
    let ts2 = storage.CurrentVersion("global").unwrap().Ver;
    let (incremental, hit, old_version, change) = loader.LoadWithTS(ts2, false).unwrap();
    assert!(!hit);
    assert_eq!(old_version, 1);
    assert_eq!(incremental.Version, 2);
    assert_eq!(
        incremental.Tables[0].Model.as_ref().unwrap().Columns.len(),
        1
    );
    let change = change.unwrap();
    assert_eq!(change.PhyTblIDS, vec![table.ID]);
    assert_eq!(
        change.ActionTypes,
        vec![astersql_infoschema_issyncer::ActionType::TableUpdate(5)]
    );
    let full = astersql_infoschema_issyncer::NewLoaderForCrossKS(
        Arc::new(super::domain::KvSchemaStore::new(storage.clone())),
        None,
    );
    let (snapshot, _, _, _) = full.LoadWithTS(ts, true).unwrap();
    assert_eq!(snapshot.Version, 1);
    assert!(
        snapshot.Tables[0]
            .Model
            .as_ref()
            .unwrap()
            .Columns
            .is_empty()
    );
    let (_, hit, _, _) = loader.LoadWithTS(ts2, false).unwrap();
    assert!(hit);
    // An allocated version without a committed diff must remain invisible.
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"3".to_vec(),
    );
    tx.Commit().unwrap();
    let ts3 = storage.CurrentVersion("global").unwrap().Ver;
    assert_eq!(full.LoadWithTS(ts3, false).unwrap().0.Version, 2);
    // User-table diffs advance the version without adding user metadata.
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_string_key(b"Diff:3").0,
        b"{\"version\":3,\"type\":12,\"schema_id\":101,\"table_id\":102}".to_vec(),
    );
    tx.Commit().unwrap();
    let user_ts = storage.CurrentVersion("global").unwrap().Ver;
    let (filtered, _, _, change) = loader.LoadWithTS(user_ts, false).unwrap();
    assert_eq!(filtered.Version, 3);
    assert_eq!(filtered.Databases.len(), 1);
    assert!(change.unwrap().PhyTblIDS.is_empty());
    // RegenerateSchemaMap is a deliberate full-load fallback, preserving models.
    updated.Columns.push(astersql_meta_model::ColumnInfo {
        ID: 2,
        Name: astersql_parser_ast::NewCIStr("job_meta"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    });
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_hash_key(
            format!("DB:{}", db.ID).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        )
        .0,
        astersql_meta_model::EncodeTableInfo(&updated).unwrap(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"4".to_vec(),
    );
    tx.Set(super::canonical_domain::tidb_string_key(b"Diff:4").0, format!("{{\"version\":4,\"type\":5,\"schema_id\":{},\"table_id\":{},\"regenerate_schema_map\":true}}", db.ID, table.ID).into_bytes());
    tx.Commit().unwrap();
    let regen_ts = storage.CurrentVersion("global").unwrap().Ver;
    let (regenerated, _, old, change) = loader.LoadWithTS(regen_ts, false).unwrap();
    assert_eq!(old, 3);
    assert_eq!(regenerated.Version, 4);
    assert!(change.is_none());
    assert_eq!(
        regenerated.Tables[0].Model.as_ref().unwrap().Columns.len(),
        2
    );
    assert_eq!(
        regenerated
            .CompleteInfoSchema()
            .TableByID(table.ID)
            .unwrap()
            .Meta()
            .model_meta
            .as_ref()
            .unwrap()
            .Columns
            .len(),
        2
    );
    // Malformed committed metadata is an error, never an empty schema.
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Set(
        super::canonical_domain::tidb_string_key(b"SchemaVersionKey").0,
        b"5".to_vec(),
    );
    tx.Set(
        super::canonical_domain::tidb_string_key(b"Diff:5").0,
        b"invalid JSON".to_vec(),
    );
    tx.Commit().unwrap();
    let ts4 = storage.CurrentVersion("global").unwrap().Ver;
    assert!(
        loader
            .LoadWithTS(ts4, false)
            .unwrap_err()
            .to_string()
            .contains("decode schema diff 5")
    );
}

#[test]
fn masking_policy_loader_honors_same_version_go_metadata_deletion() {
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let service = super::canonical_domain::DdlMetadataService::new();
    service
        .create_database(storage.as_ref(), "mysql", false)
        .unwrap();
    let change = service
        .create_table(
            storage.as_ref(),
            "mysql",
            astersql_meta_model::TableInfo {
                Name: astersql_parser_ast::NewCIStr("tidb_masking_policy"),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let table = &change.new_tables[0].1;
    assert_ne!(table.DBID, 0);
    let loader = super::canonical_domain::KvInfoSchemaLoader::new();
    let loaded = loader.load_info_schema(storage.as_ref(), "target").unwrap();
    let loaded_table = loaded
        .schema
        .TableByName(
            &infoschema::CiString::new("mysql"),
            &infoschema::CiString::new("tidb_masking_policy"),
        )
        .unwrap();
    assert_eq!(loaded_table.Meta().db_id, table.DBID);
    assert_eq!(
        loaded_table.Meta().model_meta.as_ref().unwrap().DBID,
        table.DBID
    );
    let mut tx = storage.Begin(&[]).unwrap();
    tx.Delete(
        super::canonical_domain::tidb_hash_key(
            format!("DB:{}", table.DBID).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        )
        .0,
    );
    tx.Commit().unwrap();
    let reloaded = loader.load_info_schema(storage.as_ref(), "target").unwrap();
    assert_eq!(
        loaded.schema.SchemaMetaVersion(),
        reloaded.schema.SchemaMetaVersion()
    );
    assert!(
        reloaded
            .schema
            .TableByName(
                &infoschema::CiString::new("mysql"),
                &infoschema::CiString::new("tidb_masking_policy")
            )
            .is_err()
    );
}

#[test]
fn external_workload_manager_binding_isolated_by_storage_owner() {
    use astersql_extworkload::{GetManagerFromStore, SetManagerForStore, SharedManager};
    let first = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    let second = Domain::new(
        TestStorage::new(),
        Arc::new(TestSchemaLoader::new(1)),
        DomainConfig::default(),
    );
    let manager: SharedManager = Arc::new(Mutex::new(Box::new(GoMerge43ExternalManager {
        role: "master".into(),
        updated: Default::default(),
        ttl_events: Default::default(),
    })));
    assert!(GetManagerFromStore(None).is_none());
    assert!(SetManagerForStore(None, None).is_none());
    assert!(GetManagerFromStore(Some(&first)).is_none());
    SetManagerForStore(Some(&first), Some(manager.clone()));
    assert!(Arc::ptr_eq(
        &manager,
        &GetManagerFromStore(Some(&first)).unwrap()
    ));
    assert!(GetManagerFromStore(Some(&second)).is_none());
    SetManagerForStore(Some(&first), None);
    assert!(GetManagerFromStore(Some(&first)).is_none());
    first.close();
    second.close();
}
