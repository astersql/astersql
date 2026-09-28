// Copyright 2026 AsterSQL.

use crate::init::parse_create_table;

#[test]
fn unnamed_unique_index_uses_first_column_name_like_go() {
    let table = parse_create_table(
        "CREATE TABLE performance_schema.example (\
         USER CHAR(32), EVENT_NAME VARCHAR(128), \
         UNIQUE KEY (USER, EVENT_NAME) USING HASH);",
    )
    .expect("parse unnamed unique index");

    assert_eq!(table.indices.len(), 1);
    assert_eq!(table.indices[0].name, "USER");
    assert_eq!(table.indices[0].columns, vec![0, 1]);
}
