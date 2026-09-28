// Copyright 2026 AsterSQL.

use crate::reader_wrapper::{
    ColumnChunkMeta, FileMeta, InMemoryReaderBase, RowGroupRange, row_group_range_from_meta,
};

#[test]
fn empty_row_group_keeps_go_max_int64_start_sentinel() {
    let meta = FileMeta {
        source_size: 0,
        old_parquet_mr: false,
        row_groups: vec![vec![]],
    };

    let range = row_group_range_from_meta(&meta, 0).unwrap();
    assert_eq!(range.start, i64::MAX);
    assert_eq!(range.end, 0);
    assert!(range.column_starts.is_empty());
    assert!(range.column_ends.is_empty());
}

#[test]
fn old_parquet_range_matches_go_when_chunk_crosses_source_end() {
    let meta = FileMeta {
        source_size: 100,
        old_parquet_mr: true,
        row_groups: vec![vec![ColumnChunkMeta {
            data_page_offset: 90,
            dictionary_page_offset: None,
            total_compressed_size: 20,
        }]],
    };

    let range = row_group_range_from_meta(&meta, 0).unwrap();
    assert_eq!((range.start, range.end), (90, 100));
    assert_eq!(range.column_starts, vec![90]);
    assert_eq!(range.column_ends, vec![100]);
}

#[test]
fn in_memory_reader_reports_eof_for_short_and_past_end_reads() {
    let base = InMemoryReaderBase::new(
        b"0123456789",
        RowGroupRange {
            start: 2,
            end: 5,
            column_starts: vec![2],
            column_ends: vec![5],
        },
    )
    .unwrap();

    let mut short = [0; 4];
    assert_eq!(base.read_at(&mut short, 3).unwrap_err().to_string(), "EOF");
    let mut past_end = [0; 1];
    assert_eq!(
        base.read_at(&mut past_end, 5).unwrap_err().to_string(),
        "EOF"
    );
}
