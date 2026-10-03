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

// 具体会话运行时（ConcreteSession）集成测试。
//
// 覆盖统计信息同步加载、解析/预编译/提交/回滚、慢日志 hint、
// 多语句 binding，以及严格计划+KV 执行流水线。

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use astersql_domain::{InfoSchemaLoader, LoadedInfoSchema};
use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};
use astersql_util_logutil::log::{LogLevel, Logger};

use crate::runtime::SessionStatsSyncLoadAdapter;
use crate::runtime::{
    CanonicalSessionFactory, ConcreteSession, ConcreteTestRuntime, ParseDateTimeMicrosForTest,
    RuntimeDomain, SplitSQLStatements, count_relational_rows,
    decode_relational_checksum_count_response, decode_relational_count_response,
    integer_handle_offset_candidate, is_scalar_count_non_null_constant, relational_count_checksum,
    relational_count_dag, relational_primary_key_scan_ranges, scan_relational_rows_window_key_only,
    scan_relational_rows_window_key_only_from,
};
use crate::testutil::{TestRecordSet, TestRuntime};
use crate::{SessionError, SessionResult};

/// 测试用 InfoSchema 加载器，按原子版本号构造 mock schema。
struct TestSchemaLoader {
    version: AtomicI64,
}

impl TestSchemaLoader {
    fn schema(&self, timestamp: u64) -> LoadedInfoSchema {
        let schema: SchemaRef = infoschema::infoschema::MockInfoSchemaWithSchemaVer(
            Vec::new(),
            self.version.load(Ordering::Acquire),
        );
        LoadedInfoSchema::new(schema, timestamp)
    }
}

impl InfoSchemaLoader for TestSchemaLoader {
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

/// 创建内存 mock 存储。
fn new_storage() -> SessionResult<mockStorage> {
    Arc::try_unwrap(
        NewMockStorage(KVStore::NewMemory(), None)
            .map_err(|error| SessionError::new(error.to_string()))?,
    )
    .map_err(|_| SessionError::new("mock storage retained an unexpected owner"))
}

struct KeyOnlyCountIterator {
    remaining: usize,
    closed: Arc<AtomicBool>,
}

impl kv::Iterator for KeyOnlyCountIterator {
    fn Valid(&self) -> bool {
        self.remaining != 0
    }

    fn Key(&self) -> kv::Key {
        kv::Key::default()
    }

    fn Value(&self) -> Vec<u8> {
        panic!("COUNT(*) key path must not read row values")
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        self.remaining -= 1;
        Ok(())
    }

