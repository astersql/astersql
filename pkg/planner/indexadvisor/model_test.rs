// Copyright 2026 AsterSQL.

use crate::model::{Column, Index};

#[test]
fn constructors_use_unicode_lowercase_like_go() {
    let column = Column::new("İDB", "T", "ÉCLAIR");
    assert_eq!(column.schema_name, "idb");
    assert_eq!(column.table_name, "t");
    assert_eq!(column.column_name, "éclair");

    let index = Index::new("İDB", "T", "ÉIDX", ["ÉCLAIR"]);
    assert_eq!(index.schema_name, "idb");
    assert_eq!(index.index_name, "éidx");
    assert_eq!(index.columns[0].column_name, "éclair");
}

#[test]
fn with_columns_rebuilds_all_columns_from_the_first_table_like_go() {
    let index = Index::with_columns(
        "ÉIDX",
        vec![
            Column::new("DB", "FIRST", "A"),
            Column::new("OTHER", "SECOND", "B"),
        ],
    )
    .expect("Go accepts columns with different source identities");

    assert_eq!(index.schema_name, "db");
    assert_eq!(index.table_name, "first");
    assert_eq!(index.index_name, "éidx");
    assert_eq!(
        index
            .columns
            .iter()
            .map(|column| {
                (
                    &*column.schema_name,
                    &*column.table_name,
                    &*column.column_name,
                )
            })
            .collect::<Vec<_>>(),
        vec![("db", "first", "a"), ("db", "first", "b")]
    );
}
