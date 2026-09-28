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
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled()),
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

/// 启用缓存时表扫描应置位 ReadFromTableCache；下一条禁用缓存语句应清除该标记。
#[test]
fn cached_table_scan_sets_and_next_statement_resets_stmtctx_marker() {
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

    // 启用表缓存状态执行表扫描，应走缓存读路径。
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
    session.WithSessionVars(|variables| assert!(variables.ReadFromTableCache()));

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