    fn Close(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct KeyOnlyCountRetriever {
    rows: usize,
    closed: Arc<AtomicBool>,
}

impl kv::Getter for KeyOnlyCountRetriever {
    fn Get(
        &self,
        _ctx: &kv::Context,
        _key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        panic!("COUNT(*) key path must not issue point gets")
    }
}

impl kv::Retriever for KeyOnlyCountRetriever {
    fn Iter(
        &self,
        _key: kv::Key,
        _upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Ok(Box::new(KeyOnlyCountIterator {
            remaining: self.rows,
            closed: Arc::clone(&self.closed),
        }))
    }

    fn IterReverse(
        &self,
        _key: Option<kv::Key>,
        _lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        panic!("COUNT(*) key path must scan forward")
    }
}

struct OffsetWindowIterator {
    rows: Vec<(kv::Key, Vec<u8>)>,
    position: usize,
    key_only: bool,
}

impl kv::Iterator for OffsetWindowIterator {
    fn Valid(&self) -> bool {
        self.position < self.rows.len()
    }

    fn Key(&self) -> kv::Key {
        self.rows[self.position].0.clone()
    }

    fn Value(&self) -> Vec<u8> {
        assert!(!self.key_only, "OFFSET locator must not read row values");
        self.rows[self.position].1.clone()
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        self.position += 1;
        Ok(())
    }

    fn Close(&mut self) {
        self.position = self.rows.len();
    }
}

struct OffsetWindowSnapshot {
    rows: Vec<(kv::Key, Vec<u8>)>,
    key_only: bool,
    option_history: Vec<bool>,
    iterator_modes: Mutex<Vec<bool>>,
}

impl kv::Getter for OffsetWindowSnapshot {
    fn Get(
        &self,
        _ctx: &kv::Context,
        _key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        panic!("OFFSET window scan must not issue point gets")
    }
}

impl kv::Retriever for OffsetWindowSnapshot {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        self.iterator_modes.lock().unwrap().push(self.key_only);
        Ok(Box::new(OffsetWindowIterator {
            rows: self
                .rows
                .iter()
                .filter(|(candidate, _)| {
                    candidate.as_ref() >= key.as_ref()
                        && upper_bound
                            .as_ref()
                            .is_none_or(|upper| candidate.as_ref() < upper.as_ref())
                })
                .cloned()
                .collect(),
            position: 0,
            key_only: self.key_only,
        }))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        self.iterator_modes.lock().unwrap().push(self.key_only);
        Ok(Box::new(OffsetWindowIterator {
            rows: self
                .rows
                .iter()
                .filter(|(candidate, _)| {
                    key.as_ref()
                        .is_none_or(|upper| candidate.as_ref() < upper.as_ref())
                        && lower_bound
                            .as_ref()
                            .is_none_or(|lower| candidate.as_ref() >= lower.as_ref())
                })
                .rev()
                .cloned()
                .collect(),
            position: 0,
            key_only: self.key_only,
        }))
    }
}

impl kv::Snapshot for OffsetWindowSnapshot {
    fn BatchGet(
        &self,
        _ctx: &kv::Context,
        _keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        panic!("OFFSET window scan must not issue batch gets")
    }

    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        if option != kv::KeyOnly {
            return;
        }
        self.key_only = value
            .as_deref()
            .and_then(|value| value.downcast_ref::<bool>())
            .copied()
            .unwrap_or(false);
        self.option_history.push(self.key_only);
    }
}

fn runtime() -> ConcreteTestRuntime<mockStorage, impl Fn() -> SessionResult<mockStorage>> {
    ConcreteTestRuntime::new(
        new_storage,
        Arc::new(TestSchemaLoader {
            version: AtomicI64::new(1),
        }),
        false,
    )
}

pub(crate) fn concrete_session() -> ConcreteSession {
    let runtime = runtime();
    let store = runtime.NewMockStore().expect("create canonical mock store");
    let domain = runtime
        .BootstrapSession(store)
        .expect("bootstrap canonical domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("runtime domain");
    ConcreteSession::new(Arc::clone(domain.domain()))
}

#[test]
fn datetime_parser_rejects_nonexistent_calendar_dates() {
    for value in [
        "2025-02-29 00:00:00",
        "2024-02-30 00:00:00",
        "2025-04-31 00:00:00",
    ] {
        assert!(
            ParseDateTimeMicrosForTest(value).is_err(),
            "Go datetime semantics reject {value}"
        );
    }
    assert!(ParseDateTimeMicrosForTest("2024-02-29 23:59:59.123456").is_ok());
}

#[path = "runtime_test/ddl.rs"]
mod ddl;
#[path = "runtime_test/planning.rs"]
mod planning;
#[path = "runtime_test/query.rs"]
mod query;
#[path = "runtime_test/session.rs"]
mod session;
#[path = "runtime_test/statistics.rs"]
mod statistics;
#[path = "runtime_test/storage.rs"]
mod storage;
#[path = "runtime_test/typed_adapter_bridge.rs"]
mod typed_adapter_bridge;

#[test]
fn alter_database_emits_crucial_operation_without_general_log() {
    use astersql_util_logutil::log::{LogField, background_logger};
    let (_domain, mut session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session.configure_connection(190019, 0, 46).unwrap();
    session.execute("CREATE DATABASE lifecycle_audit").unwrap();
    session.execute("USE lifecycle_audit").unwrap();
    let logger = background_logger();
    use astersql_sessionctx_vardef::ProcessGeneralLog;
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ProcessGeneralLog.Store(self.0);
        }
    }
    let _restore = Restore(ProcessGeneralLog.Load());
    ProcessGeneralLog.Store(false);
    for sql in [
        "ALTER DATABASE lifecycle_audit CHARACTER SET utf8mb4",
        "ALTER DATABASE nonexistent_lifecycle_audit CHARACTER SET utf8mb4",
    ] {
        ProcessGeneralLog.Store(sql.contains("nonexistent_lifecycle_audit"));
        let general = astersql_util_logutil::log::general_logger();
        let result = session.execute(sql);
        assert!(!general.entries().iter().any(|entry| {
            entry.message == "GENERAL_LOG"
                && entry
                    .fields
                    .contains(&LogField::String("sql".into(), sql.into()))
        }));
        if sql.contains("nonexistent_lifecycle_audit") {
            assert!(result.is_err());
        } else {
            result.unwrap();
        }
        let entries = logger.entries();
        let entry = entries
            .iter()
            .find(|entry| {
                entry.message == "CRUCIAL OPERATION"
                    && entry
                        .fields
                        .contains(&LogField::String("sql".into(), sql.into()))
            })
            .expect("ALTER DATABASE audit on success and failure");
        assert_eq!(entry.level, LogLevel::Info);
        assert!(entry.fields.contains(&LogField::U64("conn".into(), 190019)));
        assert!(
            entry
                .fields
                .contains(&LogField::String("cur_db".into(), "lifecycle_audit".into()))
        );
        assert!(
            entry
                .fields
                .iter()
                .any(|field| matches!(field, LogField::I64(key, _) if key == "schemaVersion"))
        );
        assert!(
            entry
                .fields
                .contains(&LogField::String("user".into(), String::new()))
        );
    }
}
