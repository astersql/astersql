// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{
    Bucket, ColumnStats, Error, Histogram, JsonTable, PredicateColumn, TableStats, TopNItem,
    blocks_to_json_table, generate_json_table_from_stats, json_table_to_blocks,
    table_stats_from_json,
};

fn column(name: &str, id: i64, stats_version: i64) -> ColumnStats {
    ColumnStats {
        name: name.to_owned(),
        histogram: Histogram {
            id,
            ndv: 3,
            null_count: 1,
            last_update_version: 101,
            total_column_size: 24,
            correlation: 0.75,
            buckets: vec![Bucket {
                count: 4,
                repeat: 2,
                lower: vec![0, 1],
                upper: vec![2, 3],
                ndv: 2,
            }],
        },
        cmsketch: Some(vec![4, 5]),
        top_n: vec![TopNItem {
            encoded: vec![6, 7],
            count: 8,
        }],
        fm_sketch: Some(vec![9, 10]),
        stats_version,
    }
}

fn complete_json_table() -> JsonTable {
    JsonTable {
        database_name: "test".to_owned(),
        table_name: "t".to_owned(),
        stats: TableStats {
            physical_id: 42,
            count: 7,
            modify_count: 2,
            version: 99,
            stats_version: 2,
            columns: HashMap::from([("a".to_owned(), column("a", 1, 2))]),
            indices: HashMap::from([("idx".to_owned(), column("idx", 2, 2))]),
        },
        predicate_columns: vec![
            PredicateColumn {
                id: 1,
                last_used_at: None,
                last_analyzed_at: Some("2026-01-02 03:04:05".to_owned()),
            },
            PredicateColumn {
                id: 2,
                last_used_at: Some("2026-01-01 01:02:03".to_owned()),
                last_analyzed_at: Some("2026-01-02 03:04:05".to_owned()),
            },
        ],
        is_historical_stats: true,
    }
}

#[derive(Default)]
struct DumpLoadStore {
    statements: Mutex<Vec<String>>,
}

impl DumpLoadStore {
    fn statements(&self) -> Vec<String> {
        self.statements.lock().unwrap().clone()
    }
}

impl crate::SqlStore for DumpLoadStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(123)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        self.statements.lock().unwrap().push(sql.to_owned());
        Ok(Vec::new())
    }
}

#[test]
fn dump_and_load_preserve_complete_table_statistics() {
    let table = complete_json_table();
    let blocks = json_table_to_blocks(&table, 11).unwrap();

    assert!(
        blocks.len() > 1,
        "small blocks must exercise ordered reassembly"
    );
    assert!(blocks.iter().all(|block| block.len() <= 11));
    assert_eq!(blocks_to_json_table(&blocks).unwrap(), table);
}

#[test]
fn fm_sketch_dump_load_persists_columns_and_indices() {
    let blocks = json_table_to_blocks(&complete_json_table(), 11).unwrap();
    let loaded = blocks_to_json_table(&blocks).unwrap();
    let store = DumpLoadStore::default();

    crate::save_column_or_index_stats(&store, 42, false, &loaded.stats.columns["a"]).unwrap();
    crate::save_column_or_index_stats(&store, 42, true, &loaded.stats.indices["idx"]).unwrap();

    let statements = store.statements();
    for is_index in [0, 1] {
        let fm_insert = statements
            .iter()
            .position(|sql| {
                sql == &format!(
                    "insert into mysql.stats_fm_sketch (table_id,is_index,hist_id,value) values (42,{is_index},{},x'090a')",
                    is_index + 1
                )
            })
            .expect("dump/load must restore the FM sketch for both columns and indices");
        let histogram_replace = statements
            .iter()
            .position(|sql| {
                sql.starts_with("replace into mysql.stats_histograms")
                    && sql.contains(&format!("(42,{is_index},{},", is_index + 1))
            })
            .expect("dump/load must restore the matching histogram");
        assert!(
            fm_insert < histogram_replace,
            "the FM sketch must be committed with the histogram before a concurrent ANALYZE can run"
        );
    }
}

