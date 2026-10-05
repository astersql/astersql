// Copyright 2026 AsterSQL.

//! Go parity tests for the MPP executor implementation.

use crate::cop_handler::{
    AggCall, AggKind, Datum, Executor, Expr, JoinType, KeyRange, MemoryReader,
};
use crate::mpp_exec::execute_executor;

fn aggregation_reader() -> MemoryReader {
    let mut reader = MemoryReader::default();
    for (key, group) in [(b"a".to_vec(), 2), (b"b".to_vec(), 1), (b"c".to_vec(), 2)] {
        reader.rows.insert(key, (vec![Datum::Int(group)], 1));
    }
    reader
}

fn count_by_group(stream: bool) -> Executor {
    Executor::Aggregation {
        group_by: vec![crate::cop_handler::Expr::Column(0)],
        calls: vec![AggCall {
            kind: AggKind::Count,
            expr: None,
        }],
        child: Box::new(Executor::TableScan {
            columns: vec![0],
            descending: false,
        }),
        stream,
    }
}

/// Go `aggExec.processAllRows` emits aggregate columns before grouping columns.
#[test]
fn hash_aggregation_keeps_go_column_order() {
    let output = execute_executor(
        &aggregation_reader(),
        &[KeyRange::default()],
        1,
        &count_by_group(false),
    )
    .unwrap();

    assert_eq!(
        output.rows,
        vec![
            vec![Datum::Uint(2), Datum::Int(2)],
            vec![Datum::Uint(1), Datum::Int(1)],
        ]
    );
}

/// Go records `groupKeys` on first sight instead of sorting groups by encoded key.
#[test]
fn hash_aggregation_keeps_first_seen_group_order() {
    let output = execute_executor(
        &aggregation_reader(),
        &[KeyRange::default()],
        1,
        &count_by_group(false),
    )
    .unwrap();

    assert_eq!(
        output
            .rows
            .iter()
            .map(|row| row[1].clone())
            .collect::<Vec<_>>(),
        vec![Datum::Int(2), Datum::Int(1)]
    );
}

/// Go allocates the outer-join default inner row from the build child's field types.
#[test]
fn left_outer_join_null_pads_an_empty_right_input_to_its_schema_width() {
    let reader = aggregation_reader();
    let scan = || Executor::TableScan {
        columns: vec![0],
        descending: false,
    };
    let executor = Executor::Join {
        join_type: JoinType::LeftOuter,
        left_key: Expr::Column(0),
        right_key: Expr::Column(0),
        left: Box::new(scan()),
        right: Box::new(Executor::Selection {
            condition: Expr::Constant(Datum::Int(0)),
            child: Box::new(Executor::TableScan {
                columns: vec![0, 0],
                descending: false,
            }),
        }),
    };

    let output = execute_executor(&reader, &[KeyRange::default()], 1, &executor).unwrap();

    assert_eq!(output.rows.len(), 3);
    assert!(
        output
            .rows
            .iter()
            .all(|row| row.len() == 3 && row[1..] == [Datum::Null, Datum::Null])
    );
}

/// Parent executors merge scan details from both table and index scan children.
#[test]
fn join_aggregates_table_and_index_scan_details() {
    let reader = aggregation_reader();
    let executor = Executor::Join {
        join_type: JoinType::Inner,
        left_key: Expr::Column(0),
        right_key: Expr::Column(0),
        left: Box::new(Executor::TableScan {
            columns: vec![0],
            descending: false,
        }),
        right: Box::new(Executor::IndexScan {
            columns: vec![0],
            unique: false,
            descending: false,
        }),
    };

    let output = execute_executor(&reader, &[KeyRange::default()], 1, &executor).unwrap();

    assert_eq!(output.scan_detail.processed_versions, 6);
    assert_eq!(output.scan_detail.total_versions, 6);
    assert_eq!(
        output.scan_detail.processed_versions_size,
        output.scan_detail.total_versions_size
    );
    assert!(output.scan_detail.processed_versions_size > 0);
}
