// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::{BTreeMap, BTreeSet};

use crate::infoschema_reader::{
    DataRequest, DeadlockRecord, InfoResult, InfoSchemaDataSource, InfoSchemaSnapshot, LabelRule,
    PredicateExtractor, Row, SessionState, TiFlashInstance, checkRule, decodeTableIDFromRule,
    initialTable,
};

fn label_rule(id: &str) -> LabelRule {
    LabelRule {
        id: id.to_owned(),
        rule_type: "key-range".to_owned(),
        labels: vec![("key".to_owned(), "value".to_owned())],
        data: vec![BTreeMap::from([
            ("start_key".to_owned(), "aa".to_owned()),
            ("end_key".to_owned(), "bb".to_owned()),
        ])],
        keyspace_mode: true,
    }
}

#[test]
fn check_rule_with_keyspace_id_matches_go_cases() {
    let cases = [
        ("invalid non-keyspace prefix", "foo/test/t1", None),
        ("missing table name", "schema/test", None),
        (
            "non-keyspace table",
            "schema/test/t1",
            Some(("test", "t1", "")),
        ),
        (
            "invalid keyspace inner prefix",
            "keyspace/42/foo/test/t2",
            None,
        ),
        (
            "missing keyspace table name",
            "keyspace/42/schema/test",
            None,
        ),
        (
            "keyspace table",
            "keyspace/42/schema/test/t2",
            Some(("test", "t2", "")),
        ),
        (
            "keyspace partition",
            "keyspace/42/schema/test/t3/p0",
            Some(("test", "t3", "p0")),
        ),
    ];

    for (name, id, expected) in cases {
        let actual = checkRule(&label_rule(id));
        match expected {
            Some((database, table, partition)) => assert_eq!(
                actual.unwrap(),
                (database.to_owned(), table.to_owned(), partition.to_owned()),
                "case {name}"
            ),
            None => assert!(actual.is_err(), "case {name} unexpectedly succeeded"),
        }
    }
}

struct DecodeSource;

impl InfoSchemaDataSource for DecodeSource {
    fn session_state(&self) -> InfoResult<SessionState> {
        unreachable!()
    }
    fn snapshot_info_schema(&self, _: u64) -> InfoResult<InfoSchemaSnapshot> {
        unreachable!()
    }
    fn latest_info_schema(&self) -> InfoResult<InfoSchemaSnapshot> {
        unreachable!()
    }
    fn transaction_info_schema(&self) -> InfoResult<InfoSchemaSnapshot> {
        unreachable!()
    }
    fn load_rows(
        &self,
        _: DataRequest,
        _: Option<&InfoSchemaSnapshot>,
        _: Option<&PredicateExtractor>,
    ) -> InfoResult<Vec<Row>> {
        unreachable!()
    }
    fn privilege_verification(&self, _: &str, _: &str, _: &str) -> InfoResult<Option<bool>> {
        unreachable!()
    }
    fn auto_increment_id(&self, _: &InfoSchemaSnapshot, _: i64) -> InfoResult<Option<i64>> {
        unreachable!()
    }
    fn update_stats_cache(&self, _: &[i64]) -> InfoResult {
        unreachable!()
    }
    fn ddl_jobs_open(&self, _: &InfoSchemaSnapshot) -> InfoResult<u64> {
        unreachable!()
    }
    fn ddl_jobs_next(&self, _: u64, _: usize) -> InfoResult<Vec<Row>> {
        unreachable!()
    }
    fn ddl_jobs_close(&self, _: u64) -> InfoResult {
        unreachable!()
    }
    fn initial_tables(&self, _: &PredicateExtractor) -> InfoResult<Vec<initialTable>> {
        unreachable!()
    }
    fn transaction_row_count(&self) -> InfoResult<usize> {
        unreachable!()
    }
    fn data_lock_wait_count(&self) -> InfoResult<usize> {
        unreachable!()
    }
    fn deadlock_records(&self) -> InfoResult<Vec<DeadlockRecord>> {
        unreachable!()
    }
    fn tiflash_instances(&self, _: &BTreeSet<String>) -> InfoResult<Vec<TiFlashInstance>> {
        unreachable!()
    }
    fn analyze_total_count(&self, _: &str, _: &str, _: &str) -> InfoResult<f64> {
        unreachable!()
    }
    fn decode_table_id_from_start_key(&self, key: &[u8]) -> InfoResult<i64> {
        assert_eq!(key, b"table-keyspace-42");
        Ok(123)
    }
    fn table_matches_id(&self, _: &str, _: &str, _: &str, _: i64) -> InfoResult<bool> {
        unreachable!()
    }
}

#[test]
fn decode_table_id_from_keyspace_rule_matches_go_contract() {
    let mut rule = label_rule("");
    rule.labels = vec![("merge_option".to_owned(), "allow".to_owned())];
    rule.data = vec![BTreeMap::from([
        (
            "start_key".to_owned(),
            "7461626c652d6b657973706163652d3432".to_owned(),
        ),
        ("end_key".to_owned(), "ff".to_owned()),
    ])];

    assert_eq!(decodeTableIDFromRule(&rule, &DecodeSource).unwrap(), 123);
}
