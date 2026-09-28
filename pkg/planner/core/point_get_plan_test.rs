// Copyright 2026 AsterSQL.

use crate::planbuilder::{ColumnInfo, TableInfo, Value};
use crate::point_get_plan::{FastField, FastPlan, FastQuery, Predicate, TryFastPlan};
use crate::task::{FieldType, TypeCode};

fn int_primary_key_table() -> TableInfo {
    TableInfo {
        id: 1,
        db: "test".into(),
        name: "t".into(),
        columns: vec![ColumnInfo {
            id: 1,
            name: "id".into(),
            offset: 0,
            field_type: FieldType {
                code: TypeCode::Int,
                flen: 11,
                decimal: 0,
                unsigned: false,
            },
            generated: false,
            stored: false,
            hidden: false,
            primary_key: true,
        }],
        indices: Vec::new(),
        partitions: Vec::new(),
        common_handle: false,
        pk_is_handle: true,
        temporary: false,
    }
}

#[test]
fn primary_key_in_uses_converted_handle_values_like_go() {
    let query = FastQuery {
        table: int_primary_key_table(),
        alias: None,
        fields: vec![FastField {
            column: "id".into(),
            alias: None,
            row_checksum: false,
        }],
        predicates: vec![Predicate::In(
            "id".into(),
            vec![Value::String("1".into()), Value::String("2".into())],
        )],
        lock: None,
        order_desc: false,
        limit: None,
        index_hints: Vec::new(),
        ignore_index_hints: Vec::new(),
    };

    let Some(FastPlan::Batch(plan)) = TryFastPlan(&query, true, 50_000) else {
        panic!("convertible primary-key IN values should use BatchPointGet");
    };
    assert!(matches!(
        plan.handles.as_slice(),
        [Value::Int(1), Value::Int(2)]
    ));
}

#[test]
fn null_safe_equality_does_not_enter_go_eq_only_fast_path() {
    let query = FastQuery {
        table: int_primary_key_table(),
        alias: None,
        fields: vec![FastField {
            column: "id".into(),
            alias: None,
            row_checksum: false,
        }],
        predicates: vec![Predicate::NullSafeEq("id".into(), Value::Int(1))],
        lock: None,
        order_desc: false,
        limit: None,
        index_hints: Vec::new(),
        ignore_index_hints: Vec::new(),
    };

    assert!(TryFastPlan(&query, true, 50_000).is_none());
}
