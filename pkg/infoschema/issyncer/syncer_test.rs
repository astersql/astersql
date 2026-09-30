// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Ported from pkg/infoschema/issyncer/syncer_test.go.

// Syncer 单元测试：覆盖 `skipMDLCheck` 在普通 / 跨 keyspace 下的分支。
//
// MDL（Metadata Lock，元数据锁）检查：跨 KS Syncer 仅关心系统（保留 ID）表，
// 表集合不含保留 ID 时可跳过；普通 Syncer 永不跳过。

use crate::{JobMDL, New, NewCrossKSSyncer, SchemaStore, getFlashbackStartTSFromErrorMsg};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// testStoreWithKS mirrors Go's `testStoreWithKS`, a store stub that only
/// needs to answer `GetKeyspace`; every other `SchemaStore` method keeps the
/// trait's empty default since `skipMDLCheck` never touches the store.
///
/// 仅实现 `GetKeyspace` 的 SchemaStore stub，供跨 KS Syncer 构造。
#[derive(Default)]
struct TestStoreWithKS;
impl crate::SchemaReader for TestStoreWithKS {}
impl SchemaStore for TestStoreWithKS {
    fn GetKeyspace(&self) -> String {
        "test_ks".to_string()
    }
}

/// 将 ID 切片转为 HashSet，便于传给 `skipMDLCheck`。
fn idSet(ids: &[i64]) -> HashSet<i64> {
    ids.iter().copied().collect()
}

/// 对照 Go：普通 Syncer 永不跳过；跨 KS 在含保留表 ID 时不跳过，否则跳过。
#[test]
fn test_syncer_skip_mdl_check() {
    // 普通 Syncer：任意表集合都不跳过 MDL。
    let syncer = New(None, None, 0, None, None, None);
    assert!(!syncer.skipMDLCheck(&idSet(&[])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, 456])));
    assert!(!syncer.skipMDLCheck(&idSet(&[metadef::ReservedGlobalIDUpperBound])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, metadef::ReservedGlobalIDUpperBound])));

    // 跨 KS：无保留 ID 则跳过；含保留 ID 则不跳过。
    let syncer = NewCrossKSSyncer(
        Some(Arc::new(TestStoreWithKS) as Arc<dyn SchemaStore>),
        None,
        0,
        None,
        None,
        "ks1",
    );
    assert!(syncer.skipMDLCheck(&idSet(&[])));
    assert!(syncer.skipMDLCheck(&idSet(&[123])));
    assert!(syncer.skipMDLCheck(&idSet(&[123, 456])));
    assert!(!syncer.skipMDLCheck(&idSet(&[metadef::ReservedGlobalIDUpperBound])));
    assert!(!syncer.skipMDLCheck(&idSet(&[123, metadef::ReservedGlobalIDUpperBound])));

    let mut jobs = HashMap::new();
    jobs.insert(
        1,
        JobMDL {
            Ver: 10,
            TableIDs: idSet(&[123]),
        },
    );
    jobs.insert(
        2,
        JobMDL {
            Ver: 11,
            TableIDs: idSet(&[metadef::ReservedGlobalIDUpperBound]),
        },
    );
    syncer.refreshMDLCheckTableInfoWithJobs(jobs, 11);
    let (version, jobs) = syncer.mdlCheckSnapshot();
    assert_eq!(version, 11);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs.get(&2).unwrap().Ver, 11);
    assert!(syncer.mdlCheckContains(metadef::ReservedGlobalIDUpperBound));
}

/// Go's `strconv.ParseUint` rejects leading/trailing whitespace and the helper
/// only accepts the exact flashback error suffix.
#[test]
fn flashback_start_ts_parser_matches_go() {
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is 18446744073709551615"
        ),
        u64::MAX
    );
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is 123 "
        ),
        0
    );
    assert_eq!(
        getFlashbackStartTSFromErrorMsg(
            "schema is in flashback progress, FlashbackStartTS is  123"
        ),
        0
    );
    assert_eq!(getFlashbackStartTSFromErrorMsg("unrelated error"), 0);
}

