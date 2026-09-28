// Copyright 2026 AsterSQL.

use crate::index_lookup_join::{IndexJoinExecutorBuilder, IndexJoinLookupContent};
use crate::index_lookup_merge_join::IndexLookUpMergeJoin;
use crate::joiner::{JoinType, Joiner, Row};
use crate::row_table_builder::Value;

struct Builder {
    rows: Vec<Row>,
}

impl IndexJoinExecutorBuilder for Builder {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        Ok(self
            .rows
            .iter()
            .filter(|row| {
                lookup_contents
                    .iter()
                    .any(|content| content.keys == vec![row[0].clone()])
            })
            .cloned()
            .collect())
    }
}

#[test]
fn index_lookup_merge_join_can_reopen_after_close_like_go() {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        vec![Value::Null, Value::Null],
        Vec::new(),
        None,
        false,
        32,
    )
    .unwrap();
    let mut executor = IndexLookUpMergeJoin::new(
        vec![vec![Value::Int(1), Value::Text("outer".into())]],
        vec![0],
        vec![0],
        Box::new(Builder {
            rows: vec![vec![Value::Int(1), Value::Text("inner".into())]],
        }),
        joiner,
        1,
        32,
    )
    .unwrap();

    let expected = vec![vec![
        Value::Int(1),
        Value::Text("outer".into()),
        Value::Int(1),
        Value::Text("inner".into()),
    ]];
    assert_eq!(executor.next(32).unwrap().rows, expected);
    executor.close();
    executor.open().unwrap();
    assert_eq!(executor.next(32).unwrap().rows, expected);
}
