// Copyright 2026 AsterSQL.

// RequiredRows（上层请求的行数上限）在 table reader 路径上的行为测试。
//
// 向量化执行中，父算子通过 RequiredRows 告诉子算子「本次最多需要多少行」，
// 子算子应尽量按该上限拉取，避免过量缓冲。本文件复用
// `table_readers_required_rows_test` 中的 mock backend。

use std::time::Duration;

use astersql_executor_aggregate::agg_stream_executor::StreamAggExec;
use astersql_executor_aggregate::agg_util::{AggKind, Aggregation, Value};
use astersql_executor_sortexec::sort::VecRowSource;
use astersql_executor_sortexec::{DataChunk, Limit, Row, SortExec, SortKey, SortValue, TopNExec};

use crate::table_readers_required_rows_test::{
    RequiredRowsBackend, required_rows_reader, signed_rows,
};

/// 非均匀 Next(required) 序列下，实际产出行数与下推请求一致。
#[test]
fn executor_required_rows_are_respected_across_non_uniform_next_calls() {
    let backend = RequiredRowsBackend::new(signed_rows(10), Duration::ZERO);
    let observed = backend.clone();
    let mut executor = required_rows_reader(backend);
    executor.Open().unwrap();

    // (required, 期望返回行数)：最后一次 required=8 但只剩 2 行；再 Next 得空。
    let expected = [(3, 3), (1, 1), (4, 4), (8, 2), (2, 0)];
    for (required, rows) in expected {
        assert_eq!(executor.Next(required).unwrap().len(), rows);
    }
    assert_eq!(observed.requests(), vec![3, 1, 4, 8, 2]);
    assert_eq!(executor.runtime_rows, 10);
    executor.Close().unwrap();
}

/// required=0 时不消耗子节点行，但仍记录一次请求。
#[test]
fn zero_required_rows_does_not_consume_child_rows() {
    let backend = RequiredRowsBackend::new(signed_rows(2), Duration::ZERO);
    let observed = backend.clone();
    let mut executor = required_rows_reader(backend);
    executor.Open().unwrap();

    assert!(executor.Next(0).unwrap().is_empty());
    assert_eq!(observed.requests(), vec![0]);
    assert_eq!(executor.Next(2).unwrap().len(), 2);
    assert_eq!(observed.requests(), vec![0, 2]);
}

fn sort_rows(count: usize) -> Vec<Row> {
    (0..count)
        .rev()
        .map(|value| Row(vec![SortValue::Int(value as i64)]))
        .collect()
}

/// Go `TestSortRequiredRows`: a materializing sort must still page its output
/// according to every non-uniform request, including the short final page.
#[test]
fn sort_required_rows_match_go_output_pages() {
    let input = sort_rows(10);
    let mut executor = SortExec::new(
        Box::new(VecRowSource::new(vec![DataChunk::new(input)])),
        vec![SortKey::asc(0)],
        1,
        1024,
        -1,
    );
    executor.Open().unwrap();
    let actual = [1, 5, 3, 10].map(|required| executor.Next(required).unwrap().rows.len());
    assert_eq!(actual, [1, 5, 3, 1]);
    assert!(executor.Next(10).unwrap().is_empty());
    executor.Close().unwrap();
}

/// Go `TestTopNRequiredRows`: OFFSET/COUNT is applied before the requested
/// output pages are drained.
#[test]
fn top_n_required_rows_match_go_offset_and_count_pages() {
    let input = sort_rows(100);
    let mut executor = TopNExec::new(
        Box::new(VecRowSource::new(vec![DataChunk::new(input)])),
        vec![SortKey::asc(0)],
        Limit {
            Offset: 15,
            Count: 11,
        },
        None,
        5,
        1024,
        -1,
    );
    executor.Open().unwrap();
    let actual = [1, 1, 1, 1, 10].map(|required| executor.Next(required).unwrap().rows.len());
    assert_eq!(actual, [1, 1, 1, 1, 7]);
    assert!(executor.Next(10).unwrap().is_empty());
    executor.Close().unwrap();
}

fn grouped_rows(total: usize, factor: usize) -> Vec<Vec<Value>> {
    (0..total)
        .map(|value| {
            vec![
                Value::Integer(value as i64),
                Value::Integer((value / factor) as i64),
            ]
        })
        .collect()
}

/// Go `TestStreamAggRequiredRows`: internal aggregate chunking must not leak
/// through the parent's changing RequiredRows requests.
#[test]
fn stream_aggregate_required_rows_are_preserved_between_calls() {
    let mut executor = StreamAggExec::new(
        vec![grouped_rows(30, 1)],
        vec![1],
        vec![Aggregation::new(AggKind::Sum, Some(0))],
        1024,
    );
    executor.open().unwrap();

    let actual =
        [1, 2, 3, 4, 5, 6, 7].map(|required| executor.next_required(required).unwrap().len());
    assert_eq!(actual, [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(executor.next_required(2).unwrap().len(), 2);
    assert!(executor.next_required(1).unwrap().is_empty());
    executor.close();
}

#[test]
fn stream_aggregate_zero_required_rows_does_not_consume_a_group() {
    let mut executor = StreamAggExec::new(
        vec![grouped_rows(2, 1)],
        vec![1],
        vec![Aggregation::new(AggKind::Count, None)],
        1024,
    );
    executor.open().unwrap();
    assert!(executor.next_required(0).unwrap().is_empty());
    assert_eq!(executor.next_required(2).unwrap().len(), 2);
}
