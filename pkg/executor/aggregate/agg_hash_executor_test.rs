// Copyright 2026 AsterSQL.

use super::agg_hash_executor::{HashAggExec, HashAggInput};
use super::agg_util::{AggKind, Aggregation, Value};

#[test]
fn spill_merges_tail_rows_with_their_existing_group() {
    let first_chunk = (0..10)
        .map(|group| vec![Value::Integer(group), Value::Integer(1)])
        .collect();
    let input = HashAggInput {
        chunks: vec![
            first_chunk,
            vec![vec![Value::Integer(2), Value::Integer(1)]],
        ],
        group_columns: vec![0],
        aggregations: vec![Aggregation::new(AggKind::Sum, Some(1))],
    };
    let mut exec = HashAggExec::new(input, 1, 2, 32, Some(2_000));

    exec.open();
    let mut rows = Vec::new();
    while let Some(chunk) = exec.next().unwrap() {
        rows.extend(chunk);
    }
    rows.sort_by_key(|row| match row[0] {
        Value::Integer(group) => group,
        _ => unreachable!("integer group expected"),
    });

    assert!(exec.is_spill_triggered());
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[2], vec![Value::Integer(2), Value::Float(2.0)]);
}
