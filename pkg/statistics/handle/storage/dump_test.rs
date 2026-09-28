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
