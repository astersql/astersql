// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::infoschema_reader::{
    ColumnInfo, DataRequest, Datum, DeadlockRecord, InfoResult, InfoSchemaDataSource,
    InfoSchemaSnapshot, PredicateExtractor, Row, SessionState, TableInfo, TiFlashInstance,
    initialTable, memtableRetriever,
};

struct InternalTestSource {
    requests: Mutex<Vec<DataRequest>>,
    stats_requests: Mutex<Vec<(Vec<i64>, bool)>>,
    attr_viewer: Option<String>,
    attr_privileges: Option<astersql_privilege_privileges::UserPrivileges>,
    attr_rows: Vec<Row>,
    attr_error: bool,
}

impl InternalTestSource {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            stats_requests: Mutex::new(Vec::new()),
            attr_viewer: None,
            attr_privileges: None,
            attr_rows: Vec::new(),
            attr_error: false,
        }
    }

    fn requests(&self) -> Vec<DataRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl InfoSchemaDataSource for InternalTestSource {
    fn user_attributes_privileges(
        &self,
    ) -> Option<(
        astersql_privilege_privileges::UserPrivileges,
        Vec<astersql_privilege_privileges::RoleIdentity>,
    )> {
        self.attr_privileges
            .clone()
            .map(|manager| (manager, Vec::new()))
    }
    fn session_state(&self) -> InfoResult<SessionState> {
        Ok(SessionState {
            in_transaction: false,
            transaction_start_ts: 0,
            snapshot_ts: 0,
            current_user: self
                .attr_viewer
                .clone()
                .unwrap_or_else(|| "root".to_owned()),
            current_host: "localhost".to_owned(),
        })
    }

    fn snapshot_info_schema(&self, timestamp: u64) -> InfoResult<InfoSchemaSnapshot> {
        Ok(InfoSchemaSnapshot { version: timestamp })
    }

    fn latest_info_schema(&self) -> InfoResult<InfoSchemaSnapshot> {
        Ok(InfoSchemaSnapshot { version: 1 })
    }

    fn transaction_info_schema(&self) -> InfoResult<InfoSchemaSnapshot> {
        Ok(InfoSchemaSnapshot { version: 1 })
    }

    fn load_rows(
        &self,
        request: DataRequest,
        _: Option<&InfoSchemaSnapshot>,
        _: Option<&PredicateExtractor>,
    ) -> InfoResult<Vec<Row>> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(match request {
            DataRequest::UserAttributes => {
                if self.attr_error {
                    return Err(crate::infoschema_reader::InfoSchemaError::new(
                        "restricted SQL failed",
                    ));
                }
                self.attr_rows.clone()
            }
            DataRequest::CheckConstraints {
                tidb_extended: false,
            } => vec![vec![
                text("def"),
                text("test"),
                text("t2_c1"),
                text("(id<10)"),
            ]],
            DataRequest::CheckConstraints {
                tidb_extended: true,
            } => vec![vec![
                text("def"),
                text("test"),
                text("t2_c1"),
                text("(id<10)"),
                text("t2"),
                Datum::Int(2),
            ]],
            DataRequest::Keywords => vec![vec![text("ADD"), Datum::Int(1)]],
            DataRequest::Tables { .. } | DataRequest::Partitions { .. } => Vec::new(),
            other => panic!("unexpected information-schema request: {other:?}"),
        })
    }

    fn privilege_verification(&self, _: &str, _: &str, _: &str) -> InfoResult<Option<bool>> {
        Ok(Some(true))
    }

    fn auto_increment_id(&self, _: &InfoSchemaSnapshot, _: i64) -> InfoResult<Option<i64>> {
        Ok(None)
    }

    fn update_stats_cache(&self, table_ids: &[i64], need_column_lengths: bool) -> InfoResult {
        self.stats_requests
            .lock()
            .unwrap()
            .push((table_ids.to_vec(), need_column_lengths));
        Ok(())
    }

    fn ddl_jobs_open(&self, _: &InfoSchemaSnapshot) -> InfoResult<u64> {
        Ok(0)
    }

    fn ddl_jobs_next(&self, _: u64, _: usize) -> InfoResult<Vec<Row>> {
        Ok(Vec::new())
    }

    fn ddl_jobs_close(&self, _: u64) -> InfoResult {
        Ok(())
    }

    fn initial_tables(&self, _: &PredicateExtractor) -> InfoResult<Vec<initialTable>> {
        Ok(Vec::new())
    }

    fn transaction_row_count(&self) -> InfoResult<usize> {
        Ok(0)
    }

    fn data_lock_wait_count(&self) -> InfoResult<usize> {
        Ok(0)
    }

    fn deadlock_records(&self) -> InfoResult<Vec<DeadlockRecord>> {
        Ok(Vec::new())
    }

    fn tiflash_instances(&self, _: &BTreeSet<String>) -> InfoResult<Vec<TiFlashInstance>> {
        Ok(Vec::new())
    }

    fn analyze_total_count(&self, _: &str, _: &str, _: &str) -> InfoResult<f64> {
        Ok(0.0)
    }

    fn decode_table_id_from_start_key(&self, _: &[u8]) -> InfoResult<i64> {
        Ok(0)
    }

    fn table_matches_id(&self, _: &str, _: &str, _: &str, _: i64) -> InfoResult<bool> {
        Ok(false)
    }
}

