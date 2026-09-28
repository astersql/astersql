// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
    InfoSchemaError, InfoSchemaSnapshot, PredicateExtractor, Row, SessionState, TableInfo,
    TiFlashInstance, initialTable, memtableRetriever, tableStorageStatsRetriever,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Request {
    ClusterInfo,
    TiKvRegionStatus,
    TableStorageStats { table_cursor: usize, batch: usize },
}

struct ClusterTableSource {
    cluster_info: InfoResult<Vec<Row>>,
    region_status: InfoResult<Vec<Row>>,
    storage_stats: Vec<Row>,
    initial_tables: Vec<initialTable>,
    requests: Mutex<Vec<Request>>,
}

impl ClusterTableSource {
    fn new() -> Self {
        Self {
            cluster_info: Ok(Vec::new()),
            region_status: Ok(Vec::new()),
            storage_stats: Vec::new(),
            initial_tables: Vec::new(),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl InfoSchemaDataSource for ClusterTableSource {
    fn session_state(&self) -> InfoResult<SessionState> {
        Ok(SessionState {
            in_transaction: false,
            transaction_start_ts: 0,
            snapshot_ts: 0,
            current_user: "root".to_owned(),
            current_host: "127.0.0.1".to_owned(),
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
        match request {
            DataRequest::ClusterInfo => {
                self.requests.lock().unwrap().push(Request::ClusterInfo);
                self.cluster_info.clone()
            }
            DataRequest::TiKvRegionStatus => {
                self.requests
                    .lock()
                    .unwrap()
                    .push(Request::TiKvRegionStatus);
                self.region_status.clone()
            }
            DataRequest::TableStorageStats {
                table_cursor,
                batch,
            } => {
                self.requests
                    .lock()
                    .unwrap()
                    .push(Request::TableStorageStats {
                        table_cursor,
                        batch,
                    });
                Ok(self.storage_stats.clone())
            }
            other => panic!("unexpected information-schema request: {other:?}"),
        }
    }

    fn privilege_verification(&self, _: &str, _: &str, _: &str) -> InfoResult<Option<bool>> {
        Ok(Some(true))
    }

    fn auto_increment_id(&self, _: &InfoSchemaSnapshot, _: i64) -> InfoResult<Option<i64>> {
        Ok(None)
    }

    fn update_stats_cache(&self, _: &[i64]) -> InfoResult {
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
        Ok(self.initial_tables.clone())
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

fn table(name: &str, column_names: &[&str]) -> TableInfo {
    TableInfo {
        id: 1,
        schema: "information_schema".to_owned(),
        name: name.to_owned(),
        columns: column_names
            .iter()
            .enumerate()
            .map(|(offset, name)| ColumnInfo {
                name: (*name).to_owned(),
                offset,
            })
            .collect(),
        partition_ids: Vec::new(),
    }
}

fn retriever(name: &str, columns: &[&str], source: Arc<ClusterTableSource>) -> memtableRetriever {
    let table = table(name, columns);
    memtableRetriever {
        columns: table.columns.clone(),
        table,
        rows: Vec::new(),
        row_idx: 0,
        retrieved: false,
        initialized: false,
        extractor: PredicateExtractor::default(),
        info_schema: None,
        mem_tracker: None,
        accumulated_memory_per_batch: 0,
        accumulated_memory_record_count: 0,
        source,
    }
}

#[test]
fn cluster_info_preserves_node_rows_and_propagates_source_errors() {
    let expected = vec![
        vec![text("tidb"), text(":4000"), text(":10080"), text("8.4.0")],
        vec![
            text("pd"),
            text("127.0.0.1:2379"),
            text("127.0.0.1:2379"),
            text("4.0.0-alpha"),
        ],
        vec![text("tikv"), text("store1"), text(""), text("")],
    ];
    let source = Arc::new(ClusterTableSource {
        cluster_info: Ok(expected.clone()),
        ..ClusterTableSource::new()
    });
    let mut reader = retriever(
        "CLUSTER_INFO",
        &["TYPE", "INSTANCE", "STATUS_ADDRESS", "VERSION"],
        Arc::clone(&source),
    );

    assert_eq!(reader.retrieve().unwrap(), expected);
    assert!(reader.retrieve().unwrap().is_empty());
    assert_eq!(source.requests(), vec![Request::ClusterInfo]);

    let source = Arc::new(ClusterTableSource {
        cluster_info: Err(InfoSchemaError::new("pd unavailable")),
        ..ClusterTableSource::new()
    });
    let error = retriever("CLUSTER_INFO", &["TYPE"], source)
        .retrieve()
        .unwrap_err();
    assert_eq!(error.to_string(), "pd unavailable");
}

#[test]
fn tikv_region_status_keeps_table_partition_and_global_index_shapes() {
    let expected = vec![
        vec![
            text("test"),
            text("test_t1"),
            Datum::Int(0),
            Datum::Null,
            Datum::Null,
        ],
        vec![
            text("test"),
            text("test_t1"),
            Datum::Int(1),
            text("p_a"),
            Datum::Null,
        ],
        vec![
            text("test"),
            text("test_t2"),
            Datum::Int(0),
            Datum::Null,
            text("p0"),
        ],
        vec![
            text("test"),
            text("test_t2"),
            Datum::Int(0),
            Datum::Null,
            text("p1"),
        ],
        vec![
            text("test"),
            text("test_t2"),
            Datum::Int(1),
            text("p_a"),
            text("p0"),
        ],
        vec![
            text("test"),
            text("test_t2"),
            Datum::Int(1),
            text("p_a"),
            text("p1"),
        ],
        vec![
            text("test"),
            text("test_t2"),
            Datum::Int(1),
            text("p_b"),
            Datum::Null,
        ],
    ];
    let source = Arc::new(ClusterTableSource {
        region_status: Ok(expected.clone()),
        ..ClusterTableSource::new()
    });
    let mut reader = retriever(
        "TIKV_REGION_STATUS",
        &[
            "DB_NAME",
            "TABLE_NAME",
            "IS_INDEX",
            "INDEX_NAME",
            "PARTITION_NAME",
        ],
        Arc::clone(&source),
    );

    assert_eq!(reader.retrieve().unwrap(), expected);
    assert_eq!(source.requests(), vec![Request::TiKvRegionStatus]);
}

#[test]
fn table_storage_stats_requires_schema_and_returns_partition_rows() {
    let initial_tables = vec![initialTable {
        database: "test".to_owned(),
        table: table("tp", &["TABLE_SCHEMA", "TABLE_NAME"]),
    }];
    let expected = vec![
        vec![
            text("test"),
            text("t"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
        vec![
            text("test"),
            text("tp"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
        vec![
            text("test"),
            text("tp"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
        vec![
            text("test"),
            text("tp"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
        vec![
            text("test"),
            text("tp"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
        vec![
            text("test"),
            text("tp"),
            Datum::Int(1),
            Datum::Int(1),
            Datum::Int(1),
        ],
    ];
    let source = Arc::new(ClusterTableSource {
        storage_stats: expected.clone(),
        initial_tables,
        ..ClusterTableSource::new()
    });
    let output_table = table(
        "TABLE_STORAGE_STATS",
        &[
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "REGION_COUNT",
            "TABLE_SIZE",
            "TABLE_KEYS",
        ],
    );
    let make_reader = |extractor| tableStorageStatsRetriever {
        output_columns: output_table.columns.clone(),
        table: output_table.clone(),
        retrieved: false,
        initialized: false,
        extractor,
        initial_tables: Vec::new(),
        current_table: 0,
        source: Arc::clone(&source) as Arc<dyn InfoSchemaDataSource>,
    };

    let error = make_reader(PredicateExtractor::default())
        .retrieve()
        .unwrap_err();
    assert_eq!(error.to_string(), "TABLE_SCHEMA predicate is required");
    assert!(source.requests().is_empty());

    let mut extractor = PredicateExtractor::default();
    extractor.schemas.insert("test".to_owned());
    let mut reader = make_reader(extractor);
    assert_eq!(reader.retrieve().unwrap(), expected);
    assert!(reader.retrieve().unwrap().is_empty());
    assert_eq!(
        source.requests(),
        vec![Request::TableStorageStats {
            table_cursor: 0,
            batch: 1024,
        }]
    );
}
