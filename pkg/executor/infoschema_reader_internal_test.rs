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
}

impl InternalTestSource {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<DataRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl InfoSchemaDataSource for InternalTestSource {
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
        self.requests.lock().unwrap().push(request.clone());
        Ok(match request {
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
            other => panic!("unexpected information-schema request: {other:?}"),
        })
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
