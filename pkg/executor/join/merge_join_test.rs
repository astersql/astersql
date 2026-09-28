// Copyright 2026 AsterSQL.

use crate::merge_join::MergeJoinTable;
use crate::row_table_builder::Value;

#[test]
fn inner_table_skips_null_join_key_groups_like_go() {
    let rows = vec![
        vec![Value::Null, Value::Int(10)],
        vec![Value::Null, Value::Int(11)],
        vec![Value::Int(1), Value::Int(12)],
    ];
    let mut table = MergeJoinTable::new(rows, vec![0], true).unwrap();
    table.init();

    let group = table
        .select_next_group()
        .unwrap()
        .expect("the non-null group remains");

    assert_eq!(group, &[vec![Value::Int(1), Value::Int(12)]]);
}
