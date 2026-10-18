// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 缓存表（Cached Table）运行时行为的集成测试。
//
// 缓存表将整表数据缓存在 TiDB 节点内存中以加速读；本测试验证
// `TableCacheStatusEnable` 下语句标记 `ReadFromTableCache`，
// 下一条禁用缓存的语句应清除该标记。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_domain::{InfoSchemaLoader, LoadedInfoSchema};
use astersql_executor_sortexec::{Row, SortValue};
use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};

use crate::runtime::{ConcreteSession, ConcreteTestRuntime, RuntimeDomain};
use crate::testutil::TestRuntime;
use crate::{SessionError, SessionResult};

/// 测试用 InfoSchema 加载器，按原子版本号构造 Mock schema。
struct CachedTableSchemaLoader {
    version: AtomicI64,
}

impl CachedTableSchemaLoader {
    /// 按给定时间戳构造已加载的 InfoSchema（元数据快照）。
    fn schema(&self, timestamp: u64) -> LoadedInfoSchema {
        let schema: SchemaRef = infoschema::infoschema::MockInfoSchemaWithSchemaVer(
            Vec::new(),
            self.version.load(Ordering::Acquire),
        );
        LoadedInfoSchema::new(schema, timestamp)
    }
}

impl InfoSchemaLoader for CachedTableSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(self.schema(10))
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(self.schema(timestamp))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(keyspace == "SYSTEM")
    }
}

/// 创建内存 Mock Storage，供 BootstrapSession 使用。
fn new_storage() -> SessionResult<mockStorage> {
    Arc::try_unwrap(
        NewMockStorage(KVStore::NewMemory(), None)
            .map_err(|error| SessionError::new(error.to_string()))?,
    )
    .map_err(|_| SessionError::new("mock storage retained an unexpected owner"))
}

/// Bootstrap 带缓存表 schema loader 的测试会话。
fn concrete_session() -> ConcreteSession {
    let runtime = ConcreteTestRuntime::new(
        new_storage,
        Arc::new(CachedTableSchemaLoader {
            version: AtomicI64::new(1),
        }),
        false,
    );
    let store = runtime
        .NewMockStore()
        .expect("create cached-table mock store");
    let domain = runtime
        .BootstrapSession(store)
        .expect("bootstrap cached-table domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("cached-table runtime domain");
    ConcreteSession::new(Arc::clone(domain.domain()))
}

/// 构造含单列 `a`、指定缓存状态的 Mock InfoSchema。
fn cached_table_info_schema(
    status: astersql_meta_model::TableCacheStatusType,
) -> Arc<dyn infoschema::infoschema::InfoSchema> {
    let mut field_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    field_type.SetCharset("binary".to_owned());
    field_type.SetCollate("binary".to_owned());
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 108,
        Name: astersql_parser_ast::NewCIStr("cached_t"),
        Charset: "binary".to_owned(),
        Collate: "binary".to_owned(),
        Columns: vec![astersql_meta_model::ColumnInfo {
            ID: 1,
            Name: astersql_parser_ast::NewCIStr("a"),
            Offset: 0,
            State: astersql_meta_model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }],
        TableCacheStatusType: status,
        ..Default::default()
    });
    infoschema::infoschema::MockInfoSchema(vec![infoschema::infoschema::TableInfo {
        id: model.ID,
        name: infoschema::infoschema::CiString::new("cached_t"),
        columns: vec![infoschema::infoschema::ColumnInfo {
            id: 1,
            name: infoschema::infoschema::CiString::new("a"),
            ..Default::default()
        }],
        model_meta: Some(model),
        ..Default::default()
    }])
}

/// 编码缓存表一行的 KV 键值对（表 ID 108）。
fn cached_table_row(handle: i64, value: i64) -> (kv::Key, Vec<u8>) {
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        108,
        Box::new(astersql_tablecodec::kv::IntHandle(handle)),
    );
    let value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![astersql_tablecodec::types::NewIntDatum(value)],
        vec![1],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .expect("encode cached-table row");
    (kv::Key(key.0), value)
}