fn text(value: &str) -> Datum {
    Datum::Text(value.to_owned())
}

fn retriever(source: Arc<InternalTestSource>) -> memtableRetriever {
    memtableRetriever {
        table: TableInfo {
            id: 1,
            schema: "information_schema".to_owned(),
            name: String::new(),
            columns: Vec::new(),
            partition_ids: Vec::new(),
        },
        columns: Vec::<ColumnInfo>::new(),
        rows: Vec::new(),
        row_idx: 0,
        retrieved: false,
        initialized: false,
        extractor: PredicateExtractor::default(),
        info_schema: Some(InfoSchemaSnapshot { version: 1 }),
        mem_tracker: None,
        accumulated_memory_per_batch: 0,
        accumulated_memory_record_count: 0,
        source,
    }
}

#[test]
fn table_rows_only_skips_column_length_read() {
    let source = Arc::new(InternalTestSource::new());
    let mut reader = retriever(Arc::clone(&source));
    let tables = [TableInfo {
        id: 10,
        schema: "test".to_owned(),
        name: "t".to_owned(),
        columns: Vec::new(),
        partition_ids: vec![11, 12],
    }];

    reader.columns = vec![ColumnInfo {
        name: "TABLE_ROWS".to_owned(),
        offset: 0,
    }];
    reader.updateStatsCacheIfNeed(&tables).unwrap();
    assert_eq!(
        *source.stats_requests.lock().unwrap(),
        vec![(vec![11, 12, 10], false)]
    );

    source.stats_requests.lock().unwrap().clear();
    reader.columns = vec![ColumnInfo {
        name: "DATA_LENGTH".to_owned(),
        offset: 0,
    }];
    reader.updateStatsCacheIfNeed(&tables).unwrap();
    assert_eq!(
        *source.stats_requests.lock().unwrap(),
        vec![(vec![11, 12, 10], true)]
    );

    source.stats_requests.lock().unwrap().clear();
    reader.columns = vec![ColumnInfo {
        name: "TABLE_NAME".to_owned(),
        offset: 0,
    }];
    reader.updateStatsCacheIfNeed(&tables).unwrap();
    assert!(source.stats_requests.lock().unwrap().is_empty());
}

#[test]
fn table_and_partition_requests_preserve_stats_column_requirements() {
    let source = Arc::new(InternalTestSource::new());
    let mut reader = retriever(Arc::clone(&source));
    reader.columns = vec![ColumnInfo {
        name: "TABLE_ROWS".to_owned(),
        offset: 0,
    }];
    reader.setDataFromTables().unwrap();
    reader.setDataFromPartitions().unwrap();
    assert_eq!(
        source.requests(),
        vec![
            DataRequest::Tables {
                need_row_count: true,
                need_column_lengths: false,
            },
            DataRequest::Partitions {
                need_row_count: true,
                need_column_lengths: false,
            },
        ]
    );
}

#[test]
fn set_data_from_check_constraints_matches_go_contract() {
    let source = Arc::new(InternalTestSource::new());
    let mut reader = retriever(Arc::clone(&source));

    reader.setDataFromCheckConstraints().unwrap();

    assert_eq!(reader.rows.len(), 1);
    assert_eq!(reader.rows[0].len(), 4);
    assert_eq!(
        reader.rows[0],
        vec![text("def"), text("test"), text("t2_c1"), text("(id<10)")]
    );
    assert_eq!(
        source.requests(),
        vec![DataRequest::CheckConstraints {
            tidb_extended: false
        }]
    );
}

#[test]
fn set_data_from_tidb_check_constraints_matches_go_contract() {
    let source = Arc::new(InternalTestSource::new());
    let mut reader = retriever(Arc::clone(&source));

    reader.setDataFromTiDBCheckConstraints().unwrap();

    assert_eq!(reader.rows.len(), 1);
    assert_eq!(reader.rows[0].len(), 6);
    assert_eq!(
        reader.rows[0],
        vec![
            text("def"),
            text("test"),
            text("t2_c1"),
            text("(id<10)"),
            text("t2"),
            Datum::Int(2),
        ]
    );
    assert_eq!(
        source.requests(),
        vec![DataRequest::CheckConstraints {
            tidb_extended: true
        }]
    );
}