#[test]
fn fm_sketch_dump_without_sketches_clears_stale_rows() {
    let mut json = complete_json_table();
    json.stats.columns.get_mut("a").unwrap().fm_sketch = None;
    json.stats.indices.get_mut("idx").unwrap().fm_sketch = None;
    let loaded = blocks_to_json_table(&json_table_to_blocks(&json, 11).unwrap()).unwrap();
    let store = DumpLoadStore::default();

    crate::save_column_or_index_stats(&store, 42, false, &loaded.stats.columns["a"]).unwrap();
    crate::save_column_or_index_stats(&store, 42, true, &loaded.stats.indices["idx"]).unwrap();

    let statements = store.statements();
    assert!(statements.iter().any(|sql| {
        sql == "delete from mysql.stats_fm_sketch where table_id=42 and is_index=0 and hist_id=1"
    }));
    assert!(statements.iter().any(|sql| {
        sql == "delete from mysql.stats_fm_sketch where table_id=42 and is_index=1 and hist_id=2"
    }));
    assert!(
        !statements
            .iter()
            .any(|sql| sql.starts_with("insert into mysql.stats_fm_sketch")),
        "a dump without sketches must not retain sketches from different statistics"
    );
}

#[test]
fn conversion_rebinds_physical_id_and_promotes_legacy_items() {
    let mut json = complete_json_table();
    json.stats.physical_id = 1;
    json.stats.stats_version = 0;
    json.stats.columns.get_mut("a").unwrap().stats_version = 0;
    json.stats.indices.get_mut("idx").unwrap().stats_version = 0;

    let converted = table_stats_from_json(88, &json);

    assert_eq!(converted.physical_id, 88);
    assert_eq!(converted.stats_version, 1);
    assert_eq!(converted.columns["a"].stats_version, 1);
    assert_eq!(converted.indices["idx"].stats_version, 1);
    assert_eq!(converted.count, json.stats.count);
    assert_eq!(converted.modify_count, json.stats.modify_count);
}

#[test]
fn conversion_keeps_empty_legacy_items_uninitialized() {
    let mut json = complete_json_table();
    let empty = ColumnStats {
        name: "empty".to_owned(),
        ..Default::default()
    };
    json.stats.stats_version = 0;
    json.stats.columns = HashMap::from([("empty".to_owned(), empty.clone())]);
    json.stats.indices = HashMap::from([("empty_idx".to_owned(), empty)]);

    let converted = table_stats_from_json(42, &json);

    assert_eq!(converted.stats_version, 0);
    assert_eq!(converted.columns["empty"].stats_version, 0);
    assert_eq!(converted.indices["empty_idx"].stats_version, 0);
}

#[test]
fn predicate_usage_is_sorted_and_keeps_nil_timestamps() {
    let stats = complete_json_table().stats;
    let usage = HashMap::from([
        (2, (Some("used".to_owned()), Some("analyzed".to_owned()))),
        (1, (None, Some("analyzed".to_owned()))),
    ]);
    let mut cancellation_checks = 0;

    let json = generate_json_table_from_stats("test", "t", &stats, &usage, || {
        cancellation_checks += 1;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        cancellation_checks,
        stats.columns.len() + stats.indices.len()
    );
    assert_eq!(
        json.predicate_columns
            .iter()
            .map(|column| column.id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(json.predicate_columns[0].last_used_at, None);
    assert_eq!(
        json.predicate_columns[0].last_analyzed_at.as_deref(),
        Some("analyzed")
    );
    assert!(!json.is_historical_stats);
}

#[test]
fn dump_stops_at_the_first_cancellation_error() {
    let stats = complete_json_table().stats;
    let usage = HashMap::new();
    let mut cancellation_checks = 0;

    let error = generate_json_table_from_stats("test", "t", &stats, &usage, || {
        cancellation_checks += 1;
        Err(Error("cancelled".to_owned()))
    })
    .unwrap_err();

    assert_eq!(error.0, "cancelled");
    assert_eq!(cancellation_checks, 1);
}

#[test]
fn block_conversion_rejects_invalid_boundaries() {
    assert_eq!(
        json_table_to_blocks(&complete_json_table(), 0)
            .unwrap_err()
            .0,
        "block size must be positive"
    );
    assert_eq!(
        blocks_to_json_table(&[]).unwrap_err().0,
        "Block empty error"
    );

    let mut blocks = json_table_to_blocks(&complete_json_table(), 17).unwrap();
    blocks.pop();
    assert!(blocks_to_json_table(&blocks).is_err());
}