/// A raw snapshot scan must not claim a cache hit merely from table metadata.
#[test]
fn cached_table_metadata_does_not_mark_plain_snapshot_scans() {
    let session = concrete_session();
    // 向 Mock Storage 写入两行种子数据。
    let mut transaction = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin cached-table seed transaction");
    for (handle, value) in [(1, 7), (2, 11)] {
        let (key, value) = cached_table_row(handle, value);
        transaction.Set(key, value).expect("seed cached-table row");
    }
    transaction
        .Commit(&kv::Context::default())
        .expect("commit cached-table rows");
    let snapshot = session.domain().storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        storage.GetSnapshot(version)
    });

    // Cache metadata alone does not replace the supplied raw KV retriever.
    let result = session
        .ExecutePlannedKVSelect(
            "select a from cached_t",
            cached_table_info_schema(astersql_meta_model::TableCacheStatusEnable),
            snapshot.as_ref(),
        )
        .expect("execute cached-table scan");
    assert_eq!(
        result.Rows,
        vec![Row(vec![SortValue::Int(7)]), Row(vec![SortValue::Int(11)])]
    );
    assert_eq!(result.ScannedRows, 2);
    session.WithSessionVars(|variables| assert!(!variables.ReadFromTableCache()));

    // 下一条语句禁用缓存后，StmtCtx 标记应被重置。
    let ordinary = session
        .ExecutePlannedKVSelect(
            "select a from cached_t",
            cached_table_info_schema(astersql_meta_model::TableCacheStatusDisable),
            snapshot.as_ref(),
        )
        .expect("execute next non-cached statement");
    assert_eq!(ordinary.Rows, result.Rows);
    assert_eq!(ordinary.ScannedRows, 2);
    session.WithSessionVars(|variables| assert!(!variables.ReadFromTableCache()));
}

