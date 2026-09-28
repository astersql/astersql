// Copyright 2026 AsterSQL.

use crate::analyze::{AnalyzeRequest, AnalyzeType, analyze_index, build_histogram};
use crate::cop_handler::Datum;

fn index_request(column_offsets: Vec<usize>) -> AnalyzeRequest {
    AnalyzeRequest {
        analyze_type: AnalyzeType::Index,
        column_offsets,
        bucket_count: 2,
        sample_size: 0,
        sketch_depth: 2,
        sketch_width: 64,
        primary_column_count: 0,
    }
}

#[test]
fn histogram_bucket_counts_are_cumulative_like_go_sorted_builder() {
    let histogram = build_histogram(
        &[Datum::Int(1), Datum::Int(2), Datum::Int(3), Datum::Int(4)],
        2,
    );

    assert_eq!(
        histogram
            .buckets
            .iter()
            .map(|bucket| bucket.count)
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
}

#[test]
fn composite_index_cms_counts_every_column_prefix_like_go() {
    let rows = vec![
        vec![Datum::Int(1), Datum::Bytes(b"a".to_vec())],
        vec![Datum::Int(2), Datum::Bytes(b"b".to_vec())],
    ];

    let result = analyze_index(&rows, &index_request(vec![0, 1])).unwrap();
    let inserted_counters = result.cms.iter().flatten().sum::<u64>();

    assert_eq!(inserted_counters, 8);
}
