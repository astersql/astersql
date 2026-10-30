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

#[test]
fn port_74dc8d6e87_analyze_response_keeps_visible_scan_bytes() {
    use crate::cop_handler::{KeyRange, MemoryReader, Request, RequestPayload, handle_cop_request};
    let reader = MemoryReader {
        rows: [
            (b"k1".to_vec(), (vec![Datum::Int(1)], 4)),
            (b"k2".to_vec(), (vec![Datum::Int(2)], 4)),
            (b"k3".to_vec(), (vec![Datum::Int(3)], 20)),
        ]
        .into_iter()
        .collect(),
    };
    for kind in [
        AnalyzeType::Index,
        AnalyzeType::CommonHandle,
        AnalyzeType::Columns,
        AnalyzeType::Mixed,
        AnalyzeType::FullSampling,
    ] {
        for invalid in [false, true] {
            let mut analyze = index_request(vec![if invalid { 9 } else { 0 }]);
            analyze.analyze_type = kind.clone();
            analyze.primary_column_count = 1;
            let response = handle_cop_request(
                &reader,
                &Request {
                    payload: RequestPayload::Analyze(analyze),
                    ranges: vec![KeyRange {
                        start: b"k".to_vec(),
                        end: vec![],
                    }],
                    start_ts: 10,
                    resolved_locks: vec![],
                    paging_size: 0,
                    cache_enabled: false,
                    cache_if_match_version: 0,
                },
            );
            assert_eq!(response.other_error.is_some(), invalid);
            let bytes = if astersql_config_kerneltype::IsNextGen() {
                22
            } else {
                0
            };
            assert_eq!(response.scan_detail.total_versions_size, bytes);
            assert_eq!(response.scan_detail.processed_versions_size, bytes);
            assert_eq!(
                response.scan_detail.total_versions,
                if bytes > 0 { 2 } else { 0 }
            );
        }
    }
}