#[test]
fn crossks_align_infoschema_reload_publishes_real_go_meta() {
    struct RestoreMDL(bool);
    impl Drop for RestoreMDL {
        fn drop(&mut self) {
            astersql_ddl_schemaver::SetMDLEnabled(self.0);
        }
    }
    let _restore = RestoreMDL(astersql_ddl_schemaver::IsMDLEnabled());
    astersql_ddl_schemaver::SetMDLEnabled(false);
    use astersql_ddl_schemaver::{Context, MemoryEtcdClient, NewEtcdSyncer, Syncer as _};
    use astersql_kv::{Storage, Transaction};
    use astersql_util_codec::{EncodeBytes, EncodeUint};
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let string =
        |name: &[u8]| astersql_kv::Key(EncodeUint(EncodeBytes(vec![b'm'], name), b's' as u64));
    let hash = |name: &[u8], field: &[u8]| {
        astersql_kv::Key(EncodeBytes(
            EncodeUint(EncodeBytes(vec![b'm'], name), b'h' as u64),
            field,
        ))
    };
    let db = astersql_meta_model::DBInfo {
        ID: metadef::SystemDatabaseID,
        Name: astersql_parser_ast::NewCIStr("mysql"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let table = astersql_meta_model::TableInfo {
        ID: metadef::ReservedGlobalIDUpperBound,
        DBID: db.ID,
        Name: astersql_parser_ast::NewCIStr("tidb_ddl_job"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let mut tx = Storage::Begin(storage.as_ref(), &[]).unwrap();
    tx.Set(
        hash(b"DBs", format!("DB:{}", db.ID).as_bytes()),
        astersql_meta_model::EncodeDBInfo(&db).unwrap(),
    )
    .unwrap();
    tx.Set(
        hash(
            format!("DB:{}", db.ID).as_bytes(),
            format!("Table:{}", table.ID).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    tx.Set(string(b"SchemaVersionKey"), b"1".to_vec()).unwrap();
    tx.Set(
        string(b"Diff:1"),
        format!(
            "{{\"version\":1,\"type\":3,\"schema_id\":{},\"table_id\":{}}}",
            db.ID, table.ID
        )
        .into_bytes(),
    )
    .unwrap();
    tx.Commit(&astersql_kv::Context::new()).unwrap();
    let cache = Arc::new(crate::InfoCache::default());
    let etcd = Arc::new(MemoryEtcdClient::default());
    let protocol = NewEtcdSyncer(etcd.clone(), "virtual-target");
    protocol.Init(Context::Background()).unwrap();
    let validator = Arc::new(*astersql_infoschema_isvalidator::new(
        std::time::Duration::from_secs(1),
    ));
    let mut syncer = NewCrossKSSyncer(
        Some(Arc::new(crate::KvSchemaStore::new(storage.clone()))),
        Some(cache.clone()),
        1000,
        None,
        Some(validator.clone()),
        "tenant",
    );
    syncer.InitRequiredFields(Arc::new(|| None), protocol.clone());
    syncer.Reload().unwrap();
    assert_eq!(
        cache.latest().unwrap().Tables[0].Model.as_ref().unwrap().ID,
        table.ID
    );
    let complete = cache.snapshots().GetLatest().unwrap();
    assert_eq!(
        complete
            .ModelTableInfoByName(
                &astersql_infoschema::CiString::new("mysql"),
                &astersql_infoschema::CiString::new("tidb_ddl_job")
            )
            .unwrap()
            .ID,
        table.ID
    );
    let value = astersql_ddl_schemaver::EtcdClient::Get(
        etcd.as_ref(),
        &Context::Background(),
        "/tidb/ddl/all_schema_versions/virtual-target",
        false,
    )
    .unwrap();
    assert_eq!(value.Kvs[0].Value, b"1");
    let expiry = validator.snapshot().latest_schema_expire;
    std::thread::sleep(std::time::Duration::from_millis(2));
    syncer.Reload().unwrap();
    assert!(
        validator.snapshot().latest_schema_expire > expiry,
        "cache hits must renew the real validator lease"
    );
    protocol.Done().Close();
    let context = Context::Background();
    let syncer = Arc::new(syncer);
    let run = syncer.clone();
    let ctx = context.clone();
    let worker = std::thread::spawn(move || run.SyncLoop(ctx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while validator.snapshot().restart_schema_ver != 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "schema lease did not recover"
        );
        std::thread::yield_now();
    }
    assert!(validator.snapshot().is_started);
    assert!(!protocol.Done().Done());
    context.Cancel();
    worker.join().unwrap().unwrap();
    protocol.Close();
    // A schema version without its committed diff is deliberately excluded
    // from loads, but must prevent renewing the older schema's lease.
    let mut pending = Storage::Begin(storage.as_ref(), &[]).unwrap();
    pending
        .Set(string(b"SchemaVersionKey"), b"2".to_vec())
        .unwrap();
    pending.Commit(&astersql_kv::Context::new()).unwrap();
    let timestamp = Storage::CurrentVersion(storage.as_ref(), "global")
        .unwrap()
        .Ver;
    assert_eq!(syncer.loader.schema_version_at(timestamp).unwrap(), 2);
    assert_eq!(
        crate::SchemaReader::MaxDiffVersion(&crate::KvSchemaStore::new(storage)).unwrap(),
        1
    );
}