/// Observe statement markers only after the canonical result set has closed,
/// including the empty-table case and both executions of a prepared SELECT.
#[test]
fn cached_table_marker_matches_real_session_for_empty_and_prepared_reads() {
    let session = canonical_cached_table_session();
    let query = |sql: &str| {
        let mut rows = Vec::new();
        for mut result in session.execute(sql).unwrap() {
            while let Some(row) = result.next_row().unwrap() {
                rows.push(row);
            }
            result.close().unwrap();
        }
        rows
    };
    for (insert, expected) in [(false, vec![]), (true, vec![vec!["7".to_owned()]])] {
        query("DROP TABLE IF EXISTS test.cached_marker");
        query("CREATE TABLE test.cached_marker (a INT)");
        if insert {
            query("INSERT INTO test.cached_marker VALUES (7)");
        }
        query("ALTER TABLE test.cached_marker CACHE");
        let mut hit = false;
        for _ in 0..50 {
            assert_eq!(
                query("SELECT * FROM test.cached_marker WHERE a = 7"),
                expected
            );
            hit = session.WithSessionVars(|vars| vars.ReadFromTableCache());
            if hit {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            hit,
            "canonical cached read marker missing (nonempty={insert})"
        );
        let table = session
            .domain()
            .table_by_name("test", "cached_marker")
            .unwrap();
        let buffer = session
            .CachedTableBufferForTest(table.ID)
            .expect("loaded real KV buffer");
        assert_eq!(buffer.is_empty(), !insert);
        let reused = session.CachedTableBufferForTest(table.ID).unwrap();
        assert!(
            Arc::ptr_eq(&buffer, &reused),
            "reads must reuse the loaded buffer"
        );
        query("SELECT 1");
        session.WithSessionVars(|vars| assert!(!vars.ReadFromTableCache()));
        query("SET tidb_enable_prepared_plan_cache=ON");
        query("PREPARE cached_marker_stmt FROM 'SELECT * FROM test.cached_marker WHERE a = ?'");
        query("SET @marker_a=7");
        for expected_hit in [false, true] {
            assert_eq!(
                query("EXECUTE cached_marker_stmt USING @marker_a"),
                expected
            );
            session.WithSessionVars(|vars| assert!(vars.ReadFromTableCache()));
            assert_eq!(
                query("SELECT @@last_plan_from_cache"),
                vec![vec![if expected_hit { "1" } else { "0" }.to_owned()]]
            );
            session.WithSessionVars(|vars| assert!(!vars.ReadFromTableCache()));
        }
        query("DEALLOCATE PREPARE cached_marker_stmt");
    }
    query("ALTER TABLE test.cached_marker NOCACHE");
    assert_eq!(
        query("SELECT * FROM test.cached_marker"),
        vec![vec!["7".to_owned()]]
    );
    session.WithSessionVars(|vars| assert!(!vars.ReadFromTableCache()));
}

fn canonical_cached_table_session() -> ConcreteSession {
    fn storage() -> SessionResult<mockStorage> {
        Arc::try_unwrap(
            NewMockStorage(KVStore::NewMemoryWithWallClockTSO(), None)
                .map_err(|e| SessionError::new(e.to_string()))?,
        )
        .map_err(|_| SessionError::new("unexpected cached-table storage owner"))
    }
    let runtime = ConcreteTestRuntime::new(
        storage,
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        false,
    );
    let store = runtime.NewMockStore().unwrap();
    let domain = runtime.BootstrapSession(store).unwrap();
    let domain = domain.as_any().downcast_ref::<RuntimeDomain>().unwrap();
    crate::runtime::EnableMockDDLForTest(domain.domain()).unwrap();
    ConcreteSession::new(Arc::clone(domain.domain()))
}

#[test]
fn cached_table_scan_sets_and_next_statement_resets_stmtctx_marker() {
    let session = canonical_cached_table_session();
    session
        .execute("CREATE TABLE test.cache_reset (a INT)")
        .unwrap();
    session
        .execute("INSERT INTO test.cache_reset VALUES (7),(11)")
        .unwrap();
    session
        .execute("ALTER TABLE test.cache_reset CACHE")
        .unwrap();
    for attempt in 0..50 {
        let mut result = session
            .execute("SELECT a FROM test.cache_reset ORDER BY a")
            .unwrap()
            .remove(0);
        assert_eq!(result.next_row().unwrap().unwrap(), vec!["7"]);
        assert_eq!(result.next_row().unwrap().unwrap(), vec!["11"]);
        assert!(result.next_row().unwrap().is_none());
        result.close().unwrap();
        if session.WithSessionVars(|v| v.ReadFromTableCache()) {
            break;
        }
        assert!(attempt < 49, "real cached scan must eventually hit");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    session.execute("SELECT 1").unwrap();
    session.WithSessionVars(|v| assert!(!v.ReadFromTableCache()));
}

fn cache_query(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for mut result in session.execute(sql).unwrap() {
        while let Some(row) = result.next_row().unwrap() {
            rows.push(row);
        }
        result.close().unwrap();
    }
    rows
}

fn warm_cached_table(session: &ConcreteSession, sql: &str, expected: &[Vec<String>]) {
    for attempt in 0..50 {
        assert_eq!(cache_query(session, sql), expected);
        if session.WithSessionVars(|v| v.ReadFromTableCache()) {
            return;
        }
        assert!(attempt < 49, "cache must load real table KV");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn cached_table_write_commit_observes_read_lease_and_new_data() {
    let session = canonical_cached_table_session();
    cache_query(
        &session,
        "CREATE TABLE test.cache_write (a INT PRIMARY KEY)",
    );
    cache_query(&session, "INSERT INTO test.cache_write VALUES (1)");
    cache_query(&session, "ALTER TABLE test.cache_write CACHE");
    warm_cached_table(
        &session,
        "SELECT * FROM test.cache_write",
        &[vec!["1".into()]],
    );
    let writer = ConcreteSession::new(session.domain().clone());
    cache_query(&writer, "BEGIN");
    cache_query(&writer, "INSERT INTO test.cache_write VALUES (2)");
    // A reader retains the old committed buffer until the writer's commit.
    assert_eq!(
        cache_query(&session, "SELECT * FROM test.cache_write"),
        vec![vec!["1"]]
    );
    cache_query(&writer, "COMMIT");
    assert_eq!(
        cache_query(&session, "SELECT * FROM test.cache_write ORDER BY a"),
        vec![vec!["1"], vec!["2"]]
    );
    session.WithSessionVars(|v| {
        assert!(
            !v.ReadFromTableCache(),
            "the old read lease must not cover the new commit"
        )
    });
}

#[test]
fn cached_table_reenable_reloads_instead_of_reusing_old_schema_buffer() {
    let session = canonical_cached_table_session();
    cache_query(&session, "CREATE TABLE test.cache_reenable (a INT)");
    cache_query(&session, "INSERT INTO test.cache_reenable VALUES (1)");
    cache_query(&session, "ALTER TABLE test.cache_reenable CACHE");
    warm_cached_table(
        &session,
        "SELECT * FROM test.cache_reenable",
        &[vec!["1".into()]],
    );
    let id = session
        .domain()
        .table_by_name("test", "cache_reenable")
        .unwrap()
        .ID;
    let old = session.CachedTableBufferForTest(id).unwrap();
    cache_query(&session, "ALTER TABLE test.cache_reenable NOCACHE");
    cache_query(&session, "INSERT INTO test.cache_reenable VALUES (2)");
    cache_query(&session, "ALTER TABLE test.cache_reenable CACHE");
    warm_cached_table(
        &session,
        "SELECT * FROM test.cache_reenable ORDER BY a",
        &[vec!["1".into()], vec!["2".into()]],
    );
    let new = session.CachedTableBufferForTest(id).unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
}

#[test]
fn cached_table_cold_prepared_read_does_not_claim_a_buffer_hit() {
    let session = canonical_cached_table_session();
    cache_query(&session, "CREATE TABLE test.cache_cold (a INT)");
    cache_query(&session, "INSERT INTO test.cache_cold VALUES (7)");
    cache_query(&session, "ALTER TABLE test.cache_cold CACHE");
    cache_query(
        &session,
        "PREPARE cold FROM 'SELECT * FROM test.cache_cold WHERE a = ?'",
    );
    cache_query(&session, "SET @a=7");
    assert_eq!(
        cache_query(&session, "EXECUTE cold USING @a"),
        vec![vec!["7"]]
    );
    session.WithSessionVars(|v| {
        assert!(
            !v.ReadFromTableCache(),
            "metadata and a scheduled load are not cache hits"
        )
    });
    warm_cached_table(&session, "EXECUTE cold USING @a", &[vec!["7".into()]]);
    assert_eq!(
        cache_query(&session, "SELECT @@last_plan_from_cache"),
        vec![vec!["1"]]
    );
}

#[test]
fn cached_table_ddl_uses_both_persistent_schema_steps_and_is_idempotent() {
    let session = canonical_cached_table_session();
    cache_query(&session, astersql_meta_metadef::CreateTableCacheMetaTable);
    cache_query(&session, "CREATE TABLE test.cache_steps (a INT)");
    for (sql, target) in [
        (
            "ALTER TABLE test.cache_steps CACHE",
            astersql_meta_model::TableCacheStatusEnable,
        ),
        (
            "ALTER TABLE test.cache_steps NOCACHE",
            astersql_meta_model::TableCacheStatusDisable,
        ),
    ] {
        let before = session.domain().info_schema().SchemaMetaVersion();
        cache_query(&session, sql);
        let after = session.domain().info_schema().SchemaMetaVersion();
        assert_eq!(
            after,
            before + 2,
            "switching and final state each need a schema barrier"
        );
        assert_eq!(
            session
                .domain()
                .table_by_name("test", "cache_steps")
                .unwrap()
                .TableCacheStatusType,
            target
        );
        cache_query(&session, sql);
        assert_eq!(session.domain().info_schema().SchemaMetaVersion(), after);
    }
    cache_query(
        &session,
        "CREATE TABLE test.cache_partition (a INT) PARTITION BY HASH(a) PARTITIONS 2",
    );
    assert!(
        session
            .execute("ALTER TABLE test.cache_partition CACHE")
            .is_err()
    );
    assert!(
        session
            .execute("ALTER TABLE mysql.table_cache_meta CACHE")
            .is_err()
    );
}
