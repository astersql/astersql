// Copyright 2026 AsterSQL.

use super::{ColumnMeta, IndexColumnMeta, IndexMeta, StatsTableRowCache, TableMeta};

#[test]
fn data_and_index_lengths_match_go_uint64_overflow() {
    let cache = StatsTableRowCache::default();
    let table = TableMeta {
        Columns: vec![ColumnMeta {
            ID: 1,
            FixedLength: Some(u64::MAX),
            Public: true,
        }],
        Indices: vec![IndexMeta {
            Public: true,
            Columns: vec![IndexColumnMeta {
                Offset: 0,
                Length: Some(u64::MAX),
            }],
            ..IndexMeta::default()
        }],
        ..TableMeta::default()
    };

    // Go's uint64 arithmetic wraps for both column and prefix-index lengths.
    assert_eq!(
        cache.GetDataAndIndexLength(&table, 1, 2),
        (u64::MAX - 1, u64::MAX - 1)
    );
}

#[test]
fn accumulated_lengths_match_go_uint64_overflow() {
    let cache = StatsTableRowCache::default();
    let table = TableMeta {
        Columns: vec![
            ColumnMeta {
                ID: 1,
                FixedLength: Some(u64::MAX),
                Public: true,
            },
            ColumnMeta {
                ID: 2,
                FixedLength: Some(1),
                Public: true,
            },
        ],
        ..TableMeta::default()
    };

    assert_eq!(cache.GetDataAndIndexLength(&table, 1, 1).0, 0);
}
