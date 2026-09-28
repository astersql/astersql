// Copyright 2026 AsterSQL.

use encode::{Column, Datum};

use crate::{TableDefinition, canonicalHandle};

#[test]
fn integer_primary_key_handle_uses_the_primary_key_column() {
    let table = TableDefinition {
        pk_is_handle: true,
        columns: vec![
            Column {
                name: "payload".into(),
                ..Default::default()
            },
            Column {
                name: "id".into(),
                primary_key: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let handle =
        canonicalHandle(&table, &[Datum::String("row".into()), Datum::Int(42)], 7).unwrap();
    assert!(handle.IsInt());
    assert_eq!(handle.IntValue(), 42);
}

#[test]
fn common_handle_rejects_a_missing_primary_key_value() {
    let table = TableDefinition {
        common_handle: true,
        columns: vec![
            Column {
                name: "tenant".into(),
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "id".into(),
                primary_key: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let error = match canonicalHandle(&table, &[Datum::String("acme".into())], 7) {
        Ok(_) => panic!("missing common-handle column must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("missing primary-key column id"), "{error}");
}