#[test]
fn set_data_from_keywords_matches_go_contract() {
    let source = Arc::new(InternalTestSource::new());
    let mut reader = retriever(Arc::clone(&source));

    reader.setDataFromKeywords().unwrap();

    assert_eq!(reader.rows[0], vec![text("ADD"), Datum::Int(1)]);
    assert_eq!(source.requests(), vec![DataRequest::Keywords]);
}

#[test]
fn user_attributes_visibility_follows_mysql_privileges() {
    use astersql_privilege_privileges::*;
    let names = [
        "root",
        "uacreateonly",
        "uanobody",
        "uaroot",
        "uaselectonmysql",
        "uaselectonmysqluser",
        "uasystemholder",
        "uavictim",
    ];
    let mut cache = MySQLPrivilege::default();
    for name in names {
        let mut record = NewUserRecord("%", name);
        record.Privileges = match name {
            "root" => SelectPriv | SuperPriv | CreateUserPriv,
            "uaroot" => SuperPriv,
            "uacreateonly" => CreateUserPriv,
            _ => 0,
        };
        cache.user.push(record);
    }
    cache.db.push(dbRecord {
        base: baseRecord::new("%", "uaselectonmysql"),
        DB: "mysql".into(),
        Privileges: SelectPriv,
    });
    cache.tables_priv.push(tablesPrivRecord {
        base: baseRecord::new("%", "uaselectonmysqluser"),
        DB: "mysql".into(),
        TableName: "user".into(),
        TablePriv: SelectPriv,
        ..Default::default()
    });
    cache.dynamic_priv.push(dynamicPrivRecord {
        base: baseRecord::new("%", "uasystemholder"),
        PrivilegeName: "SYSTEM_USER".into(),
        ..Default::default()
    });
    cache.SortUserTable();
    let handle = Handle::New();
    handle.merge(cache);
    for (viewer, expected) in [
        ("uanobody", vec!["uanobody"]),
        ("uaroot", vec!["uaroot"]),
        ("uaselectonmysqluser", names.to_vec()),
        ("uaselectonmysql", names.to_vec()),
        (
            "uacreateonly",
            vec![
                "uacreateonly",
                "uanobody",
                "uaselectonmysql",
                "uaselectonmysqluser",
                "uavictim",
            ],
        ),
    ] {
        let mut source = InternalTestSource::new();
        source.attr_viewer = Some(viewer.into());
        source.attr_privileges = Some(NewUserPrivileges(handle.clone()));
        source.attr_rows = names
            .iter()
            .map(|user| {
                vec![
                    text(user),
                    text("%"),
                    text(if *user == "uavictim" {
                        "{\"secret\": \"victim-data\"}"
                    } else {
                        ""
                    }),
                ]
            })
            .collect();
        let mut reader = retriever(Arc::new(source));
        reader.table.name = "USER_ATTRIBUTES".into();
        let rows = reader.retrieve().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| match &row[0] {
                    Datum::Text(name) => name.as_str(),
                    _ => panic!(),
                })
                .collect::<Vec<_>>(),
            expected,
            "viewer {viewer}"
        );
        for row in rows {
            if row[0] != text("uavictim") {
                assert_eq!(row[2], Datum::Null);
            }
        }
    }
}

#[test]
fn user_attributes_retriever_preserves_shape_errors_and_memory() {
    use std::sync::atomic::{AtomicI64, Ordering};
    struct Tracker(AtomicI64);
    impl crate::infoschema_reader::MemoryTracker for Tracker {
        fn consume(&self, bytes: i64) {
            self.0.fetch_add(bytes, Ordering::SeqCst);
        }
    }
    let mut source = InternalTestSource::new();
    source.attr_rows = vec![
        vec![text("malformed")],
        vec![
            text("victim"),
            text("%"),
            text("{\"secret\": \"victim-data\"}"),
        ],
        vec![text("empty"), text("%"), text("")],
    ];
    let tracker = Arc::new(Tracker(AtomicI64::new(0)));
    let mut reader = retriever(Arc::new(source));
    reader.mem_tracker = Some(tracker.clone());
    reader.table.name = "USER_ATTRIBUTES".into();
    let rows = reader.retrieve().unwrap();
    assert_eq!(
        rows,
        vec![
            vec![
                text("victim"),
                text("%"),
                text("{\"secret\": \"victim-data\"}")
            ],
            vec![text("empty"), text("%"), Datum::Null]
        ]
    );
    assert_eq!(
        tracker.0.load(Ordering::SeqCst),
        ("victim".len() + 1 + "{\"secret\": \"victim-data\"}".len() + "empty".len() + 1) as i64
    );
    let mut source = InternalTestSource::new();
    source.attr_error = true;
    let mut reader = retriever(Arc::new(source));
    assert_eq!(
        reader.setDataForUserAttributes().unwrap_err().0,
        "restricted SQL failed"
    );
    let mut reader = retriever(Arc::new(InternalTestSource::new()));
    reader.setDataForUserAttributes().unwrap();
    assert!(reader.rows.is_empty());
}
