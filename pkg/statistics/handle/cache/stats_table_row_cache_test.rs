// Copyright 2026 AsterSQL.

use super::{
    CacheError, ColumnMeta, IndexColumnMeta, IndexMeta, RowStatsProvider, StatsTableRowCache,
    TableMeta,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct RecordingProvider {
    column_reads: AtomicUsize,
}

impl RowStatsProvider for RecordingProvider {
    fn RowCounts(&self, _: &[i64]) -> Result<HashMap<i64, u64>, CacheError> {
        Ok(HashMap::from([(7, 42)]))
    }

    fn ColumnLengths(&self, _: &[i64]) -> Result<HashMap<(i64, i64), u64>, CacheError> {
        self.column_reads.fetch_add(1, Ordering::SeqCst);
        Ok(HashMap::from([((7, 3), 99)]))
    }
}

#[test]
fn table_rows_only_update_skips_column_length_provider() {
    let provider = RecordingProvider::default();
    let cache = StatsTableRowCache::default();

    cache.UpdateByID(&provider, &[7], false).unwrap();
    assert_eq!(cache.GetTableRows(7), 42);
    assert_eq!(provider.column_reads.load(Ordering::SeqCst), 0);

    cache.UpdateByID(&provider, &[7], true).unwrap();
    assert_eq!(provider.column_reads.load(Ordering::SeqCst), 1);

    cache.UpdateByID(&provider, &[7], false).unwrap();
    assert_eq!(
        cache.GetColLength(super::tableHistID {
            tableID: 7,
            histID: 3,
        }),
        0
    );
}

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
